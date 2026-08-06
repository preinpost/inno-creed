//! 통합검색 모듈 — `/gw/APIHandler/gw018A02`. 읽기 전용.
//!
//! 메일·전자결재·게시판·일정·자원·파일을 **하나의 API**로 검색한다. 모듈별 전용 검색
//! API(mail003A01 등)에는 검색 파라미터가 없다 — 포털 통합검색이 유일한 경로다.
//! 실측: `.claude-workspace/analyze/10-endpoint-discovery-js-bundle.md` §⑤.
//!
//! ⚠️ 검색어 필드명은 `tsearchKeyword`다. `searchText`/`keyword` 같은 이름을 넣으면
//! **서버가 조용히 무시하고 필터 없는 전체 결과를 준다**(에러 없음).

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::client::GwClient;

/// 검색 범위 → (boardType, 표시명). boardType은 실측으로 확정한 모듈 구분자.
fn scope_spec(scope: &str) -> Result<(&'static str, &'static str)> {
    Ok(match scope.trim() {
        "메일" | "mail" => ("0", "메일"),
        "일정" | "schedule" => ("3", "일정"),
        "결재" | "전자결재" | "approval" => ("6", "전자결재"),
        "게시판" | "board" | "공지" => ("9", "게시판"),
        "파일" | "첨부" | "file" => ("10", "파일"),
        "자원" | "회의실" | "resource" => ("13", "자원"),
        other => {
            return Err(anyhow!(
                "알 수 없는 scope '{other}'. 사용 가능: 메일/결재/게시판/일정/자원/파일/전체"
            ))
        }
    })
}

/// `전체` 검색 시 훑을 범위. 실측에서 결과가 나온 6종만(나머지 boardType은 항상 빈 결과).
const ALL_SCOPES: [&str; 6] = ["메일", "결재", "게시판", "일정", "자원", "파일"];

/// 필드를 문자열로. ⛔ **`util::s`와 통합 금지 — 동작이 다르다.**
/// ⚠️ 일부 필드는 **다국어 객체**(`{kr,en,jp,cn}`)로 온다
/// (결재 `deptNm`/`userNm`/`formNm` 실측) → 그 경우 한국어 값을 꺼낸다.
fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Object(o)) => o.get("kr").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        _ => String::new(),
    }
}

fn snippet(t: &str, n: usize) -> String {
    let cleaned = t.split_whitespace().collect::<Vec<_>>().join(" ");
    cleaned.chars().take(n).collect()
}

/// 모듈별로 필드가 완전히 다르므로 공통 형태로 정규화한다.
/// **후속 조회에 필요한 ID를 반드시 포함**한다(메일 muid → read_mail,
/// 결재 docId+formId → read_approval, 게시판 artSeqNo → read_notice).
fn normalize(board_type: &str, label: &str, r: &Value) -> Value {
    let base = |title: String, date: String, who: String| {
        json!({ "module": label, "title": title, "date": date, "who": who })
    };
    let mut out = match board_type {
        "0" => {
            let mut v = base(s(r, "subject"), s(r, "rfc822date"), s(r, "fromAddrName"));
            v["muid"] = json!(s(r, "muid"));
            v["box"] = json!(s(r, "boxName"));
            v["from"] = json!(s(r, "fromAddrEmail"));
            v["snippet"] = json!(snippet(&s(r, "mailBody"), 160));
            v
        }
        "6" => {
            let mut v = base(s(r, "docTitle"), s(r, "rep_dt"), s(r, "userNm"));
            v["docId"] = json!(s(r, "docId"));
            v["formId"] = json!(s(r, "formId"));
            v["form"] = json!(s(r, "formNm"));
            v["dept"] = json!(s(r, "deptNm"));
            v["status"] = json!(s(r, "docSts"));
            v["snippet"] = json!(snippet(&s(r, "docContents"), 160));
            v
        }
        "9" => {
            let mut v = base(s(r, "artTitle"), s(r, "writeDate"), s(r, "mbrNick"));
            v["artSeqNo"] = json!(s(r, "artSeqNo"));
            v["board"] = json!(s(r, "boardName"));
            v["snippet"] = json!(snippet(&s(r, "artContent"), 160));
            v
        }
        "3" => {
            let mut v = base(s(r, "schTitle"), s(r, "startDate"), String::new());
            v["schSeq"] = json!(s(r, "schSeq"));
            v["end"] = json!(s(r, "endDate"));
            v["resName"] = json!(s(r, "resName"));
            v
        }
        "13" => {
            let mut v = base(s(r, "reqText"), s(r, "startDate"), String::new());
            v["resSeq"] = json!(s(r, "resSeq"));
            v["resName"] = json!(s(r, "resName"));
            v["end"] = json!(s(r, "endDate"));
            v
        }
        "10" => {
            let mut v = base(s(r, "fileName"), s(r, "createDate"), s(r, "empName"));
            v["fileId"] = json!(s(r, "fileId"));
            v["ext"] = json!(s(r, "fileExtsn"));
            v["size"] = json!(s(r, "fileSize"));
            v["parentTitle"] = json!(s(r, "title"));
            v
        }
        _ => base(String::new(), String::new(), String::new()),
    };
    // 작성자 empSeq는 모든 모듈 공통 — find_person 등으로 이어붙일 수 있게 남긴다.
    if let Some(e) = r.get("empSeq") {
        out["empSeq"] = e.clone();
    }
    out
}

