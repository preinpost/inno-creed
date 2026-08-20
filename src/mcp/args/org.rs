//! 조직도 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;


#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct FindPersonArgs {
    /// 이름·로그인ID·이메일 **또는 조직정보**(부서명·부서경로·직책·직급)의 일부. 예: "홍길동", "네이티브 플랫폼팀", "팀장", "책임연구원". **숫자만 주면 empSeq 완전일치**로 그 사람을 되짚는다. 예: "3081"
    pub query: String,
    /// 반환할 최대 인원(기본 20). 잘리면 응답에 `truncated:true`와 안내가 붙는다.
    #[serde(default)]
    pub limit: Option<i64>,
    /// true면 상한 없이 전부 반환(limit 무시). 부서 전체처럼 인원이 많으면 응답이 매우 커진다.
    #[serde(default)]
    pub no_limit: bool,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct OrgChartArgs {
    /// 부서 ID(deptId). 지정하면 그 부서의 **사원+직책 목록**(gw102A02). 비우면 부서 트리.
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub dept_id: String,
    /// 트리 조회 시 뿌리로 삼을 부서(deptId). 비우면 전사 트리. 지정하면 **그 부서와 하위만** 잘라서 준다(예: "2989" → 클라우드 네이티브 센터와 그 팀들). dept_id가 지정되면 무시됨.
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub parent_seq: String,
    /// true면 계층 없이 **평면 목록**으로 준다(부서마다 path·parentSeq·level 포함). 부서를 조건으로 훑거나 셀 때. 기본 false(중첩 트리 — 조직 구조를 볼 때). dept_id가 지정되면 무시됨.
    #[serde(default)]
    pub flat: bool,
}
