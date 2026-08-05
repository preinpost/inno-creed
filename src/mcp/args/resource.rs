//! 자원(회의실 예약) 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;


#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReserveArgs {
    /// 자원(회의실) ID. list_resources로 확인.
    pub res_seq: String,
    /// 예약명
    pub req_text: String,
    /// 시작 시각 YYYYMMDDHHmm (예: 202608011000)
    pub start: String,
    /// 종료 시각 YYYYMMDDHHmm
    pub end: String,
    /// 내용(선택)
    #[serde(default)]
    pub desc: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CancelArgs {
    /// 자원(회의실) ID
    pub res_seq: String,
    /// 예약 ID (seqNum)
    pub seq_num: i64,
    /// 예약 인덱스 (기본 "1")
    #[serde(default)]
    pub res_idx: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateArgs {
    /// 자원(회의실) ID
    pub res_seq: String,
    /// 예약 ID (seqNum)
    pub seq_num: i64,
    /// 예약 인덱스 (기본 "1")
    #[serde(default)]
    pub res_idx: Option<String>,
    /// 새 예약명 (미지정 시 기존 유지)
    #[serde(default)]
    pub req_text: Option<String>,
    /// 새 시작 YYYYMMDDHHmm (미지정 시 기존 유지)
    #[serde(default)]
    pub start: Option<String>,
    /// 새 종료 YYYYMMDDHHmm (미지정 시 기존 유지)
    #[serde(default)]
    pub end: Option<String>,
    /// 새 내용 (미지정 시 기존 유지)
    #[serde(default)]
    pub desc: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListReservationsArgs {
    /// 시작일 YYYYMMDD
    pub start: String,
    /// 종료일 YYYYMMDD
    pub end: String,
    /// 조회할 자원 ID 목록(비우면 전체 회의실)
    #[serde(default)]
    pub res_seqs: Vec<String>,
    /// true면 서버 원본(74필드, 회의 안건 전문 포함)을 그대로 반환. 기본 false(슬림).
    #[serde(default)]
    pub verbose: bool,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct FindFreeRoomsArgs {
    /// 날짜 YYYYMMDD
    pub date: String,
    /// 필요한 시간(분). 예: 2시간=120
    pub duration_min: i64,
    /// 탐색 구간 HHmm-HHmm (기본 "0900-1800"). 오전만이면 "0900-1200".
    /// 구간을 넓게 줘도 **점심시간 13:00~14:00은 항상 빠진다**.
    #[serde(default)]
    pub window: String,
    /// 건물/자원종류: ""(전체) | "본사" | "구로" | attrSeq 숫자
    #[serde(default)]
    pub group: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MyReservationsArgs {
    /// 시작일 YYYYMMDD
    pub start: String,
    /// 종료일 YYYYMMDD
    pub end: String,
}
