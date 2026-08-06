//! 메일 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;


#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SendMailArgs {
    /// 받는사람 (표시형 "이름 <email>" 또는 email). 미지정 시 본인에게 발송.
    #[serde(default)]
    pub to: Option<String>,
    /// 제목
    pub subject: String,
    /// 본문 HTML(선택)
    #[serde(default)]
    pub html: String,
    /// 첨부할 로컬 파일 경로 목록(선택, 절대경로). 비우면 첨부 없음.
    #[serde(default)]
    pub attachments: Vec<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SaveMailDraftArgs {
    /// 받는사람 (표시형 "이름 <email>" 또는 email). 미지정 시 본인.
    #[serde(default)]
    pub to: Option<String>,
    /// 제목. 비워두면 "(제목없음)"으로 저장된다.
    pub subject: String,
    /// 본문 HTML(선택)
    #[serde(default)]
    pub html: String,
    /// 첨부할 로컬 파일 경로 목록(선택, 절대경로). 비우면 첨부 없음.
    #[serde(default)]
    pub attachments: Vec<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteMailArgs {
    /// 삭제할 메일 muid 목록(콤마 구분). list_mail_inbox의 muid 사용.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub uids: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReadMailArgs {
    /// 메일 muid. list_mail_inbox 결과의 muid 사용.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub muid: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DownloadMailAttachmentArgs {
    /// 메일 muid. read_mail/list_mail_inbox의 muid.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub muid: String,
    /// read_mail 응답 `attachments[].fileSn` 의 토큰 문자열을 **그대로** 붙여넣는다.
    /// ⚠️ 순번(0,1,2)이 아니다 — 숫자를 주면 서버가 422로 거절한다.
    #[serde(deserialize_with = "super::flex_string")]
    #[schemars(schema_with = "super::flex_str_schema")]
    pub file_sn: String,
    /// 저장 경로(절대경로 권장). 예: /tmp/attach.png
    pub out_path: String,
}
