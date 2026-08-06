//! 결재선 **후보** 제안 — 직책 스키마(`approval_schema`) + 조직도(`org`)를 엮어
//! "이 문서를 내가 기안하면 결재선은 대략 이렇게 된다"를 한 번에 보여준다.
//!
//! ⛔ **이 도구는 결재선을 확정하지 않는다.** 반환값은 전부 *후보*이며, 그대로 등록/상신하면 안 된다.
//! 이유(전부 실제 트래픽 캡처로 확인):
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

/// 기안자 구간(`my_grade`)·출장구분(`trip`)에 맞는 branch를 고른다.
/// 폴백 두 개가 **의도적**이다: `my_grade`가 `None`이면 grade 조건을 통과시켜 전체를,
/// `trip`이 빈 문자열이면 국내/해외 branch를 모두 돌려준다(사람이 고르라고).
/// `trip`은 이미 trim된 값을 받는다.
fn select_branches<'a>(
    branches: &'a [Value],
    my_grade: Option<&str>,
    trip: &str,
) -> Result<Vec<&'a Value>> {
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
    Ok(selected)
}

/// 기안자 부서의 조상 dept_id를 **가까운 상위부터** 나열한다.
/// `path` 예: `"1000|1000|2986|2987|2989|2993|"` (빈 조각·중복이 실제로 들어온다).
fn ancestor_chain(path: &str, my_dept: &str) -> Vec<String> {
    let mut ids: Vec<String> = path.split('|').filter(|s| !s.is_empty()).map(String::from).collect();
    ids.retain(|id| id.as_str() != my_dept);
    ids.reverse(); // 가까운 상위 → 먼 상위
    ids
}

/// 한 단계의 후보 수 → 사용자에게 보여줄 상태 라벨.
fn step_status(hits: usize) -> &'static str {
    match hits {
        0 => "미해결",
        1 => "후보1",
        _ => "후보다수",
    }
}

