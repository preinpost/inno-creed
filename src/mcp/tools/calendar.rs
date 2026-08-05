//! 일정(캘린더) 도구.
//!
//! 라우터는 `calendar_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::calendar`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::{map_domain_err, Amaranth};
use crate::mcp::args::calendar::*;
use crate::modules;

#[tool_router(router = calendar_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(description = "캘린더(일정 달력) 목록을 조회한다")]
    async fn list_calendars(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::calendar::list_calendars(&self.client)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "기간 내 일정을 조회한다(전체 캘린더). 날짜 YYYYMMDD")]
    async fn list_events(
        &self,
        Parameters(a): Parameters<ListEventsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let cal_list = modules::calendar::all_cal_list(&self.client)
            .await
            .map_err(map_domain_err)?;
        let data = modules::calendar::list_events(&self.client, &a.start, &a.end, cal_list)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "일정을 등록한다(등록 후 재조회로 확인). 시각 YYYYMMDDHHmm. calendar 미지정 시 본인 개인 캘린더, 지정 시 그 캘린더(mcalSeq 또는 이름)에 등록 — ⚠️ 공용 캘린더는 다른 사람에게도 보인다."
    )]
    async fn create_event(
        &self,
        Parameters(a): Parameters<CreateEventArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::calendar::create_event_and_verify(
            &self.client,
            a.calendar.as_deref(),
            &a.title,
            &a.start,
            &a.end,
            &a.contents,
            a.allday.as_deref().unwrap_or("N"),
        )
        .await
        .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "일정의 제목/내용/시간을 수정한다(본인 작성만; 변경분만 지정, 수정 후 재조회 확인). 시각 YYYYMMDDHHmm"
    )]
    async fn update_event(
        &self,
        Parameters(a): Parameters<UpdateEventArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::calendar::update_event_and_verify(
            &self.client,
            &a.sch_seq,
            &a.date,
            a.title.as_deref(),
            a.contents.as_deref(),
            a.start.as_deref(),
            a.end.as_deref(),
        )
        .await
        .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "일정을 삭제한다(본인 작성만; 소프트 삭제 30일 휴지통, 삭제 후 재조회 확인)"
    )]
    async fn delete_event(
        &self,
        Parameters(a): Parameters<DeleteEventArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::calendar::delete_event_and_verify(&self.client, &a.sch_seq, &a.date)
            .await
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
