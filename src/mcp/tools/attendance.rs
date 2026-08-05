//! 근태 도구.
//!
//! 라우터는 `attendance_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::attendance`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::Amaranth;
use crate::mcp::args::attendance::*;
use crate::modules;

#[tool_router(router = attendance_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(
        description = "[아마란스] 기간(월) 근태 현황을 조회한다. 일자별 출퇴근·근무시간·지각/연차 등 + 기간 합계. month=\"202608\" 또는 start/end(YYYYMMDD)."
    )]
    async fn attendance_month(
        &self,
        Parameters(a): Parameters<AttendanceMonthArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let (start, end) = if !a.start.trim().is_empty() && !a.end.trim().is_empty() {
            (a.start.clone(), a.end.clone())
        } else {
            modules::attendance::month_range(a.month.trim())
                .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
        };
        let data = modules::attendance::work_time_status(&self.client, &start, &end)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "오늘(또는 지정일)의 출퇴근 현황을 조회한다(읽기, 부작용 없음). comeTm(출근)/leaveTm(퇴근) YYYYMMDDHHmm, 빈값=미등록."
    )]
    async fn get_attendance_today(
        &self,
        Parameters(a): Parameters<AttendanceTodayArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let wd = if a.work_dt.trim().is_empty() {
            modules::attendance::today_kst()
        } else {
            a.work_dt.trim().to_string()
        };
        let data = modules::attendance::today(&self.client, &wd)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "출근(clock in)을 기록한다(attendFg 1). ⚠️ 실제 근태 punch — 실제 출근 시점에 사용자가 명시 지시할 때만. 이미 출근 기록(comeTm)이 있으면 재기록 안 함(덮어쓰기 방지). punch 후 read-back(comeTm)으로 확인."
    )]
    async fn clock_in(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::attendance::punch_and_verify(&self.client, "1")
            .await
            .map_err(|e| ErrorData::internal_error(format!("출근 기록 실패: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "퇴근(clock out)을 기록한다(attendFg 4). ⚠️ 실제 근태 punch — 실제 퇴근 시점에 사용자가 명시 지시할 때만. 이미 퇴근 기록(leaveTm)이 있으면 재기록 안 함. punch 후 read-back(leaveTm)으로 확인."
    )]
    async fn clock_out(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::attendance::punch_and_verify(&self.client, "4")
            .await
            .map_err(|e| ErrorData::internal_error(format!("퇴근 기록 실패: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