/// 후보가 0명/2명 이상일 때 사람에게 확인을 요구하는 경고문(1명이면 경고 없음).
fn step_warning(seq: usize, pos: &str, act: &str, hits: usize) -> Option<String> {
    match hits {
        0 => Some(format!(
            "{seq}단계 '{pos}'({act})의 담당자를 찾지 못했습니다 — 사람이 직접 지정해야 합니다."
        )),
        1 => None,
        n => Some(format!(
            "{seq}단계 '{pos}'({act})에 후보가 {n}명입니다 — 누구인지 사람이 골라야 합니다."
        )),
    }
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
    let selected = select_branches(&branches, my_grade, trip)?;

    // ── 부서 트리(상대 직책 L_* 해석용) 1콜 ────────────────────────────────
    let tree = org::dept_tree(c, "0").await?;
    let depts = tree.get("depts").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    let dept_by_id = |id: &str| depts.iter().find(|d| d.get("deptId").and_then(|v| v.as_str()) == Some(id)).cloned();
    // 기안자 부서의 조상 dept_id(가까운 상위부터). path 예: "1000|1000|2986|2987|2989|2993|"
    let ancestors: Vec<String> = dept_by_id(&my_dept)
        .and_then(|d| d.get("path").and_then(|v| v.as_str()).map(String::from))
        .map(|p| ancestor_chain(&p, &my_dept))
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

            let status = step_status(hits.len());
            if let Some(w) = step_warning(i + 1, pos, act, hits.len()) {
                warnings.push(w);
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

#[cfg(test)]
mod tests {
    use super::*;

    // 기대값 출처: 위임전결 기준_260801.xlsx(2026-08-01 시행). 기안 주체는 3구간
    // — 팀원 / 팀장 / 사업부장·실장·센터장이상 — 이고, 출장은 국내/해외로 갈린다.
    // 번들 스키마 파일의 내용 자체는 단정하지 않는다(개정되면 깨지므로). 아래 fixture는
    // 그 기준 문서가 정한 *구조*만 본뜬 자체 제작본이다.

    /// 위임전결 기준의 세 구간 이름은 스키마 branch의 `when.grade`와 글자까지 같아야 한다.
    /// 여기가 어긋나면 branch가 하나도 안 골라져 조용히 전체 후보가 나간다.
    #[test]
    fn grade_of는_팀원_팀장을_완전일치로만_판정한다() {
        assert_eq!(grade_of("팀원"), Some("팀원"));
        assert_eq!(grade_of("팀장"), Some("팀장"));
        // 완전일치라서 부분문자열은 안 잡힌다(상위 6종의 contains 규칙과 다르다).
        assert_eq!(grade_of("부팀장"), None);
        assert_eq!(grade_of("팀장대행"), None);
        assert_eq!(grade_of("선임팀원"), None);
    }

    #[test]
    fn grade_of는_상위_6종은_부분일치로_묶는다() {
        for d in ["센터장", "실장", "사업부장", "본부장", "부문장", "대표이사"] {
            assert_eq!(grade_of(d), Some("사업부장/실장/센터장이상"), "duty={d}");
        }
        // 조직 라벨은 앞에 조직명이 붙어 들어온다 — contains라서 잡힌다.
        assert_eq!(grade_of("클라우드기술센터장"), Some("사업부장/실장/센터장이상"));
        assert_eq!(grade_of("인사지원실장"), Some("사업부장/실장/센터장이상"));
        assert_eq!(grade_of("기술운영부문장"), Some("사업부장/실장/센터장이상"));
    }

    /// 못 잡으면 `None` — 이건 실패가 아니라 **의도된 폴백**이다(구간을 단정하지 않고 전체 branch를 준다).
    #[test]
    fn grade_of는_빈값과_미등록_직책을_none으로_둔다() {
        assert_eq!(grade_of(""), None);
        assert_eq!(grade_of("수석"), None);
        assert_eq!(grade_of("프로"), None);
        assert_eq!(grade_of("파트장"), None); // 신설 직책이 생기면 여기로 떨어진다
    }

    #[test]
    fn duty_matches는_or표기의_어느_하나만_걸려도_참이다() {
        let spec = "센터장|실장|사업부장"; // positions.L_센터장
        assert!(duty_matches(spec, "센터장"));
        assert!(duty_matches(spec, "실장"));
        assert!(duty_matches(spec, "사업부장"));
        assert!(!duty_matches(spec, "본부장"));
    }

    #[test]
    fn duty_matches는_구분자_주변_공백을_무시한다() {
        assert!(duty_matches("센터장 | 실장", "실장"));
        assert!(duty_matches("  팀장  ", "팀장"));
    }

    /// 조직의 실제 duty는 "클라우드기술센터장"처럼 조직명이 붙어 온다 — 부분일치라 걸린다.
    #[test]
    fn duty_matches는_부분일치다() {
        assert!(duty_matches("센터장|실장|사업부장", "클라우드기술센터장"));
        assert!(duty_matches("팀장", "인사총무팀장"));
    }

    #[test]
    fn duty_matches는_빈_duty를_항상_거부한다() {
        assert!(!duty_matches("팀장", ""));
        assert!(!duty_matches("", "")); // duty가 비면 spec과 무관하게 false
    }

    /// ⚠️ 현재 동작 고정: spec이 비면 `"".split('|')` → `[""]`이고 `contains("")`는 항상 참이라
    /// **아무 duty나 통과**한다. fixed 분기는 호출부에서 `want_duty.is_empty()`로 막지만
    /// relative 분기에는 그 가드가 없다.
    #[test]
    fn duty_matches는_빈_spec이면_아무나_통과시킨다() {
        assert!(duty_matches("", "팀원"));
        assert!(duty_matches("", "대표이사"));
    }

    #[test]
    fn candidate는_결재선에_필요한_6개_필드만_남긴다() {
        let m = json!({
            "empSeq": "12345",
            "name": "홍길동",
            "duty": "팀장",
            "position": "수석",
            "deptId": "2993",
            "deptName": "클라우드기술팀",
            "email": "buried@example.com", // 결재선에 안 쓰는 필드는 떨어져야 한다
            "mobile": "010-0000-0000"
        });
        assert_eq!(
            candidate(&m),
            json!({
                "empSeq": "12345",
                "name": "홍길동",
                "duty": "팀장",
                "position": "수석",
                "deptId": "2993",
                "deptName": "클라우드기술팀"
            })
        );
    }

    /// empSeq가 빠지면 save_approval_line에 못 넣는다 — 없어진 게 아니라 `null`로 보여야 사람이 안다.
    #[test]
    fn candidate는_없는_필드를_null로_채운다() {
        let out = candidate(&json!({ "name": "홍길동" }));
        assert_eq!(out["name"], json!("홍길동"));
        for k in ["empSeq", "duty", "position", "deptId", "deptName"] {
            assert_eq!(out[k], Value::Null, "field={k}");
        }
        assert_eq!(out.as_object().unwrap().len(), 6);
    }

    /// 260801 기준의 출장신청 구조(3구간 × 국내/해외 = 6 branch)를 본뜬 fixture.
    fn trip_branches() -> Vec<Value> {
        [
            ("팀원", "국내"),
            ("팀장", "국내"),
            ("사업부장/실장/센터장이상", "국내"),
            ("팀원", "해외"),
            ("팀장", "해외"),
            ("사업부장/실장/센터장이상", "해외"),
        ]
        .iter()
        .map(|(g, t)| json!({ "when": { "grade": g, "trip": t }, "line": [] }))
        .collect()
    }

    /// 출장 외 양식(외근·휴가·휴일주말근무)은 trip 없이 grade로만 갈린다.
    fn grade_only_branches() -> Vec<Value> {
        ["팀원", "팀장", "사업부장/실장/센터장이상"]
            .iter()
            .map(|g| json!({ "when": { "grade": g }, "line": [] }))
            .collect()
    }

    #[test]
    fn select_branches는_구간과_출장구분이_모두_맞는_하나를_고른다() {
        let bs = trip_branches();
        let got = select_branches(&bs, Some("팀장"), "해외").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["when"], json!({ "grade": "팀장", "trip": "해외" }));
    }

    /// `my_grade=None`(직책을 못 읽은 폴백)이면 grade 조건을 통과시켜 후보를 넓게 준다.
    #[test]
    fn select_branches는_grade_none이면_해당_trip_전체를_준다() {
        let bs = trip_branches();
        let got = select_branches(&bs, None, "국내").unwrap();
        assert_eq!(got.len(), 3); // 국내 3구간 전부
        assert!(got.iter().all(|b| b["when"]["trip"] == json!("국내")));

        // grade도 trip도 모르면 6개 전부.
        assert_eq!(select_branches(&bs, None, "").unwrap().len(), 6);
    }

    /// `trip`이 빈 문자열이면 국내/해외 둘 다 — 사람이 고르라는 뜻이다.
    #[test]
    fn select_branches는_trip_빈값이면_국내해외를_모두_준다() {
        let bs = trip_branches();
        let got = select_branches(&bs, Some("팀원"), "").unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0]["when"]["trip"], json!("국내"));
        assert_eq!(got[1]["when"]["trip"], json!("해외"));
    }

    /// trip 조건이 아예 없는 양식은 trip을 줘도 무시하고 grade로만 고른다.
    #[test]
    fn select_branches는_when에_trip이_없으면_trip인자를_무시한다() {
        let bs = grade_only_branches();
        let got = select_branches(&bs, Some("팀원"), "해외").unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["when"]["grade"], json!("팀원"));
    }

    #[test]
    fn select_branches는_맞는_branch가_없으면_에러다() {
        let bs = trip_branches();
        // 스키마에 없는 구간
        let e = select_branches(&bs, Some("파트장"), "국내").unwrap_err().to_string();
        assert!(e.contains("파트장"), "{e}");
        assert!(e.contains("get_approval_line_schema"), "{e}");
        // 스키마에 없는 trip 값
        assert!(select_branches(&bs, Some("팀원"), "달나라").is_err());
        // branch가 아예 비었을 때
        assert!(select_branches(&[], None, "").is_err());
    }

    /// path는 최상위 → 자기 부서 순인데, L_* 해석은 **가까운 상위부터** 올라가야 한다.
    #[test]
    fn ancestor_chain은_가까운_상위부터_먼_상위_순이다() {
        let got = ancestor_chain("1000|2986|2987|2989|2993|", "2993");
        assert_eq!(got, vec!["2989", "2987", "2986", "1000"]);
    }

    #[test]
    fn ancestor_chain은_빈_조각과_자기부서를_걷어낸다() {
        // 앞뒤/중간의 빈 조각
        assert_eq!(ancestor_chain("|1000||2986|", "2986"), vec!["1000"]);
        // 자기 부서는 몇 번 나오든 전부 제거
        assert_eq!(ancestor_chain("1000|2986|2986|", "2986"), vec!["1000"]);
    }

    /// 실데이터에 `"1000|1000|…"`처럼 최상위가 중복돼 들어온다 — 중복이 있어도 순서가 안 깨져야 한다.
    #[test]
    fn ancestor_chain은_중복_id가_있어도_순서를_지킨다() {
        let got = ancestor_chain("1000|1000|2986|2987|2989|2993|", "2993");
        assert_eq!(got, vec!["2989", "2987", "2986", "1000", "1000"]);
        // 가장 가까운 상위가 맨 앞 = 여기서부터 duty 보유자를 찾는다.
        assert_eq!(got.first().map(String::as_str), Some("2989"));
    }

    #[test]
    fn ancestor_chain은_최상위_부서면_비어있다() {
        assert!(ancestor_chain("1000|", "1000").is_empty());
        assert!(ancestor_chain("", "2993").is_empty());
    }

    #[test]
    fn step_status는_후보수_0_1_다수를_구분한다() {
        assert_eq!(step_status(0), "미해결");
        assert_eq!(step_status(1), "후보1");
        assert_eq!(step_status(2), "후보다수");
        assert_eq!(step_status(7), "후보다수");
    }

    /// status만 붙고 경고가 안 쌓이면 사용자는 확인해야 할 단계를 놓친다 — 둘은 짝이다.
    #[test]
    fn step_warning은_0명과_다수일_때만_경고를_만든다() {
        let none = step_warning(1, "L_팀장", "결재", 0).expect("0명이면 경고가 있어야 한다");
        assert!(none.contains("1단계 'L_팀장'(결재)"), "{none}");
        assert!(none.contains("찾지 못했습니다"), "{none}");

        assert_eq!(step_warning(2, "L_센터장", "결재", 1), None); // 1명은 조용히 통과

        let many = step_warning(4, "CFO", "합의", 3).expect("다수면 경고가 있어야 한다");
        assert!(many.contains("4단계 'CFO'(합의)"), "{many}");
        assert!(many.contains("3명"), "{many}");
    }
}
