//! 조직도 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;


#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct FindPersonArgs {
    /// 이름·로그인ID·이메일 일부. 예: "홍길동"
    pub query: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct OrgChartArgs {
    /// 부서 ID(deptId). 지정하면 그 부서의 **사원+직책 목록**(gw102A02). 비우면 부서 트리.
    #[serde(default)]
    pub dept_id: String,
    /// 트리 조회 시 시작 노드(deptId). 비우면 전사 트리(전체 펼침). 특정 deptId면 그 하위 서브트리. dept_id가 지정되면 무시됨.
    #[serde(default)]
    pub parent_seq: String,
}
