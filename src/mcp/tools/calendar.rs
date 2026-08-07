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
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "기간 내 일정을 조회한다(전체 캘린더). 날짜 YYYYMMDD. \
                       `mine:true`는 **본인이 참석자/작성자인 일정** — 취소·삭제와 무관하다(삭제된 일정은 애초에 목록에 없다). \
                       '내 일정'을 물으면 이 플래그로 거른다."
    )]
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
            .map_err(map_domain_err)?;
        let out = modules::calendar::shape_events(&data);
        Ok(CallToolResult::success(vec![ContentBlock::text(out.to_string())]))
    }

    #[tool(
        description = "일정을 등록한다(등록 후 재조회로 확인). 시각 YYYYMMDDHHmm. calendar 미지정 시 본인 개인 캘린더, 지정 시 그 캘린더(mcalSeq 또는 이름)에 등록 — ⚠️ 공용 캘린더는 다른 사람에게도 보인다. participants로 참여자를 넣으면 그 사람들 일정에도 나타난다(메일 발송은 안 함). secret_memo는 본인만 보는 메모이나 **반영 확인이 불가능**하다. 수정(update_calendar_event)해도 참여자·화상회의는 보존된다."
    )]
    async fn create_calendar_event(
        &self,
        Parameters(a): Parameters<CreateEventArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let specs = a.participants.unwrap_or_default();
        let participants = modules::calendar::resolve_participants(&self.client, &specs)
            .await
            .map_err(map_domain_err)?;
        let extras = modules::calendar::EventExtras {
            my_memo: a.secret_memo.unwrap_or_default(),
            participants,
            video: a.video.as_deref().unwrap_or("N").eq_ignore_ascii_case("Y"),
        };
        let data = modules::calendar::create_event_and_verify(
            &self.client,
            a.calendar.as_deref(),
            &a.title,
            &a.start,
            &a.end,
            &a.contents,
            a.allday.as_deref().unwrap_or("N"),
            &extras,
        )
        .await
        .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "일정의 제목/내용/시간을 수정한다(본인 작성만; 변경분만 지정, 수정 후 재조회 확인). 시각 YYYYMMDDHHmm"
    )]
    async fn update_calendar_event(
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
    async fn delete_calendar_event(
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
