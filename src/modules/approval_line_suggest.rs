//! 결재선 **후보** 제안 — 직책 스키마(`approval_schema`) + 조직도(`org`)를 엮어
//! "이 문서를 내가 기안하면 결재선은 대략 이렇게 된다"를 한 번에 보여준다.
//!
//! ⛔ **이 도구는 결재선을 확정하지 않는다.** 반환값은 전부 *후보*이며, 그대로 등록/상신하면 안 된다.
//! 이유(실측 근거는 `07-eapproval-api-capture.md` §8.5·§8.7):
//!  - 직책→사람 해석은 공석·겸직·대행·직책 라벨 drift(규칙의 "사업부장" ↔ 조직의 "센터장")에 취약하다.
//!  - 한 직책에 복수 인원이 잡히거나(동일 duty 2명) 아무도 안 잡힐 수 있다(상위 부서에 장이 공석).
//!  - 위임전결 기준 자체가 개정된다(260801 개정 실측) — 번들 스키마가 최신이라는 보장이 없다.
//!
//! 그래서 **모든 응답에 `verificationRequired: true` + 경고문**을 싣고, 사람이 확인한 뒤에야
//! `save_approval_line` → `submit_approval`로 넘어가도록 한다.

use std::collections::HashMap;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::modules::{approval_schema, org};

/// 조직 직책(dutyName) → 스키마의 기안자 구간(grade). 스키마는 3구간(팀원/팀장/센터장이상)이다.
/// ⚠️ 문자열 매칭이라 새 직책명이 생기면 못 잡는다 — 못 잡으면 grade를 단정하지 않고 후보 전체를 준다.
fn grade_of(duty: &str) -> Option<&'static str> {
    match duty {
        "" => None,
        "팀원" => Some("팀원"),
        "팀장" => Some("팀장"),
        d if ["센터장", "실장", "사업부장", "본부장", "부문장", "대표이사"]
            .iter()
            .any(|x| d.contains(x)) =>
        {
            Some("사업부장/실장/센터장이상")
        }
        _ => None,
    }
}

/// 부서 사원 목록을 부서별로 1회만 조회하는 캐시 헬퍼.
async fn members_of(
    c: &GwClient,
    cache: &mut HashMap<String, Vec<Value>>,
    dept_id: &str,
) -> Vec<Value> {
    if !cache.contains_key(dept_id) {
        let list = org::dept_members(c, dept_id)
            .await
            .ok()
            .and_then(|v| v.get("members").and_then(|m| m.as_array()).cloned())
            .unwrap_or_default();
        cache.insert(dept_id.to_string(), list);
    }
    cache.get(dept_id).cloned().unwrap_or_default()
}

/// duty 문자열이 스키마 정의(`"센터장|실장|사업부장"` 같은 OR 표기)에 걸리는지.
fn duty_matches(spec: &str, duty: &str) -> bool {
    !duty.is_empty() && spec.split('|').any(|want| duty.contains(want.trim()))
}

/// 사람 후보 1건을 결재선 표시용으로 축약(그대로 save_approval_line에 넣을 수 있게 empSeq 포함).
fn candidate(m: &Value) -> Value {
    json!({
        "empSeq": m.get("empSeq").cloned().unwrap_or(Value::Null),
        "name": m.get("name").cloned().unwrap_or(Value::Null),
        "duty": m.get("duty").cloned().unwrap_or(Value::Null),
        "position": m.get("position").cloned().unwrap_or(Value::Null),
        "deptId": m.get("deptId").cloned().unwrap_or(Value::Null),
        "deptName": m.get("deptName").cloned().unwrap_or(Value::Null)
    })
}

