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
    /// 비밀메모(선택) — **작성자 본인만 볼 수 있다**. 참여자에게도 보이지 않는다.
    /// ⚠️ 일정이 삭제되거나 본인이 참여자에서 빠지면 사라진다.
    /// ⚠️ 조회 응답에 이 필드가 없어 **반영 여부를 확인할 수 없다**(응답의 secretMemo.verified=false).
    #[serde(default)]
    pub secret_memo: Option<String>,
    /// 참여자(선택) — 본인 외 참석자. 이름 또는 empSeq 목록.
    /// 본인은 항상 주최자로 포함되므로 넣지 않아도 된다.
    /// 이름이 명부에 없거나 동명이인이면 **등록하지 않고 실패**한다 — find_person으로 empSeq를 확인해 지정할 것.
    /// ⚠️ 참여자로 지정된 사람의 아마란스 일정에 이 일정이 나타난다(메일 발송은 하지 않는다).
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string_vec_opt")]
    #[schemars(schema_with = "super::flex_str_vec_opt_schema")]
    pub participants: Option<Vec<String>>,
    /// 화상회의 사용 여부 "Y"/"N" (기본 "N").
    #[serde(default)]
    pub video: Option<String>,
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