/// 한 모듈 검색. `limit`는 페이지 크기(서버가 페이지 단위로 준다).
async fn search_one(
    c: &GwClient,
    query: &str,
    scope: &str,
    limit: i64,
    from: &str,
    to: &str,
) -> Result<Value> {
    let (bt, label) = scope_spec(scope)?;
    let body = json!({
        "header": {},
        "body": {
            "tsearchKeyword": query,
            "tsearchSubKeyword": "",
            "boardType": bt,
            "fromDate": from, "toDate": to,
            // ⚠️ dateDiv는 **반드시 빈 문자열**. "A"/"R"/"W" 등 아무 값이나 넣으면 서버가
            // 날짜 필터를 통째로 무시한다(에러 없이 전체 결과 반환) — 2026-08-04 실측.
            "dateDiv": "",
            "detailSearchYn": "N", "selectDiv": "S", "orderDiv": "B", "syncTime": "N",
            "pageIndex": 1, "hrSearchYn": "N", "hrEmpSeq": "",
            "pageSize": limit, "webMobileDiv": "W"
        }
    });
    let d = c.call("/gw/APIHandler/gw018A02", &body).await?;
    let items: Vec<Value> = d
        .get("resultgrid")
        .and_then(|g| g.as_array())
        .map(|a| a.iter().map(|r| normalize(bt, label, r)).collect())
        .unwrap_or_default();
    Ok(json!({
        "module": label,
        "totalCount": d.get("totalcount").cloned().unwrap_or(Value::Null),
        "returned": items.len(),
        "items": items
    }))
}

/// 통합검색. `scope`가 "전체"면 6개 모듈을 모두 훑는다.
pub async fn search(
    c: &GwClient,
    query: &str,
    scope: &str,
    limit: i64,
    from: &str,
    to: &str,
) -> Result<Value> {
    if query.trim().is_empty() {
        return Err(anyhow!("검색어가 비어 있습니다"));
    }
    let limit = limit.clamp(1, 50);
    let scope = if scope.trim().is_empty() { "전체" } else { scope.trim() };

    let groups: Vec<Value> = if scope == "전체" || scope == "all" {
        let mut v = Vec::new();
        for sc in ALL_SCOPES {
            // 한 모듈이 실패해도 나머지는 살린다.
            if let Ok(g) = search_one(c, query, sc, limit, from, to).await {
                v.push(g);
            }
        }
        v
    } else {
        vec![search_one(c, query, scope, limit, from, to).await?]
    };

    let total: i64 = groups
        .iter()
        .filter_map(|g| g.get("totalCount").and_then(|t| t.as_i64()))
        .sum();
    Ok(json!({
        "kind": "search",
        "query": query,
        "scope": scope,
        "period": if from.is_empty() && to.is_empty() { Value::Null } else { json!(format!("{from}~{to}")) },
        "totalAcrossModules": total,
        "results": groups
    }))
}

