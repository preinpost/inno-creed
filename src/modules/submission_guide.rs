//! 양식별 신청 가이드(문서 본문에 채울 내용 + 상신 절차) 조회 — 번들 데이터.
//! 데이터: `src/data/submission_guides.json`. 근거: `03-document-guide.md`, `07-eapproval-api-capture.md` §8.
//! ⚠️ 결재라인(누가 결재)과 별개. 본문 입력·상신은 아직 MCP 자동화 불가라, 사람이 손으로 작성하도록 안내하는 용도.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

const BUNDLED: &str = include_str!("../data/submission_guides.json");

fn bundled() -> Value {
    serde_json::from_str(BUNDLED).expect("번들 신청 가이드 JSON 파싱 실패")
}

/// 특정 양식의 신청 가이드(본문 필수항목/절차/주의/결재라인 힌트).
pub fn get_guide(doc_type: &str) -> Result<Value> {
    let b = bundled();
    let forms = b
        .get("forms")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("번들 가이드에 forms 없음"))?;

    let (name, guide) = find_form(forms, doc_type).ok_or_else(|| {
        anyhow!("'{doc_type}' 신청 가이드 없음. list_submission_guides로 목록 확인")
    })?;

    Ok(json!({
        "docType": name,
        "version": b.get("version").cloned().unwrap_or(Value::Null),
        "source": b.get("source").cloned().unwrap_or(Value::Null),
        "mcpStatus": b.get("mcpStatus").cloned().unwrap_or(Value::Null),
        "guide": guide,
        "note": "draftHelp = submit_approval 기안 데이터 채우는 법(--help). fixed(고정코드 그대로)·fill(의미별 채울 필드)·hpApplicationExample/bindDataExample(복사 후 fill·identity만 교체). 결재라인은 get_approval_line_schema + save_approval_line로 준비. requiredBody/steps는 아마란스 웹 직접작성용 참고."
    }))
}

/// 수록된 신청 가이드 목록(양식명/form_id/alias).
pub fn list_guides() -> Result<Value> {
    let b = bundled();
    let forms = b
        .get("forms")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("번들 가이드에 forms 없음"))?;
    let list: Vec<Value> = forms
        .iter()
        .map(|(name, v)| {
            json!({
                "docType": name,
                "formId": v.get("form_id").cloned().unwrap_or(Value::Null),
                "aliases": v.get("aliases").cloned().unwrap_or(Value::Null)
            })
        })
        .collect();
    Ok(json!({
        "version": b.get("version").cloned().unwrap_or(Value::Null),
        "mcpStatus": b.get("mcpStatus").cloned().unwrap_or(Value::Null),
        "forms": list
    }))
}

/// 양식 이름(키) / alias / form_id 로 매칭.
fn find_form<'a>(
    forms: &'a serde_json::Map<String, Value>,
    q: &str,
) -> Option<(String, &'a Value)> {
    if let Some(v) = forms.get(q) {
        return Some((q.to_string(), v));
    }
    for (name, v) in forms {
        if let Some(aliases) = v.get("aliases").and_then(|a| a.as_array()) {
            if aliases.iter().any(|a| a.as_str() == Some(q)) {
                return Some((name.clone(), v));
            }
        }
        if let Some(fid) = v.get("form_id").and_then(|f| f.as_i64()) {
            if q == fid.to_string() {
                return Some((name.clone(), v));
            }
        }
    }
    None
}