/// 결재선 후보 제안. `doc_type`=양식명/form_id, `trip`=출장의 국내/해외(다른 양식은 무시).
pub async fn suggest_line(c: &GwClient, doc_type: &str, trip: &str) -> Result<Value> {
    let schema = approval_schema::get_schema(doc_type)?;
    let positions = schema.get("positions").cloned().unwrap_or(Value::Null);
    let branches = schema
        .get("schema")
        .and_then(|s| s.get("branches"))
        .and_then(|b| b.as_array())
        .cloned()
        .unwrap_or_default();

    // ── 기안자(본인) 구간 판정 ─────────────────────────────────────────────
    let prof = org::my_profile(c).await;
    let my_duty = prof.get("duty").and_then(|v| v.as_str()).unwrap_or("");
    let my_dept = prof.get("deptId").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let my_grade = grade_of(my_duty);

    let mut warnings: Vec<String> = Vec::new();
    if my_grade.is_none() {
        warnings.push(format!(
            "기안자 직책('{my_duty}')으로 결재선 구간(팀원/팀장/센터장이상)을 판정하지 못했습니다 — 해당 branch를 사람이 직접 고르세요."
        ));
    }

    // 출장은 trip(국내/해외)로도 갈린다. 지정 안 했는데 스키마가 요구하면 경고.
    let needs_trip = branches.iter().any(|b| b.get("when").and_then(|w| w.get("trip")).is_some());
    let trip = trip.trim();
    if needs_trip && trip.is_empty() {
        warnings.push(
            "이 양식은 국내/해외에 따라 결재선이 다릅니다 — trip 인자를 지정하지 않아 국내/해외 branch를 모두 반환합니다.".into(),
        );
    }

    // ── 해당 branch 선택(못 고르면 전체를 후보로) ──────────────────────────
    let selected: Vec<&Value> = branches
        .iter()
        .filter(|b| {
            let w = b.get("when").cloned().unwrap_or(Value::Null);
            let g_ok = match my_grade {
                Some(g) => w.get("grade").and_then(|v| v.as_str()) == Some(g),
                None => true,
            };
            let t_ok = match w.get("trip").and_then(|v| v.as_str()) {
                Some(t) => trip.is_empty() || t == trip,
                None => true,
            };
            g_ok && t_ok
        })
        .collect();
    if selected.is_empty() {
        return Err(anyhow!(
            "기안자 구간(grade={my_grade:?})·trip('{trip}')에 맞는 결재선 branch가 스키마에 없습니다 — get_approval_line_schema로 직접 확인하세요."
        ));
    }

    // ── 부서 트리(상대 직책 L_* 해석용) 1콜 ────────────────────────────────
    let tree = org::dept_tree(c, "0").await?;
    let depts = tree.get("depts").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let dept_by_id = |id: &str| depts.iter().find(|d| d.get("deptId").and_then(|v| v.as_str()) == Some(id)).cloned();
    // 기안자 부서의 조상 dept_id(가까운 상위부터). path 예: "1000|1000|2986|2987|2989|2993|"
    let ancestors: Vec<String> = dept_by_id(&my_dept)
        .and_then(|d| d.get("path").and_then(|v| v.as_str()).map(String::from))
        .map(|p| {
            let mut ids: Vec<String> = p.split('|').filter(|s| !s.is_empty()).map(String::from).collect();
            ids.retain(|id| *id != my_dept);
            ids.reverse(); // 가까운 상위 → 먼 상위
            ids
        })
        .unwrap_or_default();

    let mut cache: HashMap<String, Vec<Value>> = HashMap::new();
    let mut out_branches: Vec<Value> = Vec::new();

    for b in selected {
        let line = b.get("line").and_then(|l| l.as_array()).cloned().unwrap_or_default();
        let mut steps: Vec<Value> = Vec::new();

        for (i, step) in line.iter().enumerate() {
            let act = step.get("act").and_then(|v| v.as_str()).unwrap_or("결재");
            let pos = step.get("pos").and_then(|v| v.as_str()).unwrap_or("");
            let final_step = step.get("final").and_then(|v| v.as_bool()).unwrap_or(false);
            let def = positions.get(pos).cloned().unwrap_or(Value::Null);
            let kind = def.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let want_duty = def.get("duty").and_then(|v| v.as_str()).unwrap_or("");

            // relative(L_*): 기안자 부서에서 상위로 올라가며 해당 duty 보유자를 찾는다.
            // fixed: positions[].dept 이름으로 부서를 찾고 그 부서원에서 duty로 거른다.
            let mut hits: Vec<Value> = Vec::new();
            let mut resolved_in = String::new();
            if kind == "relative" {
                // 기안자 본인 부서부터(팀장 기안 시 L_팀장 = 자기 팀의 팀장) 위로.
                let mut chain = vec![my_dept.clone()];
                chain.extend(ancestors.iter().cloned());
                for dept_id in chain {
                    if dept_id.is_empty() {
                        continue;
                    }
                    let found: Vec<Value> = members_of(c, &mut cache, &dept_id)
                        .await
                        .iter()
                        .filter(|m| duty_matches(want_duty, m.get("duty").and_then(|v| v.as_str()).unwrap_or("")))
                        .map(candidate)
                        .collect();
                    if !found.is_empty() {
                        resolved_in = dept_by_id(&dept_id)
                            .and_then(|d| d.get("name").and_then(|v| v.as_str()).map(String::from))
                            .unwrap_or(dept_id);
                        hits = found;
                        break;
                    }
                }
            } else {
                let want_dept = def.get("dept").and_then(|v| v.as_str()).unwrap_or("");
                let dept_ids: Vec<String> = depts
                    .iter()
                    .filter(|d| d.get("name").and_then(|v| v.as_str()).is_some_and(|n| n.contains(want_dept)))
                    .filter_map(|d| d.get("deptId").and_then(|v| v.as_str()).map(String::from))
                    .collect();
                for dept_id in dept_ids {
                    let found: Vec<Value> = members_of(c, &mut cache, &dept_id)
                        .await
                        .iter()
                        .filter(|m| {
                            want_duty.is_empty()
                                || duty_matches(want_duty, m.get("duty").and_then(|v| v.as_str()).unwrap_or(""))
                        })
                        .map(candidate)
                        .collect();
                    if !found.is_empty() {
                        resolved_in = want_dept.to_string();
                        hits = found;
                        break;
                    }
                }
            }

            let status = match hits.len() {
                0 => "미해결",
                1 => "후보1",
                _ => "후보다수",
            };
            if hits.is_empty() {
                warnings.push(format!("{}단계 '{pos}'({act})의 담당자를 찾지 못했습니다 — 사람이 직접 지정해야 합니다.", i + 1));
            } else if hits.len() > 1 {
                warnings.push(format!("{}단계 '{pos}'({act})에 후보가 {}명입니다 — 누구인지 사람이 골라야 합니다.", i + 1, hits.len()));
            }

            steps.push(json!({
                "seq": i + 1,
                "act": act,          // 결재(3000) / 합의(4000)
                "pos": pos,
                "posKind": kind,     // relative(기안자 라인 기준) / fixed(지정 부서)
                "wantDuty": want_duty,
                "resolvedIn": resolved_in,
                "final": final_step, // true = 이 단계가 전결(종결)
                "status": status,
                "candidates": hits
            }));
        }

        out_branches.push(json!({
            "when": b.get("when").cloned().unwrap_or(Value::Null),
            "steps": steps
        }));
    }

    Ok(json!({
        "kind": "approvalLineSuggestion",
        "verificationRequired": true,
        "docType": schema.get("docType").cloned().unwrap_or(Value::Null),
        "schemaVersion": schema.get("version").cloned().unwrap_or(Value::Null),
        "schemaSource": schema.get("source").cloned().unwrap_or(Value::Null),
        "drafter": {
            "empSeq": prof.get("empSeq").cloned().unwrap_or(Value::Null),
            "name": prof.get("name").cloned().unwrap_or(Value::Null),
            "deptName": prof.get("deptName").cloned().unwrap_or(Value::Null),
            "duty": prof.get("duty").cloned().unwrap_or(Value::Null),
            "gradeGuess": my_grade
        },
        "branches": out_branches,
        "warnings": warnings,
        "note": "⛔ 이건 확정 결재선이 아니라 후보다. 직책→사람 해석은 공석·겸직·대행·직책 라벨 차이에 취약하고 위임전결 기준도 개정된다(실측). \
                 그대로 save_approval_line에 넣지 말고, 반드시 사용자에게 이름을 보여주고 확인받은 뒤 등록할 것. \
                 status가 '미해결'/'후보다수'인 단계는 사람이 반드시 지정해야 한다. \
                 등록 시에는 결재(3000) 노드만 담는다 — 양식필수 합의자·수신참조·시행자는 상신 때 서버(eap110A03)가 자동 병합한다."
    }))
}
