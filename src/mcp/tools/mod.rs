//! MCP 도구 핸들러 — 도메인별 파일. 각 파일이 자기 라우터를 만들고 `Amaranth::all_tools()`가 합친다.

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
