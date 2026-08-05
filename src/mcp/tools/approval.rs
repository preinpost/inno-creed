//! 전자결재 — 읽기 도구.
//!
//! 라우터는 `approval_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::approval`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::Amaranth;
use crate::mcp::args::approval::*;
use crate::modules;

#[tool_router(router = approval_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(
        description = "[아마란스] 미결함 문서를 제목·기안자·대기일수와 함께 요약한다(오래 기다린 순). approval_counts는 건수만 주므로 실제 처리 판단에는 이쪽을 쓸 것."
    )]
    async fn pending_approvals(
        &self,
        Parameters(a): Parameters<PendingApprovalsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::approval::pending_digest(&self.client, a.page_size.unwrap_or(20))
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "전자결재 함별 문서 목록을 조회한다. box_name=pending(미결)/approved(기결)/approved_ongoing/approved_done/reference(수신참조)/enforcement(시행)/sent(상신)/draft(임시보관). 상신 결과 확인은 sent, 취소 확인도 sent 감소로. draft는 상신 안 된 임시저장·상신취소(purge=false) 복귀 문서이며, 쌓여도 신규 상신을 막지 않는다(과거 '잔여 draft가 2099를 유발한다'는 설은 반증됨) — 정리는 delete_temp_approval."
    )]
    async fn list_approvals(
        &self,
        Parameters(a): Parameters<ListApprovalsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data =
            modules::approval::list_approvals(&self.client, &a.box_name, a.page, a.page_size, &a.from, &a.to)
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "전자결재 문서 1건의 본문(평문)·헤더·결재선을 조회한다(열람 부작용 없음). doc_id+form_id는 list_approvals 결과 사용."
    )]
    async fn read_approval(
        &self,
        Parameters(a): Parameters<ReadApprovalArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = modules::approval::read_approval(&self.client, &a.doc_id, &a.form_id)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "전자결재 함별 미처리 건수를 조회한다(미결/기결/수신참조/시행/상신 등).")]
    async fn approval_counts(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::approval::approval_counts(&self.client)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
