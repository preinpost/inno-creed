//! 자원(회의실 예약) 도구.
//!
//! 라우터는 `resource_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::resource`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::{map_domain_err, Amaranth};
use crate::mcp::args::resource::*;
use crate::modules;

#[tool_router(router = resource_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(description = "회의실(자원) 목록을 조회한다")]
    async fn list_resources(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::resource::list_resources(&self.client)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "기간·자원별 회의실 예약 현황을 조회한다")]
    async fn list_reservations(
        &self,
        Parameters(a): Parameters<ListReservationsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::resource::reservations_view(
            &self.client, &a.start, &a.end, &a.res_seqs, a.verbose,
        )
        .await
        .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "[아마란스] 회의실 빈 시간을 찾는다. 날짜·필요시간(분)·구간·건물을 주면 자원별 예약을 빼고 가능한 구간만 반환. 예: date=20260805, duration_min=120, window=\"0900-1200\", group=\"본사\". ⚠️ **점심시간 13:00~14:00은 예약과 동일하게 점유로 처리해 결과에서 제외**한다(응답의 lunchBreak/note 참조) — 점심시간에도 찾아야 하면 include_lunch=true로 호출한다."
    )]
    async fn find_free_rooms(
        &self,
        Parameters(a): Parameters<FindFreeRoomsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::resource::find_free_slots(
            &self.client,
            &a.date,
            a.duration_min,
            &a.window,
            &a.group,
            a.include_lunch,
        )
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "[아마란스] 본인이 예약한 회의실만 조회한다. 예약 수정·취소에 필요한 seqNum/resIdx를 얻는 경로. title=예약명, displayTitle=아마란스 화면 표시 문구(`[예약자] 자원명`)."
    )]
    async fn my_reservations(
        &self,
        Parameters(a): Parameters<MyReservationsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::resource::my_reservations(&self.client, &a.start, &a.end)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "회의실을 예약한다(등록 후 재조회로 실제 생성 확인). 시각은 YYYYMMDDHHmm. ⚠️ **13:00~14:00은 점심시간** — 예약 구간이 여기 걸치면 응답에 lunchWarning이 실린다. 막지는 않으니 그 경우 사용자에게 의도한 것인지 확인할 것. 응답의 reqText=예약명, displayTitle=아마란스 화면에 실제로 찍히는 문구(`[예약자] 자원명`) — **화면은 예약명을 표시하지 않으므로**, 사용자가 \"예약명이 다르게 보인다\"고 하면 이 차이로 설명할 것.")]
    async fn reserve_resource(
        &self,
        Parameters(a): Parameters<ReserveArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::resource::reserve_and_verify(
            &self.client, &a.res_seq, &a.req_text, &a.start, &a.end, &a.desc,
        )
        .await
        .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "회의실 예약을 수정한다(본인 소유만; 변경분만 지정, 수정 후 재조회로 확인). 시각은 YYYYMMDDHHmm. ⚠️ 바꾼 시간이 **점심시간 13:00~14:00**에 걸치면 응답에 lunchWarning이 실린다 — 사용자에게 확인할 것."
    )]
    async fn update_reservation(
        &self,
        Parameters(a): Parameters<UpdateArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::resource::update_and_verify(
            &self.client,
            &a.res_seq,
            a.seq_num,
            a.res_idx.as_deref(),
            a.req_text.as_deref(),
            a.start.as_deref(),
            a.end.as_deref(),
            a.desc.as_deref(),
        )
        .await
        .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "회의실 예약을 취소한다(본인 소유만; 취소 후 재조회로 확인)")]
    async fn cancel_reservation(
        &self,
        Parameters(a): Parameters<CancelArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::resource::cancel_and_verify(
            &self.client, &a.res_seq, a.seq_num, a.res_idx.as_deref(),
        )
        .await
        .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
