//! 전자결재(EAPPROVAL) 모듈 — `/eap/*`. 읽기 전용(함별 목록·문서 상세·미처리 카운트).
//! 인증은 헤더 서명만으로 완결. 쓰기(상신/승인/반려)는 실 결재 발생이라 미구현.
//! 실측: `.claude-workspace/approval-analysis/07-eapproval-api-capture.md`.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::util::{days_to_ymd, digits_only, fmt_ymd, json_str};

/// 함(box) → (목록 API, eaBoxId, menuNo, periodPicker, 응답 list 경로).
/// 수신계열(미결/기결/수신참조/시행)은 eap105A04(resultData.map.list), 상신함은 eap107A04(resultData.list.list).
fn box_spec(b: &str) -> Result<(&'static str, &'static str, &'static str, &'static str, &'static str)> {
    Ok(match b {
        "pending" => ("eap105A04", "1000900", "1001000", "ARRIVED_DT", "map"),
        "approved" => ("eap105A04", "1000900", "1001100", "ACTION_TIME", "map"),
        "approved_ongoing" => ("eap105A04", "1000900", "1001110", "ACTION_TIME", "map"),
        "approved_done" => ("eap105A04", "1000900", "1001120", "ACTION_TIME", "map"),
        "reference" => ("eap105A04", "1000900", "1001200", "REP_DT", "map"),
        "enforcement" => ("eap105A04", "1000900", "1001400", "REP_DT", "map"),
        "sent" => ("eap107A04", "1000300", "1000400", "REP_DT", "list"),
        // 임시보관(결재작성 중 저장 + 상신취소로 복귀한 문서). eap107A06, menuNo 1000500.
        // ※ 여기 쌓인 문서가 신규 상신을 막지는 않는다(07 §10.6에서 반증).
        "draft" => ("eap107A06", "1000300", "1000500", "REP_DT", "list"),
        _ => return Err(anyhow!(
            "알 수 없는 함 '{b}'. 사용 가능: pending(미결)/approved(기결)/approved_ongoing(기결진행)/approved_done(기결종결)/reference(수신참조)/enforcement(시행)/sent(상신)/draft(임시보관)"
        )),
    })
}

/// 함별 문서 목록 — eap105A04(수신계열) / eap107A04(상신함).
/// `from`/`to`는 등록·도착일 범위(YYYY-MM-DD 또는 YYYYMMDD; 빈값이면 서버 기본 최근 3개월).
#[allow(clippy::too_many_arguments)]
pub async fn list_approvals(
    c: &GwClient,
    box_name: &str,
    page: i64,
    page_size: i64,
    from: &str,
    to: &str,
) -> Result<Value> {
    let (api, ea_box_id, menu_no, period, list_path) = box_spec(box_name)?;
    // 빈 날짜면 서버 기본이 좁아 문서를 놓친다 → UI처럼 최근 ~3개월로 채운다.
    let (def_from, def_to) = default_range();
    let sfr = if from.trim().is_empty() { def_from } else { digits_only(from) };
    let sto = if to.trim().is_empty() { def_to } else { digits_only(to) };
    let body = json!({
        "fDocSts": [], "page": page.to_string(), "pageSize": page_size.to_string(),
        "eaBoxId": ea_box_id, "nMenuID": menu_no, "menuNo": menu_no, "upperMenuNo": ea_box_id,
        "sfrDt": sfr, "stoDt": sto,
        "sFormId": ["0"], "periodPicker": period, "sortField": period, "sortType": "DESC",
        "docContentsData": {}, "item": {},
        "useElasticSearch": true, "useElasticSearch_new": true,
        "pageCode": ""
    });
    let data = c.call(&format!("/eap/{api}"), &body).await?;

    // 응답 봉투: 수신계열 resultData.map.{list,totalCount}, 상신함 resultData.list.{list,totalCount}.
    let container = data
        .get(list_path)
        .ok_or_else(|| anyhow!("{api} 응답에 {list_path} 없음"))?;
    let total = container.get("totalCount").cloned().unwrap_or(Value::Null);
    let arr = container.get("list").and_then(|v| v.as_array()).cloned().unwrap_or_default();

    let s = |v: &Value, k: &str| json_str(v.get(k));
    let docs: Vec<Value> = arr
        .iter()
        .map(|d| {
            // 상신/수신함은 FORM_NM, 임시보관(draft)은 DRAFT_FORM_NM에 양식명이 있다.
            let form = {
                let f = s(d, "FORM_NM");
                if f.is_empty() { s(d, "DRAFT_FORM_NM") } else { f }
            };
            json!({
                "docId": s(d, "DOC_ID"),
                "docNo": s(d, "DOC_NO"),
                "title": s(d, "DOC_TITLE"),
                "form": form,
                "formId": s(d, "FORM_ID"),
                "drafter": s(d, "USER_NM"),
                "dept": s(d, "DEPT_NM"),
                "status": s(d, "DOC_STSNM"),
                "currentApprover": s(d, "lineUserName"),
                "readYn": s(d, "READYN"),
                "repDt": s(d, "REP_DT"),
                "arrivedDt": s(d, "ARRIVED_DT"),
                "endDt": s(d, "END_DT"),
                "commentCount": s(d, "COMMENT_COUNT"),
                "fileCount": s(d, "FILE_CNT")
            })
        })
        .collect();
    Ok(json!({ "box": box_name, "totalCount": total, "documents": docs }))
}

