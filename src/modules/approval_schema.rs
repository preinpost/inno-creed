//! 결재라인 스키마(직책 기반) 조회 — 번들 기본값 + `~/.config` override 논리 병합.
//! 데이터: `src/data/approval_line_schema.json` (version = 위임전결 PDF 날짜). 근거: `02-delegation-matrix.md`.
//! ⚠️ 스키마는 **직책만** 담는다 — 사람은 org_chart 로 해석(후보)하고 **상신 전 사람 확인 필수**.
//!
//! override 파일: `$XDG_CONFIG_HOME/inno-creed/approval_line.json` (없으면 `~/.config/inno-creed/...`).
//! 형식: `{ "overrides": { "<양식명>": { "overridden_at": "YYYY-MM-DD", "branches": [...] } } }`.
//! 병합 규칙(논리, 파일 미변경): 폼별로 `overridden_at >= 번들.version` 이면 override, 아니면 번들(=새 PDF 가 낡은 override 를 이김).

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

const BUNDLED: &str = include_str!("../data/approval_line_schema.json");

fn bundled() -> Value {
    serde_json::from_str(BUNDLED).expect("번들 결재라인 스키마 JSON 파싱 실패")
}

/// override 파일 경로: XDG_CONFIG_HOME 우선, 없으면 ~/.config.
fn override_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("inno-creed").join("approval_line.json"))
}

/// override 파일의 `overrides` 오브젝트(없으면 Null).
fn load_overrides() -> Value {
    override_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("overrides").cloned())
        .unwrap_or(Value::Null)
}

/// 특정 양식의 유효 스키마(번들 vs override 를 폼별 날짜 date-max 로 논리 병합).
pub fn get_schema(doc_type: &str) -> Result<Value> {
    let b = bundled();
    let bundled_version = b.get("version").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let forms = b
        .get("forms")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("번들 스키마에 forms 없음"))?;

    let (name, form) = find_form(forms, doc_type).ok_or_else(|| {
        anyhow!("'{doc_type}' 에 해당하는 결재라인 스키마 없음. list_approval_line_schemas 로 목록 확인")
    })?;

    let ov = load_overrides();
    let ov_form = ov.get(&name);
    let ov_date = ov_form
        .and_then(|f| f.get("overridden_at"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // YYYY-MM-DD 는 사전식 비교 = 날짜 비교. override 날짜가 번들 version 이상이면 override 사용.
    let use_override = !ov_date.is_empty() && ov_date >= bundled_version.as_str();
    let (source_of, version, effective) = if use_override {
        ("user", ov_date.to_string(), ov_form.cloned().unwrap_or(Value::Null))
    } else {
        ("bundled", bundled_version.clone(), form.clone())
    };

    Ok(json!({
        "docType": name,
        "sourceOf": source_of,
        "version": version,
        "source": if source_of == "user" { "user override".into() } else { b.get("source").cloned().unwrap_or(Value::Null) },
        "positions": b.get("positions").cloned().unwrap_or(Value::Null),
        "schema": effective,
        "note": "직책 스키마 — 사람은 org_chart 로 해석(후보). 상신 전 사람 확인 필수."
    }))
}

/// 수록된 스키마 목록(양식명/form_id/alias/버전/출처).
pub fn list_schemas() -> Result<Value> {
    let b = bundled();
    let bundled_version = b.get("version").and_then(|v| v.as_str()).unwrap_or("");
    let ov = load_overrides();
    let forms = b
        .get("forms")
        .and_then(|v| v.as_object())
        .ok_or_else(|| anyhow!("번들 스키마에 forms 없음"))?;

    let list: Vec<Value> = forms
        .iter()
        .map(|(name, v)| {
            let ov_date = ov
                .get(name)
                .and_then(|f| f.get("overridden_at"))
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let source_of = if !ov_date.is_empty() && ov_date >= bundled_version { "user" } else { "bundled" };
            json!({
                "docType": name,
                "formId": v.get("form_id").cloned().unwrap_or(Value::Null),
                "aliases": v.get("aliases").cloned().unwrap_or(Value::Null),
                "version": if source_of == "user" { ov_date } else { bundled_version },
                "sourceOf": source_of
            })
        })
        .collect();

    Ok(json!({
        "bundledVersion": bundled_version,
        "source": b.get("source").cloned().unwrap_or(Value::Null),
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
        if let Some(aliases) = v.get("aliases").and_then(|a| a.as_array())
            && aliases.iter().any(|a| a.as_str() == Some(q))
        {
            return Some((name.clone(), v));
        }
        if let Some(fid) = v.get("form_id").and_then(|f| f.as_i64())
            && q == fid.to_string()
        {
            return Some((name.clone(), v));
        }
    }
    None
}
