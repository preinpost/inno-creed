//! 일정(캘린더) 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;


#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListEventsArgs {
    /// 시작일 YYYYMMDD
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub start: String,
    /// 종료일 YYYYMMDD
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub end: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateEventArgs {
    /// 일정 제목
    pub title: String,
    /// 시작 시각 YYYYMMDDHHmm (예: 202608071100)
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub start: String,
    /// 종료 시각 YYYYMMDDHHmm
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub end: String,
    /// 메모/내용(선택)
    #[serde(default)]
    pub contents: String,
    /// 종일 일정 여부 "Y"/"N" (기본 "N")
    #[serde(default)]
    pub allday: Option<String>,
    /// 등록할 캘린더 — mcalSeq 또는 캘린더 이름(부분 일치). 미지정 시 본인 개인 캘린더.
    /// 공용 캘린더에 등록하려면 list_calendars로 확인 후 지정. ⚠️ 공용은 다른 사람에게도 보인다.
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string_opt")]
    #[schemars(schema_with = "super::flex_str_opt_schema")]
    pub calendar: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateEventArgs {
    /// 일정 ID (schSeq). list_events로 확인.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub sch_seq: String,
    /// 대상 일정이 걸친 날짜 YYYYMMDD (원본 조회·소유권 확인용)
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub date: String,
    /// 새 제목 (미지정 시 기존 유지)
    #[serde(default)]
    pub title: Option<String>,
    /// 새 내용 (미지정 시 기존 유지)
    #[serde(default)]
    pub contents: Option<String>,
    /// 새 시작 YYYYMMDDHHmm (미지정 시 기존 유지)
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string_opt")]
    #[schemars(schema_with = "super::flex_str_opt_schema")]
    pub start: Option<String>,
    /// 새 종료 YYYYMMDDHHmm (미지정 시 기존 유지)
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string_opt")]
    #[schemars(schema_with = "super::flex_str_opt_schema")]
    pub end: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteEventArgs {
    /// 일정 ID (schSeq). list_events로 확인.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub sch_seq: String,
    /// 대상 일정이 걸친 날짜 YYYYMMDD (원본 조회·소유권 확인용)
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub date: String,
}
