//! 전자결재 — 상신·회수 도구.
//!
//! 라우터는 `approval_submit_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::approval_submit`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::{map_domain_err_ctx, Amaranth};
use crate::mcp::args::approval::*;
use crate::modules;

#[tool_router(router = approval_submit_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(
        description = "문서를 상신(제출)한다. ⚠️ 실제 결재요청·수신참조 통지가 나감 — 시험 상신은 본인/합의된 인원만 담은 별도 결재라인으로 하고, 끝나면 `cancel_approval(doc_id, form_id, purge=true)`로 되돌릴 것(상신 직후 문서는 doc_sts=30이라 form_id 필요). ⭐ **hp_application_json / bind_data_json 을 어떻게 채우는지는 `get_approval_submission_guide(양식명 또는 form_id)` 의 `draftHelp` 를 먼저 조회할 것** — 양식별 고정코드(atCd/linkAtCd 등)·의미별 채울 필드·복사용 실동작 예시(hpApplicationExample/bindDataExample)·권장 제목(defaultDocTitle)을 준다(CLI --help 격). 신원은 이 도구가 로그인 사용자 값으로 **자동 주입**한다 — 코드계(coCd/deptCd/empCd)·이름뿐 아니라 **문서에 렌더되는 표시문자열(부서명·직급·직책, `singleDeptNm`/`empNmDutyNm`/`employees` 등)까지** 조직도 값으로 덮어쓰므로 예시값을 그대로 둬도 됨. 결재라인은 `suggest_approval_line`으로 후보를 받아 **사용자 확인 후** save_approval_line으로 등록할 것. 흐름(근태): 0hr00011 → create(appSq 획득) → eap110A03(결재선 병합 + 양식별 form_d_tp 취득) → HP interlock 등록 3콜(GetLinkKey→saveAttendApplicationLinkKey→SetEnageGroup) → eap110A06 상신. **이 interlock 등록이 빠지면 2099(HP_HPD0110_000XX)** — 근태 상신 실패의 사실상 유일한 원인이었다(잔여 draft·날짜·payload 가설은 전부 반증됨). 성공 시 새 docId를 반환한다 — **docId 발급을 도구가 직접 판정하므로**(없으면 에러) 응답이 오면 상신된 것이다. 실증 범위: 근태 4양식(연차36/출장40/외근41/휴일43) 순수 API 상신·취소 e2e. HP 비연동(비근태) 양식은 hp_application_json 없이 호출하는 경로가 있으나 **미검증**."
    )]
    async fn submit_approval(
        &self,
        Parameters(a): Parameters<SubmitApprovalArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::approval_submit::submit_approval(
            &self.client,
            a.form_id,
            &a.doc_title,
            a.line_id,
            &a.hp_application_json,
            &a.bind_data_json,
            &a.doc_contents_html,
            &a.numbering_id,
        )
        .await
        .map_err(map_domain_err_ctx("상신 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "상신 문서를 취소한다. 문서 상태(doc_sts)에 따라 결재취소(eap110A54)→상신취소(eap110A18)→(purge시)임시보관삭제(eap110A19)를 순차 실행. ⚠️ doc_sts=30(결재 진행중) 문서는 결재취소가 선행돼야 하며 form_id 필요(list_approvals의 formId). 상신 직후(20)면 form_id 없이 상신취소만. purge=true면 임시보관 문서까지 완전 삭제. **검증은 도구가 한다** — 실행 후 문서 상태를 재조회해 `ok`/`verified_by_readback`(purge=false면 doc_sts 10 복귀, purge=true면 doc_sts 999=삭제)로 알려주므로 별도 확인 호출이 필요 없다. `ok:false`는 **반영이 확인되지 않았다**는 뜻이니(반영 실패이거나 확인 실패 — `postState`/`note`에 구분해 담긴다) 그대로 사용자에게 알릴 것. 없는 docId·남의 문서(기안자를 확인할 수 없는 경우 포함)·이미 삭제된 문서를 되돌리려는 요청은 **실행 없이 에러**로 끝난다. **취소가 실증된 상태는 10(임시보관)·20(상신)·30(결재 진행중)뿐**이라 종결(90)·반려(100) 등은 거동이 관측되지 않아 실행 없이 거부한다 — 그 문서는 아마란스 웹에서 처리할 것. 이미 삭제된 문서에 purge=true를 다시 걸면 `already:true`와 빈 `steps`로 '할 일이 없었다'를 알려준다."
    )]
    async fn cancel_approval(
        &self,
        Parameters(a): Parameters<CancelApprovalArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::approval_submit::cancel_and_verify(
            &self.client,
            a.doc_id.trim(),
            a.form_id.trim(),
            a.purge,
        )
        .await
        .map_err(map_domain_err_ctx("상신취소 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "임시보관 전자결재 문서를 삭제한다(eap107A25). doc_ids는 콤마구분 docId(list_approvals(box_name:\"draft\")에서 확인). ⚠️ 실제 삭제(복구 불가). 용도는 상신취소(purge=false)로 되돌아온 문서나 시험 잔여물 정리 — **상신 실패(2099)의 해결책이 아니다**(잔여 draft 원인설은 반증, 원인은 interlock 등록 누락). 삭제 후 draft 재조회로 검증."
    )]
    async fn delete_temp_approval(
        &self,
        Parameters(a): Parameters<DeleteTempApprovalArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::approval_submit::delete_temp_approval(&self.client, a.doc_ids.trim())
            .await
            .map_err(map_domain_err_ctx("임시보관 삭제 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