/// 문서 상세 — eap111A04. `doc_id`/`form_id`는 목록의 docId/formId.
/// 본문 평문(contentsWord)·헤더·결재선·첨부수 반환. **열람 부작용 없음**(setReadYn:"N").
pub async fn read_approval(c: &GwClient, doc_id: &str, form_id: &str) -> Result<Value> {
    let body = json!({
        "doc_id": doc_id, "form_id": form_id, "bindType": "V", "p_doc_id": 0,
        "doc_auth": "0", "spDocId": "", "setReadYn": "N", "commentReqYn": "N",
        "pageCode": "UBA1100", "docToken": ""
    });
    let d = c.call("/eap/eap111A04", &body).await?;

    let s = |k: &str| json_str(d.get(k));
    // 결재선 처리내역: user_info[] (처리시각/처리여부). 이름 필드는 미노출이라 코드 위주로 요약.
    let line: Vec<Value> = d
        .get("user_info")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|u| {
                    json!({
                        "userId": json_str(u.get("user_id")),
                        "receiveDiv": json_str(u.get("receive_div")),
                        "procYn": json_str(u.get("proc_yn")),
                        "procTime": json_str(u.get("proc_time"))
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let body_text = {
        let w = s("contentsWord");
        if w.trim().is_empty() { html_to_text(&s("docContents")) } else { w }
    };
    Ok(json!({
        "docId": doc_id,
        "docNo": s("docNo"),
        "title": s("docTitle"),
        "form": s("formName"),
        "status": s("docStsName"),
        "drafter": s("empName"),
        "dept": s("deptName"),
        "repDt": s("repDt"),
        "attachCount": s("attachCnt"),
        "currentApprover": s("lineName"),
        "content": collapse_ws(&body_text),
        "approvalLine": line
    }))
}

/// 함별 미처리 건수 — `/eap/api/getMenuCountInfo`. companyInfo 필요(ensure_session 선행).
/// menuNo→count 맵을 사람이 읽기 쉬운 라벨로 변환.
pub async fn approval_counts(c: &GwClient) -> Result<Value> {
    let body = json!({
        "deptSeq": c.dept_seq(), "userSe": "USER|AT", "compSeq": c.comp_seq(),
        "bizSeq": c.comp_seq(), "empSeq": c.emp_seq(), "groupSeq": c.group_seq(),
        "menuType": "", "pageCode": "EapSide"
    });
    let d = c.call("/eap/api/getMenuCountInfo", &body).await?;
    let label = |mn: &str| match mn {
        "1001000" => "pending(미결)",
        "1001100" => "approved(기결)",
        "1001110" => "approved_ongoing(기결진행)",
        "1001120" => "approved_done(기결종결)",
        "1001200" => "reference(수신참조)",
        "1001400" => "enforcement(시행)",
        "1000400" => "sent(상신)",
        _ => "",
    };
    let mut counts = serde_json::Map::new();
    if let Some(obj) = d.as_object() {
        for (mn, cnt) in obj {
            let key = label(mn);
            let name = if key.is_empty() { mn.clone() } else { key.to_string() };
            counts.insert(name, cnt.clone());
        }
    }
    Ok(Value::Object(counts))
}

/// 미결함 요약. `approval_counts`가 숫자만 주는 것에 대응 — 실제로 필요한
/// "무엇을 며칠째 붙들고 있는지"를 낸다. 대기일수는 `ARRIVED_DT`(도착일) 기준.
pub async fn pending_digest(c: &GwClient, page_size: i64) -> Result<Value> {
    let listed = list_approvals(c, "pending", 1, page_size, "", "").await?;
    let docs = listed
        .get("documents")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let today = {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| ((d.as_secs() as i64) + 9 * 3600) / 86400)
            .unwrap_or(0)
    };
    // YYYYMMDD → epoch days (days_to_ymd의 역함수, Howard Hinnant days_from_civil).
    let to_days = |ymd: &str| -> Option<i64> {
        if ymd.len() < 8 {
            return None;
        }
        let y: i64 = ymd[0..4].parse().ok()?;
        let m: i64 = ymd[4..6].parse().ok()?;
        let d: i64 = ymd[6..8].parse().ok()?;
        let y2 = if m <= 2 { y - 1 } else { y };
        let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
        let yoe = y2 - era * 400;
        let mp = if m > 2 { m - 3 } else { m + 9 };
        let doy = (153 * mp + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
        Some(era * 146097 + doe - 719468)
    };

    let mut out: Vec<Value> = docs
        .iter()
        .map(|d| {
            let arrived = d.get("arrivedDt").and_then(|v| v.as_str()).unwrap_or("");
            let digits: String = arrived.chars().filter(|c| c.is_ascii_digit()).collect();
            let waiting = to_days(&digits).map(|a| today - a);
            json!({
                "docId": d.get("docId"),
                "formId": d.get("formId"),
                "title": d.get("title"),
                "form": d.get("form"),
                "drafter": d.get("drafter"),
                "dept": d.get("dept"),
                "arrivedDt": arrived,
                "waitingDays": waiting,
                "unread": d.get("readYn").and_then(|v| v.as_str()) == Some("N")
            })
        })
        .collect();
    // 오래 기다린 것부터
    out.sort_by_key(|d| -d.get("waitingDays").and_then(|v| v.as_i64()).unwrap_or(0));

    Ok(json!({
        "kind": "pendingDigest",
        "totalCount": listed.get("totalCount").cloned().unwrap_or(Value::Null),
        "count": out.len(),
        "oldestWaitingDays": out.first().and_then(|d| d.get("waitingDays").cloned()),
        "documents": out
    }))
}


/// 기본 조회 범위(최근 ~3개월) → (sfrDt, stoDt) YYYYMMDD. chrono 없이 SystemTime으로 계산.
fn default_range() -> (String, String) {
    let day = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86400) as i64)
        .unwrap_or(0);
    (fmt_ymd(days_to_ymd(day - 92)), fmt_ymd(days_to_ymd(day)))
}




/// HTML → 대략 평문(태그 제거·엔티티 디코드). 상세 본문이 contentsWord로 안 올 때 fallback.
/// ⛔ **`board::html_to_text`와 통합 금지 — 동작이 반대다.** 이쪽(결재)은 블록 구분 없이 태그를
/// 공백 하나로 바꿔 본문을 **한 줄로 눌러** 쓴다. 게시판·메일 쪽은 개행·탭으로 구조를 보존한다.
fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if in_tag => {}
            _ => out.push(ch),
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// 연속 공백/개행 축약. ⛔ **`board::collapse_ws`와 통합 금지** —
/// 이쪽은 **개행까지 전부 없애** 한 줄로 만들고, 저쪽은 빈 줄을 1개까지 보존한다.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 함 이름 → API/menuNo 매핑. 실측으로 확정된 값이라 바뀌면 다른 함을 조회하게 된다.
    /// 상신함(sent)·임시보관(draft)만 **다른 API·다른 응답 경로**를 쓴다는 점이 핵심.
    #[test]
    fn box_spec은_8개_함을_매핑한다() {
        assert_eq!(box_spec("pending").unwrap(), ("eap105A04", "1000900", "1001000", "ARRIVED_DT", "map"));
        assert_eq!(box_spec("sent").unwrap(), ("eap107A04", "1000300", "1000400", "REP_DT", "list"));
        assert_eq!(box_spec("draft").unwrap(), ("eap107A06", "1000300", "1000500", "REP_DT", "list"));
        for b in ["approved", "approved_ongoing", "approved_done", "reference", "enforcement"] {
            let (api, _, menu, _, path) = box_spec(b).unwrap();
            assert_eq!(api, "eap105A04", "{b}는 수신계열 API여야 한다");
            assert_eq!(path, "map", "{b}는 resultData.map.list 경로여야 한다");
            assert!(menu.starts_with("1001"), "{b}의 menuNo가 수신계열 대역이 아니다");
        }
        assert!(box_spec("없는함").is_err());
        assert!(box_spec("").is_err());
    }

    /// ⚠️ 결재의 `html_to_text`/`collapse_ws`는 `board` 의 동명 함수와 **의도적으로 다르다** —
    /// 여기서는 블록 구분 없이 태그를 공백으로 바꾸고 개행을 전부 없앤다(본문을 한 줄로).
    /// 근거: `.claude-workspace/todo/refactor-structure/05-shared-util-extraction.md` (B)절.
    #[test]
    fn html_to_text는_블록구분_없이_공백으로만_바꾼다() {
        assert_eq!(html_to_text("<p>가</p><p>나</p>"), " 가  나 ");
        assert!(!html_to_text("가<br>나").contains('\n'), "결재 본문은 개행을 만들지 않는다");
        assert_eq!(html_to_text("&lt;a&gt;&nbsp;b"), "<a> b");
    }

    #[test]
    fn collapse_ws는_개행까지_전부_없앤다() {
        assert_eq!(collapse_ws("가\n나  다"), "가 나 다");
        assert!(!collapse_ws("가\n\n나").contains('\n'), "결재는 한 줄로 눌러야 한다");
    }

    /// 기본 조회 범위는 "오늘로부터 92일 전 ~ 오늘". 오늘에 의존하므로 불변식만 검증한다.
    #[test]
    fn default_range는_92일_구간이다() {
        let (from, to) = default_range();
        assert_eq!(from.len(), 8);
        assert_eq!(to.len(), 8);
        assert!(from < to, "시작이 종료보다 앞서야 한다");
        // 같은 함수의 날짜 계산으로 역산해 정확히 92일 차이인지 확인
        let day = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.as_secs() / 86400) as i64)
            .unwrap_or(0);
        assert_eq!(to, fmt_ymd(days_to_ymd(day)));
        assert_eq!(from, fmt_ymd(days_to_ymd(day - 92)));
    }
}
