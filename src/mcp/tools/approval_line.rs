//! 전자결재 — 결재선 도구.
//!
//! 라우터는 `approval_line_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::approval_line / approval_line_suggest`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::Amaranth;
use crate::mcp::args::approval::*;
use crate::modules;

#[tool_router(router = approval_line_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(
        description = "이 양식을 **내가 기안할 때의 결재선 후보**를 한 번에 제안한다(스키마 + 조직도 해석). 하는 일: 본인 직책(duty)으로 grade 구간 판정 → 해당 branch 선택(출장은 trip 국내/해외) → 각 직책을 실제 사람 후보로 해석(L_* 상대직책은 기안자 부서에서 상위로, 고정직책은 지정 부서에서). ⛔ **결과는 확정 결재선이 아니라 후보다** — 응답의 `verificationRequired:true`·`warnings`·단계별 `status`(후보1/후보다수/미해결)를 그대로 사용자에게 보여주고 **이름을 확인받은 뒤에** save_approval_line으로 등록할 것. 공석·겸직·대행·직책 라벨 차이(규칙 '사업부장' ↔ 조직 '센터장')·위임전결 개정 때문에 해석이 틀릴 수 있다. 등록 시에는 결재(3000) 노드만 담는다(양식필수 합의자·수신참조·시행자는 상신 때 서버가 자동 병합). 스키마 원본만 보려면 get_approval_line_schema."
    )]
    async fn suggest_approval_line(
        &self,
        Parameters(a): Parameters<SuggestApprovalLineArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::approval_line_suggest::suggest_line(&self.client, &a.doc_type, &a.trip)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "저장된 개인결재라인 목록을 조회한다(eap102A02). 각 항목의 lineId는 read/save에, `_row`는 delete에 사용. 상신 아님(재사용 config)."
    )]
    async fn list_approval_lines(&self) -> Result<CallToolResult, ErrorData> {
        let data = modules::approval_line::list_lines(&self.client)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "개인결재라인 1건의 결재자 구성을 조회한다(eap102A05). 반환 members[]는 등록에 필요한 원본 결재자 객체(user_id/co_id/grade_cd/duty_cd/act_id 등) — 신규 라인 만들 때 이걸 그대로 재사용해 detail_line에 넣으면 전 필드가 채워져 가장 안전하다. (user_id 자체는 org_chart의 empSeq와 동일하므로 새 인물은 org_chart로도 구성 가능 — 이쪽에만 있는 건 grade_cd 같은 표시용 필드.)"
    )]
    async fn read_approval_line(
        &self,
        Parameters(a): Parameters<ReadApprovalLineArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = modules::approval_line::read_line(&self.client, &a.line_id)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "개인결재라인을 생성/수정한다(eap102A10). line_id=0 신규, 기존 id면 수정. detail_line_json은 결재자 객체 JSON 배열(배열 순서=결재 순서, 순서 필드 자동 주입). ⚠️ 이건 재사용 config 저장이지 상신이 아님. 결재자 객체는 read_approval_line(기존 라인)에서 재사용하거나, org_chart로 새로 구성(user_id=empSeq, co_id=\"1000\", grade_cd만 없어 표시용 추정치 허용). 저장 후 read_approval_line로 순서 재확인 권장."
    )]
    async fn save_approval_line(
        &self,
        Parameters(a): Parameters<SaveApprovalLineArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let detail: Vec<serde_json::Value> = serde_json::from_str(&a.detail_line_json)
            .map_err(|e| ErrorData::invalid_params(format!("detail_line_json 파싱 실패(JSON 배열이어야 함): {e}"), None))?;
        let data = modules::approval_line::save_line(
            &self.client,
            a.line_id,
            &a.line_nm,
            a.form_id,
            &a.proc_id,
            detail,
        )
        .await
        .map_err(|e| ErrorData::internal_error(format!("결재라인 저장 실패: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "개인결재라인을 삭제한다(eap102A09). row_json은 list_approval_lines 결과의 `_row` 객체 JSON(⚠️ lineId 숫자 아님)."
    )]
    async fn delete_approval_line(
        &self,
        Parameters(a): Parameters<DeleteApprovalLineArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let row: serde_json::Value = serde_json::from_str(&a.row_json)
            .map_err(|e| ErrorData::invalid_params(format!("row_json 파싱 실패: {e}"), None))?;
        let data = modules::approval_line::delete_line(&self.client, row)
            .await
            .map_err(|e| ErrorData::internal_error(format!("결재라인 삭제 실패: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