#[cfg(test)]
// 테스트 이름에 아마란스 실제 필드명(empSeq·delYn·boardType…)을 그대로 적는다 —
// 무엇을 검증하는지 이름만 보고 알기 위해서다. 소문자로 풀면 실재하지 않는 이름이 되므로
// 이름을 바꾸는 대신 lint를 끈다. (한글은 대소문자가 없어 경고 대상이 아니다.)
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn scope_spec은_별칭을_boardType으로_바꾼다() {
        assert_eq!(scope_spec("메일").unwrap(), ("0", "메일"));
        assert_eq!(scope_spec("mail").unwrap(), ("0", "메일"));
        assert_eq!(scope_spec(" 전자결재 ").unwrap(), ("6", "전자결재"));
        assert_eq!(scope_spec("공지").unwrap(), ("9", "게시판"));
        assert_eq!(scope_spec("회의실").unwrap(), ("13", "자원"));
        assert_eq!(scope_spec("파일").unwrap(), ("10", "파일"));
        assert!(scope_spec("몰라").is_err());
        assert!(scope_spec("전체").is_err(), "'전체'는 상위에서 ALL_SCOPES로 펼친다");
    }

    #[test]
    fn all_scopes는_전부_해석된다() {
        for sc in ALL_SCOPES {
            assert!(scope_spec(sc).is_ok(), "ALL_SCOPES의 '{sc}'가 해석 불가");
        }
    }

    /// ⚠️ 이 `s`는 다른 모듈의 동명 함수와 **다르다** — 다국어 객체에서 `kr`을 꺼낸다.
    /// (결재 deptNm/userNm/formNm이 `{kr,en,jp,cn}`으로 오는 실측 대응.) 통합하면 안 되는 이유.
    #[test]
    fn s는_다국어_객체에서_한국어를_꺼낸다() {
        let v = json!({
            "plain": "문자열",
            "num": 42,
            "multi": {"kr": "인사총무팀", "en": "HR"},
            "empty_multi": {"en": "HR"},
            "arr": [1, 2]
        });
        assert_eq!(s(&v, "plain"), "문자열");
        assert_eq!(s(&v, "num"), "42");
        assert_eq!(s(&v, "multi"), "인사총무팀");
        assert_eq!(s(&v, "empty_multi"), ""); // kr 없으면 빈 문자열
        assert_eq!(s(&v, "arr"), "");
        assert_eq!(s(&v, "없는키"), "");
    }

    #[test]
    fn snippet은_공백을_눌러_n자로_자른다() {
        assert_eq!(snippet("  가나  다라\n마바  ", 100), "가나 다라 마바");
        assert_eq!(snippet("abcdefghij", 3), "abc");
        assert_eq!(snippet("한글도 문자수 기준", 3), "한글도"); // 바이트 아님
        assert_eq!(snippet("", 10), "");
    }

    /// 정규화의 핵심 계약: **후속 조회 ID가 살아남아야 한다**(muid/docId+formId/artSeqNo).
    #[test]
    fn normalize는_모듈별_후속조회_ID를_보존한다() {
        let mail = normalize("0", "메일", &json!({
            "subject": "제목", "muid": "123", "boxName": "INBOX",
            "fromAddrName": "이재학", "fromAddrEmail": "a@b.c", "mailBody": "본문 내용"
        }));
        assert_eq!(mail["module"], "메일");
        assert_eq!(mail["muid"], "123");
        assert_eq!(mail["title"], "제목");

        let appr = normalize("6", "전자결재", &json!({
            "docTitle": "외근신청", "docId": "141640", "formId": "41",
            "deptNm": {"kr": "네이티브 플랫폼팀"}, "userNm": {"kr": "이재학"}
        }));
        assert_eq!(appr["docId"], "141640");
        assert_eq!(appr["formId"], "41");
        assert_eq!(appr["dept"], "네이티브 플랫폼팀"); // 다국어 객체 해석
        assert_eq!(appr["who"], "이재학");

        let board = normalize("9", "게시판", &json!({ "artTitle": "공지", "artSeqNo": "3009" }));
        assert_eq!(board["artSeqNo"], "3009");

        // 모르는 boardType이어도 패닉 없이 빈 껍데기를 준다.
        let unknown = normalize("99", "미상", &json!({}));
        assert_eq!(unknown["module"], "미상");
        assert_eq!(unknown["title"], "");
    }

    #[test]
    fn normalize는_empSeq를_모듈_공통으로_붙인다() {
        let v = normalize("9", "게시판", &json!({ "artTitle": "공지", "empSeq": "3166" }));
        assert_eq!(v["empSeq"], "3166");
    }
}
