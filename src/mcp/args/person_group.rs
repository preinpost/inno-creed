//! 사람 그룹 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct PersonGroupArgs {
    /// 조회할 그룹 이름. **비우면 그룹 목록**(이름/인원수/메모)을 준다.
    /// 이름은 목록에 나온 것을 그대로 쓴다(부분 일치가 아니라 정확히 일치해야 한다).
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub name: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SavePersonGroupArgs {
    /// 그룹 이름. 나중에 이 이름으로 부른다(예: "네이티브플랫폼팀", "주간보고").
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub name: String,
    /// 멤버 — **이름 또는 empSeq** 목록. 예: `["김철수", "3166"]`.
    /// 저장되는 정본은 empSeq이고 이름은 사람이 파일을 읽을 때용 라벨로만 남는다.
    /// ⚠️ 저장 시점에 조직도 명부로 검증한다 — 없는 사람이거나 **동명이인이면 저장하지 않고
    /// 후보를 돌려준다**(누구인지는 사용자가 정한다).
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string_vec_opt")]
    #[schemars(schema_with = "super::flex_str_vec_opt_schema")]
    pub members: Option<Vec<String>>,
    /// 그룹 메모(선택). 비우면 기존 메모를 유지한다.
    #[serde(default)]
    pub note: String,
    /// `replace`(기본) = 새로 만들거나 멤버를 통째로 교체 · `add` = 기존에 더하기 ·
    /// `remove` = 기존에서 빼기. add/remove는 그룹이 이미 있어야 한다.
    #[serde(default)]
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub mode: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeletePersonGroupArgs {
    /// 지울 그룹 이름. 없는 이름이면 실패한다.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub name: String,
}
