//! MCP 도구 **인자 스키마**만 모아 둔 곳. 도메인별 파일 8개로 나뉜다.
//!
//! ⚠️ 여기 있는 doc comment는 전부 **LLM에게 가는 프롬프트**다(MCP 스키마의 `description`).
//! 문구를 고치면 모델의 인자 채우기 동작이 바뀐다 — 타입 정의가 아니라 문서로 취급할 것.
//!
//! serde 기본값 함수는 여기 모아 둔다. `one()`이 게시판(`ListNoticesArgs.page`)과
//! 전자결재(`ListApprovalsArgs.page`) **양쪽에서 쓰이기** 때문이다 — 도메인 파일에 두면
//! 한쪽이 다른 쪽을 import 하게 된다. 나머지도 성격이 같아 함께 둔다.
//! (`util.rs`는 도메인 무관 순수 함수용이라 성격이 다르다.)

pub mod approval;
pub mod attendance;
pub mod board;
pub mod calendar;
pub mod mail;
pub mod org;
pub mod resource;
pub mod search;

pub(super) fn one() -> i64 {
    1
}

pub(super) fn twenty() -> i64 {
    20
}

pub(super) fn box_pending() -> String {
    "pending".to_string()
}

pub(super) fn thirty() -> i64 {
    30
}
