//! 게시판 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;
use super::{one, twenty};


#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListNoticesArgs {
    /// 페이지 번호(기본 1)
    #[serde(default = "one")]
    pub page: i64,
    /// 페이지 크기(기본 20)
    #[serde(default = "twenty")]
    pub page_size: i64,
    /// 검색어(선택). field로 대상 지정.
    #[serde(default)]
    pub search: String,
    /// 검색 대상(선택): "title"(제목)/"content"(내용)/"author"(작성자). 그 외/미지정은 통합검색.
    #[serde(default)]
    pub field: String,
    /// 등록일 시작(선택, YYYY-MM-DD)
    #[serde(default)]
    pub start_date: String,
    /// 등록일 종료(선택, YYYY-MM-DD)
    #[serde(default)]
    pub end_date: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReadNoticeArgs {
    /// 게시글 ID(artSeqNo). list_notices 결과의 artSeqNo 사용.
    pub art_seq_no: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListAttachmentsArgs {
    /// 게시글 번호(art_seq_no). list_notices/read_notice 결과의 artSeqNo 사용.
    pub art_seq_no: String,
    /// 게시글 첨부 uid(attachmentUid). list_notices/read_notice 결과의 attachmentUid 사용.
    pub uid: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DownloadAttachmentArgs {
    /// 게시글 번호(art_seq_no). list_attachments와 동일.
    pub art_seq_no: String,
    /// 게시글 첨부 uid(attachmentUid). list_attachments와 동일.
    pub uid: String,
    /// 파일 순번(0-base). list_attachments 결과 배열의 인덱스. 기본 0.
    #[serde(default)]
    pub file_sn: i64,
    /// 저장 경로(절대경로 권장). 예: /tmp/notice.pdf
    pub out_path: String,
}
