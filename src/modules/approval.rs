//! 전자결재(EAPPROVAL) 모듈 — `/eap/*`. 읽기 전용(함별 목록·문서 상세·미처리 카운트).
//! 인증은 헤더 서명만으로 완결. 쓰기(상신/승인/반려)는 실 결재 발생이라 미구현.
//! 실측: `.claude-workspace/approval-analysis/07-eapproval-api-capture.md`.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::client::GwClient;

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
        _ => return Err(anyhow!(
            "알 수 없는 함 '{b}'. 사용 가능: pending(미결)/approved(기결)/approved_ongoing(기결진행)/approved_done(기결종결)/reference(수신참조)/enforcement(시행)/sent(상신)"
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
            json!({
                "docId": s(d, "DOC_ID"),
                "docNo": s(d, "DOC_NO"),
                "title": s(d, "DOC_TITLE"),
                "form": s(d, "FORM_NM"),
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

/// 날짜 문자열에서 숫자만(YYYY-MM-DD → YYYYMMDD).
fn digits_only(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// 기본 조회 범위(최근 ~3개월) → (sfrDt, stoDt) YYYYMMDD. chrono 없이 SystemTime으로 계산.
fn default_range() -> (String, String) {
    let day = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| (d.as_secs() / 86400) as i64)
        .unwrap_or(0);
    (fmt_ymd(days_to_ymd(day - 92)), fmt_ymd(days_to_ymd(day)))
}

/// epoch days → (year, month, day). Howard Hinnant civil_from_days.
fn days_to_ymd(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn fmt_ymd((y, m, d): (i64, i64, i64)) -> String {
    format!("{y:04}{m:02}{d:02}")
}

/// 필드를 문자열로(number/string 혼용 흡수).
fn json_str(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// HTML → 대략 평문(태그 제거·엔티티 디코드). 상세 본문이 contentsWord로 안 올 때 fallback.
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

/// 연속 공백/개행 축약.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
