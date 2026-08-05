//! 통합검색 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;


#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SearchArgs {
    /// 검색어
    pub query: String,
    /// 범위: ""/"전체" | "메일" | "결재" | "게시판" | "일정" | "자원" | "파일"
    #[serde(default)]
    pub scope: String,
    /// 모듈당 결과 수(기본 10, 최대 50)
    #[serde(default)]
    pub limit: Option<i64>,
    /// 시작일 YYYY-MM-DD(선택)
    #[serde(default)]
    pub from: String,
    /// 종료일 YYYY-MM-DD(선택)
    #[serde(default)]
    pub to: String,
}
