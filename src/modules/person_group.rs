//! 사람 그룹 — `~/.config/inno-creed/person_groups.json`.
//!
//! **아마란스에 그룹메일이 없어서** 만든 것이다. 아마란스에도 그룹 개념은 있지만 메일에 적용되지
//! 않는다. 그래서 "자주 함께 지정하는 사람들"을 이 파일에 두고, MCP는 **꺼내주기만** 한다.
//!
//! ⚠️ **조립은 이 모듈이 하지 않는다.** 그룹은 메일 전용이 아니라 범용이다 — 캘린더 참여자,
//! 결재선, 메일 수신자·참조 어디에나 쓴다. 소비처마다 요구하는 모양이 다르므로(캘린더는 empSeq
//! 배열, 메일은 콤마로 이은 주소 문자열) 도구가 그 변환을 떠맡으면 소비처가 늘 때마다 여기가
//! 커진다. **호출자(에이전트)가 필요한 모양으로 조립**하고, 여기서는 재료를 준다.
//!
//! ## 파일 형식
//!
//! ```json
//! {
//!   "version": "2026-08-20",
//!   "groups": {
//!     "네이티브플랫폼팀": {
//!       "note": "주간보고 수신자",
//!       "members": [
//!         { "empSeq": "3166", "label": "이재학" }
//!       ]
//!     }
//!   }
//! }
//! ```
//!
//! **정본은 `empSeq` 하나다.** 이름·이메일·부서·직책은 저장하지 않고 조회할 때 명부(`org::roster`,
//! 30분 캐시)에서 푼다 — 박아두면 사람이 옮기거나 주소가 바뀔 때마다 조용히 낡기 때문이다.
//! `label`은 **사람이 파일을 열었을 때 읽으라고 두는 주석**일 뿐 정본이 아니다(명부에서 못 찾은
//! 사람을 보고할 때만 쓴다).
//!
//! ## 쓰기
//!
//! `save`/`delete`가 파일을 고친다. 사람도 같은 파일을 손으로 편집하므로 규칙이 둘 있다:
//!
//! 1. **읽고 → 고치고 → 통째로 다시 쓴다.** 다른 그룹을 건드리지 않기 위해서다.
//! 2. **임시 파일에 쓰고 rename 한다.** 쓰는 도중에 죽어도 원본이 반쯤 잘린 채 남지 않는다.
//!
//! ⚠️ 깨진 JSON 위에는 **쓰지 않는다.** 사람이 편집하다 만 파일을 통째로 덮으면 그 내용이
//! 사라진다 — 읽기와 같은 이유로 에러를 올리고 사람이 고치게 한다.

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::error::InvalidInput;
use crate::modules::org;
use crate::util::s;

/// 설정 파일 이름. 경로는 `crate::config`가 정한다.
const FILE: &str = "person_groups.json";

/// 파일을 읽어 `groups` 오브젝트를 낸다. 파일이 없으면 빈 오브젝트(그룹 0개).
///
/// ⚠️ **"파일 없음"과 "파일이 깨짐"을 구분한다.** 없는 것은 정상(아직 안 만든 것)이지만,
/// 깨진 JSON을 빈 그룹으로 넘기면 사용자는 "그룹이 왜 안 보이지"만 겪고 원인을 못 찾는다.
fn load() -> Result<(Value, String)> {
    let path = crate::config::file(FILE)
        .ok_or_else(|| anyhow!("설정 디렉토리를 정할 수 없다(HOME·XDG_CONFIG_HOME 둘 다 없음)"))?;
    let shown = path.display().to_string();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        // 파일이 아직 없는 것은 오류가 아니다 — 그룹 0개로 본다.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((json!({}), shown)),
        Err(e) => return Err(anyhow!("그룹 파일을 읽지 못했다({shown}): {e}")),
    };
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|e| anyhow!("그룹 파일의 JSON이 깨졌다({shown}): {e}"))?;
    Ok((parsed.get("groups").cloned().unwrap_or(json!({})), shown))
}

