//! 근태 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;


#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AttendanceMonthArgs {
    /// 조회 월 YYYYMM (예: "202608"). start/end를 주면 그쪽이 우선.
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub month: String,
    /// 시작일 YYYYMMDD(선택)
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub start: String,
    /// 종료일 YYYYMMDD(선택)
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub end: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AttendanceTodayArgs {
    /// 조회할 날짜 YYYYMMDD(선택, 비우면 오늘 KST).
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub work_dt: String,
}
