//! MCP 도구 핸들러 — 도메인별 파일. 각 파일이 자기 라우터를 만들고 `Amaranth::all_tools()`가 합친다.
//!
//! ⚠️ **도메인 모듈 에러는 `super::map_domain_err`(접두사가 필요하면 `map_domain_err_ctx`)로만 감싼다.**
//! `ErrorData::internal_error`를 직접 부르면 `NotOwner`/`InvalidInput`이 `invalid_params` 대신
//! `internal_error`로 나가는데, 컴파일러도 도구 표면 스냅샷도 그것을 잡지 못한다.
//! 예외는 도구 층이 스스로 판정한 인자 오류(JSON 파싱 실패·없는 doc_type·잘못된 month)뿐 —
//! 그건 모듈에 닿기 전이라 `ErrorData::invalid_params`를 직접 쓴다.

pub mod approval;
pub mod approval_line;
pub mod approval_meta;
pub mod approval_submit;
pub mod attendance;
pub mod board;
pub mod calendar;
pub mod mail;
pub mod org;
pub mod resource;
pub mod search;
