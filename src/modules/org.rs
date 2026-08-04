//! 조직도(ORG) 모듈 — `/gw/APIHandler/gw102A0x`. 읽기 전용.
//! 부서 트리(gw102A01) + 부서별 사원·직책(gw102A02). 결재선 "직책→담당자" 해석의 재료.
//! 인증은 헤더 서명만으로 완결(companyInfo 불필요). 실측: `.claude-workspace/approval-analysis/07-eapproval-api-capture.md` §8.
//!
//! ⚠️ 조회 결과는 정확하지만, 여기서 "이 직책 담당자 = 이 문서 결재자"로 단정하지 말 것.
//! dutyName(직책 텍스트)이 권위 필드이고 dutyCode 숫자 매핑은 불안정 — [[eapproval-server-default-line-untrusted]].

use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tokio::task::JoinSet;

use crate::client::GwClient;

/// 부서 트리 — gw102A01. `parent_seq`="0"이면 전사 트리, 특정 deptId면 그 부서 하위 서브트리.
/// `isTreeAllOpen:true`로 **전체 노드 펼침**(과거 partial 트리 문제 해소 — 경영지원본부 하위 인사총무팀/인사지원실까지 나옴).
/// `path`(dept_id 경로)로 기안자 부서에서 상위(센터/본부/부문)를 추적.
pub async fn dept_tree(c: &GwClient, parent_seq: &str) -> Result<Value> {
    let parent = if parent_seq.trim().is_empty() { "0" } else { parent_seq.trim() };
    let body = json!({
        "parentSeq": parent, "popupType": "main", "selectedType": "tree",
        "isAllCompShow": false, "compFilter": "", "isTreeChecked": "",
        "isTreeAllOpen": true, "isPartYn": false
    });
    let data = c.call("/gw/APIHandler/gw102A01", &body).await?;
    let arr = data
        .get("treeList")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("gw102A01 응답에 treeList 없음"))?;
    let depts: Vec<Value> = arr
        .iter()
        .map(|d| {
            json!({
                "deptId": s(d, "id"),
                "name": s(d, "text"),
                "path": s(d, "path"),          // dept_id 경로 예: 1000|1000|2986|2987|2989|2993|
                "parentSeq": s(d, "parentSeq"),
                "level": d.get("orgLevel").cloned().unwrap_or(Value::Null),
                "gubun": s(d, "orgGubun"),     // c=회사, b=사업장, d=부서
                "userCount": d.get("childUserCnt").cloned().unwrap_or(Value::Null)
            })
        })
        .collect();
    Ok(json!({ "kind": "deptTree", "parentSeq": parent, "count": depts.len(), "depts": depts }))
}

/// 부서별 사원 목록 — gw102A02. `dept_id`(deptTree의 deptId)의 사원 전원 + 직책(dutyName).
pub async fn dept_members(c: &GwClient, dept_id: &str) -> Result<Value> {
    let body = json!({
        "selectedId": dept_id, "orgGubun": "d",
        "popupType": "main", "selectedType": "tree",
        "searchDiv": "all", "searchText": "",
        "isBdayOption": "1", "isJoinDayOption": "0",
        "isOrganizationDisplayOption": "5|0|1|3|",
        "isGridListDisplayOption": "0", "isLoginIdOption": "1"
    });
    let data = c.call("/gw/APIHandler/gw102A02", &body).await?;
    let arr = data
        .as_array()
        .ok_or_else(|| anyhow!("gw102A02 응답이 배열이 아님"))?;
    let members: Vec<Value> = arr
        .iter()
        .map(|m| {
            json!({
                "empSeq": s(m, "empSeq"),
                "name": s(m, "empName"),
                "loginId": s(m, "loginId"),
                "duty": s(m, "dutyName"),          // 직책(팀장/센터장/본부장/실장/팀원) — 권위 필드
                "dutyCode": s(m, "dutyCode"),      // 참고용(숫자 매핑은 불안정, 단정 금지)
                "position": s(m, "positionName"),  // 직급(부장/책임연구원 등)
                "deptId": s(m, "deptSeq"),
                "deptName": s(m, "deptName"),
                "deptPath": s(m, "pathName"),      // 사람 읽기용 경로 예: (주)이노그리드>…>네이티브 플랫폼팀
                "email": s(m, "emailAddr"),
                "mobile": s(m, "mobileTelNum"),
                "note": s(m, "atNm")               // 부재 표시(예: 육아휴직)
            })
        })
        .collect();
    Ok(json!({ "kind": "deptMembers", "deptId": dept_id, "count": members.len(), "members": members }))
}

