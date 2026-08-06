//! 전자결재 — 규칙·가이드(정적 데이터) 도구.
//!
//! 라우터는 `approval_meta_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::approval_schema / submission_guide`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::{map_domain_err, Amaranth};
use crate::mcp::args::approval::*;
use crate::modules;

#[tool_router(router = approval_meta_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(
        description = "문서 종류별 결재라인 스키마(직책 기반) **원본**을 조회한다. branches[].line[]은 act(결재/합의)+pos(직책)만 담는다. ⭐ 사람까지 해석된 결과가 필요하면 이 도구 대신 **`suggest_approval_line`**을 쓸 것(본인 직책으로 branch 자동 선택 + 각 pos를 조직도로 후보 해석). 이 도구는 스키마 자체를 보고 싶을 때(규칙 확인·branch 수동 선택)용이다. pos 해석 규칙: `L_*`(relative)는 기안자 부서에서 상위로 올라가며 duty 보유자, 나머지(인사총무팀장 등 fixed)는 positions[].dept의 부서원에서 duty로 찾는다. ⛔ 서버 자동 결재선 신뢰 금지, 상신 전 사람 확인 필수. ℹ️ 양식필수 합의자·수신참조·시행자는 상신 시 서버(eap110A03)가 자동 병합하므로 개인결재라인에 중복 등록하지 말 것. 현재 근태 계열(외근/연차/출장/휴일근무)만 수록. 버전·출처는 반환 필드(version/source; 현재 위임전결 기준_260801.xlsx), 사용자 override(~/.config/inno-creed/approval_line.json)가 더 최신이면 그쪽 사용."
    )]
    async fn get_approval_line_schema(
        &self,
        Parameters(a): Parameters<GetApprovalSchemaArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = modules::approval_schema::get_schema(&a.doc_type)
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "수록된 결재라인 스키마 목록(양식명/form_id/alias/버전/출처)을 조회한다. 어떤 문서 종류에 스키마가 있는지 확인용."
    )]
    async fn list_approval_line_schemas(&self) -> Result<CallToolResult, ErrorData> {
        let data = modules::approval_schema::list_schemas()
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "양식별 '신청 가이드'를 반환한다 — ⭐ **submit_approval 기안 데이터 채우는 법(draftHelp: 고정코드 fixed/채울필드 fill/복사용 실동작 예시 hpApplicationExample·bindDataExample/권장 제목 defaultDocTitle·titleHelp) = CLI --help 격**. 그 외 문서 본문 필수항목(requiredBody)·아마란스 웹 작성 절차(steps)·주의(notes)·결재라인 힌트(approvalLineHint) 포함. submit_approval 호출 전 이 도구로 draftHelp를 조회해 hp_application_json/bind_data_json/doc_title을 구성할 것. 결재라인은 get_approval_line_schema+save_approval_line로 준비. 수록 범위는 근태 4양식(연차36/출장40/외근41/휴일43) — list_submission_guides로 확인."
    )]
    async fn get_submission_guide(
        &self,
        Parameters(a): Parameters<GetSubmissionGuideArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = modules::submission_guide::get_guide(&a.doc_type)
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "신청 가이드가 수록된 양식 목록(양식명/form_id/alias)을 조회한다.")]
    async fn list_submission_guides(&self) -> Result<CallToolResult, ErrorData> {
        let data = modules::submission_guide::list_guides()
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
