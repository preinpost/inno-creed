//! 도메인 공통 에러 타입.
//!
//! 모듈은 `anyhow::Result`를 쓰지만, **MCP 층이 에러 종류를 구분해야 하는 경우**가 있다.
//! 소유권 위반은 "서버/네트워크 실패"가 아니라 **호출자가 잘못된 대상을 지목한 것**이라
//! `invalid_params`로 분류돼야 하고, 그러려면 문자열이 아닌 타입으로 전달돼야 한다.
//!
//! 사용법: 모듈은 `anyhow::Error`로 감싸 올리고(`Err(NotOwner{..}.into())`),
//! `mcp.rs`는 `err.downcast_ref::<NotOwner>()`로 판별해 `ErrorData::invalid_params`에 매핑한다.
//! (문자열 매칭은 취약해서 쓰지 않는다.)

use thiserror::Error;

/// 본인 소유가 아닌 대상에 쓰기를 시도했을 때. `docs/architecture.md` §7.2 소유권 가드.
#[derive(Debug, Error)]
// 조사는 "이"로 고정 — 이 에러가 다루는 대상(예약/일정)은 모두 받침으로 끝나 "이"가 맞다.
// `{relation}자` = "소유자"/"작성자" — 도메인마다 다른 호칭을 한 필드로 처리한다.
#[error("본인 {relation} {kind}이 아니라 {action}할 수 없습니다 ({relation}자 empSeq={owner}, 본인={me})")]
pub struct NotOwner {
    /// 소유 관계(예: "소유"=자원 예약, "작성"=일정). 뒤에 "자"를 붙여 호칭으로도 쓴다.
    pub relation: &'static str,
    /// 대상 종류(예: "예약", "일정").
    pub kind: &'static str,
    /// 시도한 동작(예: "수정", "취소").
    pub action: &'static str,
    /// 실제 소유자 empSeq. 필드가 없으면 빈 문자열 — 그 경우도 거부한다(불일치).
    pub owner: String,
    /// 로그인 사용자 empSeq.
    pub me: String,
}

/// 호출자가 준 인자가 잘못됐을 때(존재하지 않는 대상·필수 인자 누락 등).
/// 서버/네트워크 실패가 아니므로 `invalid_params`로 분류돼야 한다.
///
/// 메시지를 그대로 담는 얇은 타입인 이유: 문구는 케이스마다 제각각이라 구조화할 게 없고,
/// **필요한 정보는 "이건 호출자 잘못" 이라는 분류 하나**뿐이기 때문이다.
#[derive(Debug, Error)]
#[error("{0}")]
pub struct InvalidInput(pub String);

impl InvalidInput {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}
