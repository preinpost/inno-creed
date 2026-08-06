//! 통합검색 도구.
//!
//! 라우터는 `search_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::search`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::{map_domain_err, Amaranth};
use crate::mcp::args::search::*;
use crate::modules;

#[tool_router(router = search_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(
        description = "[아마란스] 메일·전자결재·게시판·일정·자원·파일을 한 번에 검색한다. 결과에 후속 조회용 ID 포함(메일 muid→read_mail, 결재 docId+formId→read_approval, 게시판 artSeqNo→read_notice). scope 미지정 시 전체 모듈."
    )]
    async fn search(
        &self,
        Parameters(a): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::search::search(
            &self.client,
            &a.query,
            &a.scope,
            a.limit.unwrap_or(10),
            &a.from,
            &a.to,
        )
        .await
        .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
