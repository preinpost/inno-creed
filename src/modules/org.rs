//! 조직도(ORG) 모듈 — `/gw/APIHandler/gw102A0x`. 읽기 전용.
//! 부서 트리(gw102A01) + 부서별 사원·직책(gw102A02). 결재선 "직책→담당자" 해석의 재료.
//! 인증은 헤더 서명만으로 완결(companyInfo 불필요). 실측: `.claude-workspace/approval-analysis/07-eapproval-api-capture.md` §8.
//!
//! ⚠️ 조회 결과는 정확하지만, 여기서 "이 직책 담당자 = 이 문서 결재자"로 단정하지 말 것.
//! dutyName(직책 텍스트)이 권위 필드이고 dutyCode 숫자 매핑은 불안정 — [[eapproval-server-default-line-untrusted]].

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

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

/// 필드를 문자열로(number/string 혼용 흡수).
fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}