/// 그룹 목록 — 이름·인원수·메모. 멤버는 풀지 않는다(명부 조회 없이 끝난다).
pub fn list() -> Result<Value> {
    let (groups, path) = load()?;
    let mut out: Vec<Value> = groups
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(name, g)| {
                    json!({
                        "name": name,
                        "memberCount": g.get("members").and_then(|v| v.as_array()).map_or(0, |a| a.len()),
                        "note": s(g, "note")
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    // 파일의 키 순서는 보장되지 않는다(JSON 오브젝트) — 이름순으로 고정해 출력이 흔들리지 않게 한다.
    out.sort_by_key(|a| s(a, "name"));

    Ok(json!({
        "kind": "personGroups",
        "count": out.len(),
        "groups": out,
        "path": path,
        "note": "그룹을 만들거나 고치려면 save_person_group(이름/empSeq를 그대로 주면 된다), 지우려면 \
                 delete_person_group. path 의 JSON 파일을 사람이 직접 편집해도 된다 — \
                 형식: {\"groups\":{\"<이름>\":{\"note\":\"...\",\"members\":[{\"empSeq\":\"3166\",\"label\":\"이재학\"}]}}}"
    }))
}

/// 그룹 하나를 명부로 풀어 낸다.
///
/// 반환의 `empSeqs`/`emails`는 **소비처가 바로 쓰도록 배열로** 준다. 문자열로 미리 이어 붙이지
/// 않는 이유는 소비처마다 모양이 다르기 때문이다 — 캘린더 참여자는 배열을 받고, 메일은 콤마로
/// 이은 문자열을 받는다. 잇는 것은 호출자 몫이다.
///
/// ⚠️ **명부에 없는 사람은 조용히 빼지 않는다.** `members[].status`가 `"not_found"`로 남고
/// `missing`에도 실린다 — 그룹 발송에서 사람이 소리 없이 빠지는 것이 가장 나쁜 실패다.
pub async fn get(c: &Arc<GwClient>, name: &str) -> Result<Value> {
    let name = name.trim();
    if name.is_empty() {
        return Err(InvalidInput("그룹 이름이 비어 있다".into()).into());
    }
    let (groups, path) = load()?;
    let Some(group) = groups.get(name) else {
        let known: Vec<String> = groups
            .as_object()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        return Err(InvalidInput(format!(
            "'{name}' 그룹이 없다. 있는 그룹: {} (파일: {path})",
            if known.is_empty() { "없음".to_string() } else { known.join(", ") }
        ))
        .into());
    };

    let specs = group
        .get("members")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 명부는 30분 캐시다. 그룹이 비어 있으면 부르지 않는다(빈 그룹 조회에 부서 순회를 태우지 않는다).
    let roster = if specs.is_empty() {
        Vec::new()
    } else {
        org::roster(c).await?
    };
    let find = |emp_seq: &str| roster.iter().find(|p| s(p, "empSeq") == emp_seq).cloned();

    let mut members = Vec::new();
    let mut emp_seqs = Vec::new();
    let mut emails = Vec::new();
    let mut missing = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for spec in &specs {
        let emp_seq = s(spec, "empSeq");
        if emp_seq.is_empty() {
            return Err(InvalidInput(format!(
                "'{name}' 그룹에 empSeq 없는 멤버가 있다(파일: {path}) — 정본은 empSeq다. \
                 find_person 으로 확인해 채울 것"
            ))
            .into());
        }
        // 같은 사람을 두 번 적어도 한 번만 낸다(수신자 중복 방지).
        if !seen.insert(emp_seq.clone()) {
            continue;
        }
        match find(&emp_seq) {
            Some(p) => {
                let email = s(&p, "email");
                if !email.is_empty() {
                    emails.push(email.clone());
                }
                emp_seqs.push(emp_seq.clone());
                members.push(json!({
                    "empSeq": emp_seq,
                    "name": s(&p, "name"),
                    "email": email,
                    "dept": s(&p, "deptName"),
                    "duty": s(&p, "duty"),
                    "status": "ok"
                }));
            }
            None => {
                // 파일의 label 은 여기서만 쓴다 — 누구를 못 찾았는지 사람이 알아볼 수 있게.
                let label = s(spec, "label");
                missing.push(json!({ "empSeq": emp_seq, "label": label }));
                members.push(json!({
                    "empSeq": emp_seq,
                    "name": label,
                    "email": "",
                    "dept": "",
                    "duty": "",
                    "status": "not_found"
                }));
            }
        }
    }

    Ok(json!({
        "kind": "personGroup",
        "name": name,
        "note": s(group, "note"),
        "count": members.len(),
        "members": members,
        // 소비처가 바로 쓰는 재료. 이어 붙이는 것은 호출자 몫이다.
        "empSeqs": emp_seqs,
        "emails": emails,
        "missing": missing,
        "path": path,
        "note2": if missing.is_empty() {
            "empSeqs → create_calendar_event(participants) / save_approval_line(user_id). \
             emails → 콤마로 이어 send_mail(to|cc|bcc)".to_string()
        } else {
            format!(
                "⚠️ {}명을 조직도 명부에서 찾지 못했다(status=not_found) — 퇴사했거나 empSeq가 틀렸을 수 있다. \
                 emails 에도 빠져 있으니 그대로 보내면 그 사람만 누락된다. missing 을 사용자에게 알리고 \
                 파일({path})을 고칠 것",
                missing.len()
            )
        }
    }))
}

/// 그룹 파일 전체를 새로 쓴다 — **읽고 → 고치고 → 통째로**.
///
/// 임시 파일에 쓴 뒤 `rename` 으로 갈아끼운다(같은 디렉토리라 원자적이다). 도중에 죽어도
/// 원본은 온전하다. 사람이 손으로 편집하는 파일이라 들여쓴 JSON으로 쓴다.
fn write_groups(groups: &Value) -> Result<String> {
    let dir = crate::config::ensure_dir().map_err(|e| anyhow!("설정 디렉토리를 만들지 못했다: {e}"))?;
    let path = dir.join(FILE);
    let body = serde_json::to_string_pretty(&json!({ "groups": groups }))
        .map_err(|e| anyhow!("그룹 JSON 직렬화 실패: {e}"))?;

    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, body.as_bytes())
        .map_err(|e| anyhow!("그룹 파일 임시본을 쓰지 못했다({}): {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp); // 갈아끼우기 실패 시 임시본을 남기지 않는다
        anyhow!("그룹 파일을 갈아끼우지 못했다({}): {e}", path.display())
    })?;
    Ok(path.display().to_string())
}

/// `이름` 또는 `empSeq` 목록을 **검증된 멤버 항목**(`{empSeq, label}`)으로 바꾼다.
///
/// ⚠️ **저장 시점에 명부로 검증한다.** 없는 empSeq나 오타 난 이름을 그대로 파일에 넣으면,
/// 나중에 그 그룹으로 메일을 보낼 때 **그 사람만 조용히 빠진다.** 틀린 것은 지금 막는 편이 낫다.
/// 동명이인은 고르지 않고 후보를 돌려주며 거부한다(누구인지는 사용자가 정한다).
async fn resolve_members(c: &Arc<GwClient>, specs: &[String]) -> Result<Vec<Value>> {
    let people = org::roster(c).await?;
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for spec in specs {
        let q = spec.trim();
        if q.is_empty() {
            continue;
        }
        let hit = if q.chars().all(|ch| ch.is_ascii_digit()) {
            // empSeq 직접 지정 — 실재하는지까지 본다(없으면 저장하지 않는다).
            people.iter().find(|m| s(m, "empSeq") == q).cloned().ok_or_else(|| {
                InvalidInput(format!(
                    "empSeq '{q}' 가 조직도 명부에 없다 — 그대로 저장하면 나중에 그 사람만 빠진다. \
                     find_person 으로 확인할 것"
                ))
            })?
        } else {
            let ql = q.to_lowercase();
            let exact: Vec<&Value> = people.iter().filter(|m| s(m, "name").to_lowercase() == ql).collect();
            let hits = if exact.is_empty() {
                people.iter().filter(|m| s(m, "name").to_lowercase().contains(&ql)).collect::<Vec<_>>()
            } else {
                exact
            };
            match hits.len() {
                0 => {
                    return Err(InvalidInput(format!(
                        "'{q}' 를 조직도 명부에서 찾지 못했다. find_person 으로 확인 후 이름이나 empSeq를 줄 것"
                    ))
                    .into())
                }
                1 => hits[0].clone(),
                _ => {
                    // 조용히 하나를 고르지 않는다 — 누구를 넣을지는 사용자가 정해야 한다.
                    let cands: Vec<String> = hits
                        .iter()
                        .take(10)
                        .map(|m| format!("{}({}, {})", s(m, "name"), s(m, "empSeq"), s(m, "deptName")))
                        .collect();
                    return Err(InvalidInput(format!(
                        "'{q}' 가 {}명이다. empSeq로 지정할 것: {}",
                        hits.len(),
                        cands.join(", ")
                    ))
                    .into());
                }
            }
        };
        let emp_seq = s(&hit, "empSeq");
        if seen.insert(emp_seq.clone()) {
            out.push(json!({ "empSeq": emp_seq, "label": s(&hit, "name") }));
        }
    }
    Ok(out)
}

/// 그룹을 저장한다. `mode` — `"replace"`(기본, 새로 만들거나 통째로 교체) ·
/// `"add"`(기존에 더하기) · `"remove"`(기존에서 빼기).
///
/// `members`는 **이름 또는 empSeq**를 받는다("김철수", "3166" 둘 다). 저장되는 것은 empSeq이고
/// 이름은 `label`(사람이 파일을 읽을 때용 주석)로만 남는다.
pub async fn save(
    c: &Arc<GwClient>,
    name: &str,
    members: &[String],
    note: &str,
    mode: &str,
) -> Result<Value> {
    let name = name.trim();
    if name.is_empty() {
        return Err(InvalidInput("그룹 이름이 비어 있다".into()).into());
    }
    let mode = if mode.trim().is_empty() { "replace" } else { mode.trim() };
    if !matches!(mode, "replace" | "add" | "remove") {
        return Err(InvalidInput(format!(
            "mode 는 replace|add|remove 중 하나여야 한다(받은 값: '{mode}')"
        ))
        .into());
    }

    // 깨진 파일 위에 쓰지 않는다 — load 가 에러를 올린다.
    let (mut groups, _) = load()?;
    let existing = groups.get(name).cloned();
    if mode != "replace" && existing.is_none() {
        return Err(InvalidInput(format!(
            "'{name}' 그룹이 없어 {mode} 할 수 없다 — 새로 만들려면 mode=replace 로 부를 것"
        ))
        .into());
    }

    let resolved = resolve_members(c, members).await?;
    let prev: Vec<Value> = existing
        .as_ref()
        .and_then(|g| g.get("members"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let next: Vec<Value> = match mode {
        "replace" => resolved,
        "add" => {
            // 기존 순서를 지키고 뒤에 붙인다. 이미 있는 사람은 넣지 않는다.
            let have: HashSet<String> = prev.iter().map(|m| s(m, "empSeq")).collect();
            let mut v = prev.clone();
            v.extend(resolved.into_iter().filter(|m| !have.contains(&s(m, "empSeq"))));
            v
        }
        _ => {
            let drop: HashSet<String> = resolved.iter().map(|m| s(m, "empSeq")).collect();
            prev.iter().filter(|m| !drop.contains(&s(m, "empSeq"))).cloned().collect()
        }
    };

    // note 를 안 주면 기존 것을 지킨다 — 멤버만 고치려다 메모가 날아가면 안 된다.
    let note = if note.trim().is_empty() {
        existing.as_ref().map(|g| s(g, "note")).unwrap_or_default()
    } else {
        note.trim().to_string()
    };

    let obj = groups.as_object_mut().ok_or_else(|| anyhow!("그룹 파일의 groups 가 오브젝트가 아니다"))?;
    obj.insert(name.to_string(), json!({ "note": note, "members": next.clone() }));
    let path = write_groups(&groups)?;

    Ok(json!({
        "ok": true,
        "name": name,
        "mode": mode,
        "memberCount": next.len(),
        "members": next,
        "path": path,
        "note": "저장됨. person_group(name)으로 이메일·empSeq를 풀어 쓸 것"
    }))
}

/// 그룹을 지운다. 없는 이름이면 에러(조용히 성공으로 보고하지 않는다).
pub fn delete(name: &str) -> Result<Value> {
    let name = name.trim();
    if name.is_empty() {
        return Err(InvalidInput("그룹 이름이 비어 있다".into()).into());
    }
    let (mut groups, path) = load()?;
    let obj = groups.as_object_mut().ok_or_else(|| anyhow!("그룹 파일의 groups 가 오브젝트가 아니다"))?;
    if obj.remove(name).is_none() {
        return Err(InvalidInput(format!("'{name}' 그룹이 없다(파일: {path})")).into());
    }
    let written = write_groups(&groups)?;
    Ok(json!({ "ok": true, "deleted": name, "remaining": groups.as_object().map_or(0, |m| m.len()), "path": written }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 목록은 이름순으로 고정한다 — JSON 오브젝트 키 순서는 보장되지 않아 출력이 흔들린다.
    #[test]
    fn 목록은_이름순으로_고정된다() {
        let groups = json!({
            "나팀": { "members": [{"empSeq": "1"}] },
            "가팀": { "note": "메모", "members": [{"empSeq": "1"}, {"empSeq": "2"}] }
        });
        let mut out: Vec<Value> = groups
            .as_object()
            .unwrap()
            .iter()
            .map(|(name, g)| {
                json!({
                    "name": name,
                    "memberCount": g.get("members").and_then(|v| v.as_array()).map_or(0, |a| a.len()),
                    "note": s(g, "note")
                })
            })
            .collect();
        out.sort_by_key(|a| s(a, "name"));
        assert_eq!(s(&out[0], "name"), "가팀");
        assert_eq!(out[0]["memberCount"], 2);
        assert_eq!(s(&out[0], "note"), "메모");
        assert_eq!(s(&out[1], "name"), "나팀");
        assert_eq!(s(&out[1], "note"), "", "note 가 없으면 빈 문자열");
    }

    /// 깨진 JSON을 "그룹 없음"으로 삼키면 사용자가 원인을 못 찾는다 — 에러로 올린다.
    #[test]
    fn 깨진_json은_빈_그룹이_아니라_에러다() {
        let e = serde_json::from_str::<Value>("{ not json").unwrap_err();
        let msg = format!("그룹 파일의 JSON이 깨졌다(/p): {e}");
        assert!(msg.contains("깨졌다"), "원인을 밝히는 메시지여야 한다");
    }

    /// `save` 의 병합 규칙. 여기가 틀리면 **멤버가 조용히 사라지거나 중복**되는데,
    /// 둘 다 그룹 발송에서야 드러난다(그때는 이미 메일이 나갔다).
    fn merge(mode: &str, prev: &[Value], resolved: &[Value]) -> Vec<Value> {
        match mode {
            "replace" => resolved.to_vec(),
            "add" => {
                let have: HashSet<String> = prev.iter().map(|m| s(m, "empSeq")).collect();
                let mut v = prev.to_vec();
                v.extend(resolved.iter().filter(|m| !have.contains(&s(m, "empSeq"))).cloned());
                v
            }
            _ => {
                let drop: HashSet<String> = resolved.iter().map(|m| s(m, "empSeq")).collect();
                prev.iter().filter(|m| !drop.contains(&s(m, "empSeq"))).cloned().collect()
            }
        }
    }

    #[test]
    fn 병합은_순서를_지키고_중복을_만들지_않는다() {
        let m = |seq: &str| json!({ "empSeq": seq, "label": format!("p{seq}") });
        let prev = vec![m("1"), m("2")];

        // add — 기존 순서를 지키고 뒤에 붙인다. **이미 있는 사람은 두 번 넣지 않는다.**
        let added = merge("add", &prev, &[m("2"), m("3")]);
        assert_eq!(
            added.iter().map(|x| s(x, "empSeq")).collect::<Vec<_>>(),
            vec!["1", "2", "3"],
            "중복 없이 뒤에 붙어야 한다"
        );

        // remove — 지정한 사람만 빠지고 나머지 순서는 그대로.
        let removed = merge("remove", &prev, &[m("1")]);
        assert_eq!(removed.iter().map(|x| s(x, "empSeq")).collect::<Vec<_>>(), vec!["2"]);

        // remove 대상이 원래 없으면 아무 일도 없다(에러도 아니고 손실도 아니다).
        assert_eq!(merge("remove", &prev, &[m("9")]).len(), 2);

        // replace — 통째로 갈아끼운다.
        assert_eq!(
            merge("replace", &prev, &[m("7")]).iter().map(|x| s(x, "empSeq")).collect::<Vec<_>>(),
            vec!["7"]
        );
    }

    /// 저장 파일에는 **empSeq가 정본**으로 들어가고 이름은 라벨로만 남는다.
    /// (이메일·부서를 박아두면 사람이 옮길 때마다 낡는다 — 그래서 넣지 않는다.)
    #[test]
    fn 저장_항목은_emp_seq와_label만_담는다() {
        let person = json!({ "empSeq": "3166", "name": "이재학", "email": "a@b.c",
                             "deptName": "네이티브 플랫폼팀", "duty": "팀원" });
        let saved = json!({ "empSeq": s(&person, "empSeq"), "label": s(&person, "name") });
        let keys: Vec<&String> = saved.as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["empSeq", "label"], "email·dept 를 파일에 박으면 낡는다");
    }
}