/// 부서 순회 동시 호출 수. 서버 부하와 응답시간의 절충(75개 부서 기준).
const ROSTER_CONCURRENCY: usize = 8;

/// 전사 사원 명부(캐시 30분). `dept_tree`(1콜)로 부서를 뽑고, **인원이 있는 부서마다**
/// gw102A02를 호출해 합친다. 전사 일괄 조회는 불가능하다 — gw102A02에 회사/사업장 노드
/// (`orgGubun` c/b)를 주면 **0명**이 오는 것을 실측 확인(2026-08-04). 그래서 부서 단위 순회가
/// 유일한 경로이고, 비용이 커서 캐시가 필수다.
pub async fn roster(c: &Arc<GwClient>) -> Result<Vec<Value>> {
    if let Some(cached) = c.cached_roster() {
        return Ok(cached);
    }
    let tree = dept_tree(c, "0").await?;
    let depts = tree
        .get("depts")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // 실제 부서(gubun=="d") + 인원>0 인 곳만. 회사/사업장 노드는 멤버가 안 나온다.
    let targets: Vec<String> = depts
        .iter()
        .filter(|d| d.get("gubun").and_then(|v| v.as_str()) == Some("d"))
        .filter(|d| d.get("userCount").and_then(|v| v.as_i64()).unwrap_or(0) > 0)
        .filter_map(|d| d.get("deptId").and_then(|v| v.as_str()).map(String::from))
        .collect();

    let spawn = |set: &mut JoinSet<Result<Value>>, dept_id: String| {
        let cc = Arc::clone(c);
        set.spawn(async move { dept_members(&cc, &dept_id).await });
    };

    let mut set: JoinSet<Result<Value>> = JoinSet::new();
    let mut queue = targets.into_iter();
    for _ in 0..ROSTER_CONCURRENCY {
        match queue.next() {
            Some(id) => spawn(&mut set, id),
            None => break,
        }
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut people: Vec<Value> = Vec::new();
    while let Some(joined) = set.join_next().await {
        // 개별 부서 실패는 건너뛴다(권한 없는 부서 등) — 명부 전체를 실패시키지 않는다.
        if let Ok(Ok(v)) = joined {
            if let Some(members) = v.get("members").and_then(|m| m.as_array()) {
                for m in members {
                    let key = m.get("empSeq").and_then(|v| v.as_str()).unwrap_or("").to_string();
                    if !key.is_empty() && seen.insert(key) {
                        people.push(m.clone());
                    }
                }
            }
        }
        if let Some(id) = queue.next() {
            spawn(&mut set, id);
        }
    }

    c.set_roster(people.clone());
    Ok(people)
}

/// 이름·로그인ID·이메일로 사람 찾기. 결재선/참석자/수신자에 필요한 `empSeq`를 얻는 진입점.
/// 매칭은 부분일치(대소문자 무시)이고, **완전일치를 앞에 정렬**한다.
/// ⚠️ 서버측 인물검색 API는 쓰지 않는다 — gw102A02의 `searchText`는 **서버가 무시**하고
/// (부서 인원 17명이 검색어와 무관하게 그대로 반환), `/ab/ab099A23`은 JSON이 아닌 응답을
/// 준다. 둘 다 2026-08-04 실측. 그래서 명부를 받아 클라이언트에서 거른다.
pub async fn find_person(c: &Arc<GwClient>, query: &str) -> Result<Value> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Err(anyhow!("검색어가 비어 있습니다"));
    }
    let people = roster(c).await?;
    let field = |m: &Value, k: &str| m.get(k).and_then(|v| v.as_str()).unwrap_or("").to_lowercase();

    let mut hits: Vec<Value> = people
        .iter()
        .filter(|m| {
            field(m, "name").contains(&q)
                || field(m, "loginId").contains(&q)
                || field(m, "email").contains(&q)
        })
        .cloned()
        .collect();
    // 이름 완전일치 우선 → 동명이인이 있어도 의도한 사람이 먼저 온다.
    hits.sort_by_key(|m| if field(m, "name") == q { 0 } else { 1 });

    Ok(json!({
        "kind": "findPerson",
        "query": query,
        "count": hits.len(),
        "rosterSize": people.len(),
        "people": hits
    }))
}

/// 필드를 문자열로(number/string 혼용 흡수).
fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}
