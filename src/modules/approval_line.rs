//! 개인결재라인(config) CRUD — `/eap/eap102A0x`. **상신이 아니라 "상신 시 재사용할 결재선 config"**.
//! 생성→수정→삭제 왕복을 실호출로 검증하고 환경을 원복해 확정했다.
//! 인증은 헤더 서명만으로 완결(ensure_session 불필요).
//!
//! ⚠️ 결재자 객체(detailLine)는 `user_id/login_id/dept_id/co_id/duty_cd/grade_cd/act_id`가 필요한데,
//! 조직도(gw102A02)는 user_id/co_id/grade_cd를 주지 않는다. 따라서 **신규 라인의 결재자 객체는
//! read_line(eap102A05)으로 기존 라인에서 뽑아 재사용**하는 게 정석이다(임의 신규 인물은 조직-picker
//! API 미조사라 아직 못 채움). 서버가 자동 생성한 결재선은 규칙과 어긋나므로 그대로 신뢰하지 말 것.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::util::s;

/// 저장된 개인결재라인 목록 — eap102A02(body `{}`).
/// resultData[] : `{line_id, line_nm, form_id, form_nm, proc_id, proc_nm, line_kind, form_list}`.
/// 반환 객체는 **삭제 시 그대로 lineIdList에 넣어야 하므로**(행 객체 배열) 원본을 보존해 같이 돌려준다.
pub async fn list_lines(c: &GwClient) -> Result<Value> {
    let d = c.call("/eap/eap102A02", &json!({})).await?;
    let arr = d.as_array().cloned().unwrap_or_default();
    let lines: Vec<Value> = arr
        .iter()
        .map(|l| {
            json!({
                "lineId": s(l, "line_id"),
                "lineName": s(l, "line_nm"),
                "formId": s(l, "form_id"),
                "formName": s(l, "form_nm"),
                "procId": s(l, "proc_id"),
                "procName": s(l, "proc_nm"),
                "lineKind": s(l, "line_kind"),
                "_row": l  // ⚠️ delete_line에 이 객체를 그대로 넘겨야 함(id 배열 아님)
            })
        })
        .collect();
    Ok(json!({ "kind": "approvalLines", "count": lines.len(), "lines": lines }))
}

/// 라인 단건의 결재자 구성 — eap102A05(body `{lineId, line_id}`).
/// resultData.aaData[] 결재자 객체를 **원본 그대로** 반환한다(save_line detailLine 재사용용:
/// user_id/co_id/grade_cd/duty_cd/act_id 등 등록에 필요한 필드가 여기 다 있음).
pub async fn read_line(c: &GwClient, line_id: &str) -> Result<Value> {
    let d = c
        .call("/eap/eap102A05", &json!({ "lineId": line_id, "line_id": line_id }))
        .await?;
    let members = d
        .get("aaData")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    Ok(json!({
        "kind": "approvalLineMembers",
        "lineId": line_id,
        "count": members.len(),
        "members": members,  // 원본 결재자 객체(그대로 save_line detailLine에 재사용 가능)
        "note": "각 객체의 act_id 3000=결재/4000=합의. 이 객체들을 순서대로 detailLine에 넣고 save_line 호출."
    }))
}

/// 개인결재라인 생성/수정 — eap102A10.
/// `line_id`=0이면 신규, 기존 id면 수정. `detail_line`=결재자 객체 배열(각 read_line 결과 재사용).
/// ⚠️ **순서 필드 `doc_line_seq`/`doc_line_m_seq`(1-base)를 인덱스 순으로 자동 주입한다.**
/// (실측: line_seq만으론 순서가 저장 안 되고 doc_line_seq가 null이 돼 결재 순서가 뒤섞임 — 2026-08-03.)
/// ⚠️ 이건 config 저장일 뿐 **상신이 아니다**. 실제 상신은 별도(미구현).
pub async fn save_line(
    c: &GwClient,
    line_id: i64,
    line_nm: &str,
    form_id: i64,
    proc_id: &str,
    detail_line: Vec<Value>,
) -> Result<Value> {
    if detail_line.is_empty() {
        return Err(anyhow!("detail_line(결재자)이 비어있음"));
    }
    // 순서 필드 1-base 자동 주입(기존 값 있으면 덮어써 배열 순서 = 결재 순서 보장).
    let detail: Vec<Value> = detail_line
        .into_iter()
        .enumerate()
        .map(|(i, mut m)| {
            let seq = i as i64 + 1;
            if let Some(obj) = m.as_object_mut() {
                obj.insert("doc_line_seq".into(), json!(seq));
                obj.insert("doc_line_m_seq".into(), json!(seq));
                obj.insert("line_seq".into(), json!(seq));
            }
            m
        })
        .collect();
    let proc = if proc_id.trim().is_empty() { "1000" } else { proc_id.trim() };
    let body = json!({
        "line_id": line_id,
        "line_nm": line_nm,
        "line_kind": "10",
        "proc_id": proc,
        "detailLine": detail,
        "formList": [form_id],
        "form_id": form_id
    });
    let d = c.call("/eap/eap102A10", &body).await?;
    Ok(json!({
        "kind": "approvalLineSaved",
        "createdLineId": d.get("createdLineId").cloned().unwrap_or(Value::Null),
        "insertDResult": d.get("insertDResult").cloned().unwrap_or(Value::Null),
        "insertFormResult": d.get("insertFormResult").cloned().unwrap_or(Value::Null),
        "note": "config 저장 완료(상신 아님). list_lines로 재조회해 실제 반영 확인 권장."
    }))
}

/// 개인결재라인 삭제 — eap102A09(body `{lineIdList:[행 객체]}`).
/// ⚠️ **id 배열이 아니라 list_lines의 `_row` 행 객체 배열**을 넘겨야 함(id만 넣으면 resultCode 2165).
pub async fn delete_line(c: &GwClient, row: Value) -> Result<Value> {
    let d = c
        .call("/eap/eap102A09", &json!({ "lineIdList": [row] }))
        .await?;
    Ok(json!({
        "kind": "approvalLineDeleted",
        "resultCount": d.get("resultCount").cloned().unwrap_or(Value::Null),
        "note": "삭제 완료. list_lines로 사라졌는지 확인 권장."
    }))
}

