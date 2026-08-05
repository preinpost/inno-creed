//! MCP 서버: 코어 모듈을 rmcp 도구로 노출.

use std::sync::Arc;

use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData, ServerHandler,
};

use crate::{client::GwClient, modules};

pub mod args;
use args::{
    approval::*, attendance::*, board::*, calendar::*, mail::*, org::*, resource::*, search::*,
};

/// 모듈 에러 → MCP 에러 매핑. **호출자 잘못(`NotOwner`·`InvalidInput`)만 `invalid_params`**로
/// 분류한다 — 서버/네트워크 실패가 아니라 잘못된 대상·인자를 준 것이기 때문이다(리팩터 전 동작 보존).
/// 문자열 매칭이 아니라 타입(`downcast_ref`)으로 판별한다.
fn map_domain_err(e: anyhow::Error) -> ErrorData {
    if e.downcast_ref::<crate::error::NotOwner>().is_some()
        || e.downcast_ref::<crate::error::InvalidInput>().is_some()
    {
        ErrorData::invalid_params(e.to_string(), None)
    } else {
        ErrorData::internal_error(e.to_string(), None)
    }
}






































/// MCP 서버 핸들러. 상태는 `client` 하나뿐이다.
/// ⚠️ **라우터를 필드로 들고 있지 않다.** `#[tool_handler]`(인자 없음)의 기본 동작이
/// `call_tool`/`list_tools` 본문에서 `Self::tool_router()`(정적 생성자)를 매번 호출하는 것이라,
/// 인스턴스 필드에 라우터를 저장해도 **어떤 경로로도 읽히지 않는다**(rmcp-macros 2.2.0 확인).
/// 필드를 두면 "인스턴스가 라우터를 보유한다"는 거짓 신호만 남으므로 제거했다.
/// 훗날 도메인별 라우터를 합성하려면 `#[tool_handler(router = <표현식>)]`로 경로를 명시해야 한다.
#[derive(Clone)]
pub struct Amaranth {
    client: Arc<GwClient>,
}

#[tool_router]
impl Amaranth {
    pub fn new(client: GwClient) -> Self {
        Self {
            client: Arc::new(client),
        }
    }

    /// 세션 정보(gw050A02, 10분 TTL 캐시)를 lazy 보장. 모든 도구 핸들러가 진입 시 호출한다.
    async fn ensure_session(&self) -> Result<(), ErrorData> {
        self.client
            .ensure_session()
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }

    #[tool(description = "회의실(자원) 목록을 조회한다")]
    async fn list_resources(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::resource::list_resources(&self.client)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "메일함(폴더) 목록을 조회한다")]
    async fn list_mailboxes(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::mail::list_mailboxes(&self.client)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "받은메일함 최근 20통을 조회한다")]
    async fn list_inbox(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::mail::list_mails(&self.client, modules::mail::INBOX, 1, 20)
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
        description = "[아마란스] 회의실 빈 시간을 찾는다. 날짜·필요시간(분)·구간·건물을 주면 자원별 예약을 빼고 가능한 구간만 반환. 예: date=20260805, duration_min=120, window=\"0900-1200\", group=\"본사\""
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
        )
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "[아마란스] 본인이 예약한 회의실만 조회한다. 예약 수정·취소에 필요한 seqNum/resIdx를 얻는 경로."
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

    #[tool(
        description = "[아마란스] 로그인한 본인 정보를 반환한다 — empSeq/deptSeq/이메일, 근태용 empCd/deptCd/coCd, 그리고 **부서명·직책(duty)·직급(position)**. '내 예약', '내가 결재할 것' 류 필터의 기준값이자 결재선 grade 판정 근거. 직책·직급은 세션에 없어 조직도(gw102A02)에서 채우며(30분 캐시), 실패 시 `profileResolved:false`와 함께 빈 값이 온다."
    )]
    async fn whoami(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let c = &self.client;
        // 부서명·직책·직급은 세션(gw050A02)에 없어 조직도에서 채운다(1콜, 30분 캐시).
        // 실패해도 resolved:false + 빈 값이라 whoami 자체는 성공한다.
        let prof = modules::org::my_profile(c).await;
        let p = |k: &str| prof.get(k).cloned().unwrap_or(serde_json::Value::Null);
        let info = serde_json::json!({
            "empSeq": c.emp_seq(),          // UC 계열 사원 ID(결재선·참석자·예약자에 사용)
            "empName": c.emp_name(),
            "deptSeq": c.dept_seq(),
            "compSeq": c.comp_seq(),
            "groupSeq": c.group_seq(),
            "email": format!("{}@{}", c.email_addr(), c.email_domain()),
            "empCd": c.emp_cd(),            // ERP(근태) 계열 — UC seq와 별개 체계
            "deptCd": c.dept_cd(),
            "coCd": c.co_cd(),
            // 아래는 조직도(gw102A02) 출처 — 결재선 grade 판정·문서 표시필드에 쓰인다.
            "deptName": p("deptName"),
            "duty": p("duty"),              // 직책(팀원/팀장/센터장…)
            "position": p("position"),      // 직급(책임연구원/부장…)
            "profileResolved": p("resolved")
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(info.to_string())]))
    }

    #[tool(
        description = "[아마란스] 이름·로그인ID·이메일로 사람을 찾아 empSeq/부서/직책/연락처를 반환한다. 결재선 구성·회의 참석자·메일 수신자에 필요한 empSeq의 진입점. 첫 호출은 전사 명부를 조립하느라 수 초 걸리고 이후 30분간 캐시된다."
    )]
    async fn find_person(
        &self,
        Parameters(a): Parameters<FindPersonArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::org::find_person(&self.client, &a.query)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

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
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

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

    #[tool(description = "회의실을 예약한다(등록 후 재조회로 실제 생성 확인). 시각은 YYYYMMDDHHmm")]
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
        description = "회의실 예약을 수정한다(본인 소유만; 변경분만 지정, 수정 후 재조회로 확인). 시각은 YYYYMMDDHHmm"
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

    // ── 일정 도구 ──

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

    // ── 메일 도구 ──

    #[tool(
        description = "메일을 발송한다(2단계: 작성폼 초기화→발송). 받는사람 미지정 시 본인에게. attachments에 로컬 파일 경로를 주면 첨부 발송."
    )]
    async fn send_mail(
        &self,
        Parameters(a): Parameters<SendMailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let to = a.to.clone().unwrap_or_else(|| {
            format!(
                "{} <{}@{}>",
                self.client.emp_name(),
                self.client.email_addr(),
                self.client.email_domain()
            )
        });
        modules::mail::send_mail(&self.client, &to, &a.subject, &a.html, &a.attachments)
            .await
            .map_err(|e| ErrorData::internal_error(format!("메일 발송 실패: {e}"), None))?;
        let msg = serde_json::json!({
            "ok": true,
            "to": to,
            "subject": a.subject,
            "attachments": a.attachments.len(),
            "note": "발송 성공(result:true). 도착 확인은 list_inbox/보낸메일함 재조회 권장"
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(description = "메일을 삭제한다(휴지통 이동). uids=콤마구분 muid. list_inbox의 muid 사용.")]
    async fn delete_mail(
        &self,
        Parameters(a): Parameters<DeleteMailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        modules::mail::delete_mails(&self.client, &a.uids)
            .await
            .map_err(|e| ErrorData::internal_error(format!("메일 삭제 실패: {e}"), None))?;
        let msg = serde_json::json!({
            "ok": true,
            "uids": a.uids,
            "deleted": true,
            "note": "휴지통 이동됨(muid 재부여 — 이후 추적은 재조회 필요)"
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(
        description = "메일 1건의 본문(평문)·헤더·첨부목록을 조회한다. 본문 HTML은 렌더링하지 않고 평문화(외부 이미지 자동로드 안 함, remoteResourceCount로 경고). muid=list_inbox의 muid."
    )]
    async fn read_mail(
        &self,
        Parameters(a): Parameters<ReadMailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = modules::mail::read_mail(&self.client, &a.muid)
            .await
            .map_err(|e| ErrorData::internal_error(format!("메일 조회 실패: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "메일 첨부파일을 다운로드해 out_path에 저장한다(실행하지 않고 저장만). muid+file_sn(read_mail attachments[].fileSn)."
    )]
    async fn download_mail_attachment(
        &self,
        Parameters(a): Parameters<DownloadMailAttachmentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data =
            modules::mail::download_attachment(&self.client, &a.muid, &a.file_sn, &a.out_path)
                .await
                .map_err(|e| ErrorData::internal_error(format!("첨부 다운로드 실패: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    // 게시판 도구는 헤더 인증만으로 완결(companyInfo 불필요) → ensure_session 생략.
    #[tool(
        description = "게시판 최근 공지/게시글 목록을 조회한다(본문 프리뷰 포함). 검색어(field로 제목/내용/작성자 지정)·등록일 범위로 필터 가능."
    )]
    async fn list_notices(
        &self,
        Parameters(a): Parameters<ListNoticesArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = modules::board::list_notices(
            &self.client,
            a.page,
            a.page_size,
            &a.search,
            &a.field,
            &a.start_date,
            &a.end_date,
        )
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "게시글 1건의 본문(평문)·댓글을 조회한다. ⚠️ 호출 시 조회수 증가(실제 열람 처리)."
    )]
    async fn read_notice(
        &self,
        Parameters(a): Parameters<ReadNoticeArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = modules::board::read_post(&self.client, &a.art_seq_no)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "게시글 첨부파일 목록을 조회한다(파일 fileId/이름/확장자/크기). art_seq_no+uid 필요.")]
    async fn list_attachments(
        &self,
        Parameters(a): Parameters<ListAttachmentsArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = modules::board::list_attachments(&self.client, &a.art_seq_no, &a.uid)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "게시글 첨부파일을 다운로드해 out_path에 저장한다. art_seq_no+uid+file_sn(순번).")]
    async fn download_attachment(
        &self,
        Parameters(a): Parameters<DownloadAttachmentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data =
            modules::board::download_attachment(&self.client, &a.art_seq_no, &a.uid, a.file_sn, &a.out_path)
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    // 전자결재 읽기 도구(/eap/*). 목록/상세는 헤더 인증만으로 완결 → ensure_session 생략.
    // 카운트는 companyInfo 필요 → ensure_session 선행.
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

    // 조직도 조회(/gw/APIHandler/gw102A0x). 헤더 인증만으로 완결 → ensure_session 생략.
    #[tool(
        description = "조직도를 조회한다. dept_id 지정 시 그 부서의 사원+직책(duty=dutyName) 목록. dept_id 미지정 시 부서 트리(전체 펼침 — 인사총무팀/인사지원실 등 말단팀까지 나옴); parent_seq로 특정 부서 하위 서브트리만 볼 수도 있음. 결재선 직책→담당자 해석용 재료이자 본인 직급(grade) 확인 경로(dept_id=whoami.deptSeq). ⚠️ 직책으로 담당자를 '확정'하지 말고 후보로만 쓸 것(dutyName 권위, dutyCode 숫자 매핑 불안정). ℹ️ 결재라인 등록용 값 중 user_id=여기의 empSeq, co_id=\"1000\" 고정이고 grade_cd(직급코드)만 없다 — 정확한 값이 필요하면 read_approval_line의 기존 결재자 객체를 재사용."
    )]
    async fn org_chart(
        &self,
        Parameters(a): Parameters<OrgChartArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = if a.dept_id.trim().is_empty() {
            modules::org::dept_tree(&self.client, &a.parent_seq).await
        } else {
            modules::org::dept_members(&self.client, a.dept_id.trim()).await
        }
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    // ── 근태 출퇴근(human 모듈). ⚠️ 실제 근태 기록. empCd/deptCd/coCd 필요 → ensure_session 선행. ──
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

    // 결재선 후보 제안(스키마 + 조직도). ⛔ 확정이 아니라 후보 — 응답에 검증 필요 표시를 싣는다.
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

    // 결재라인 스키마(직책 기반, 번들+override 논리 병합). 로컬 데이터라 세션/인증 불필요.
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
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    // ── 신청 가이드(양식별 draftHelp = submit_approval 기안 데이터 --help + 웹 작성 절차). ──
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
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    // ── 개인결재라인 config CRUD(eap102A0x). "상신 시 재사용할 결재선 저장"이지 상신 자체가 아님. ──
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

    // ── 전자결재 쓰기(상신/상신취소). ⚠️ 실제 결재 발생 — 테스트는 테스트 결재라인으로. ──
    #[tool(
        description = "문서를 상신(제출)한다. ⚠️ 실제 결재요청·수신참조 통지가 나감 — 시험 상신은 본인/합의된 인원만 담은 별도 결재라인으로 하고, 끝나면 `cancel_approval(doc_id, form_id, purge=true)`로 되돌릴 것(상신 직후 문서는 doc_sts=30이라 form_id 필요). ⭐ **hp_application_json / bind_data_json 을 어떻게 채우는지는 `get_submission_guide(양식명 또는 form_id)` 의 `draftHelp` 를 먼저 조회할 것** — 양식별 고정코드(atCd/linkAtCd 등)·의미별 채울 필드·복사용 실동작 예시(hpApplicationExample/bindDataExample)·권장 제목(defaultDocTitle)을 준다(CLI --help 격). 신원은 이 도구가 로그인 사용자 값으로 **자동 주입**한다 — 코드계(coCd/deptCd/empCd)·이름뿐 아니라 **문서에 렌더되는 표시문자열(부서명·직급·직책, `singleDeptNm`/`empNmDutyNm`/`employees` 등)까지** 조직도 값으로 덮어쓰므로 예시값을 그대로 둬도 됨. 결재라인은 `suggest_approval_line`으로 후보를 받아 **사용자 확인 후** save_approval_line으로 등록할 것. 흐름(근태): 0hr00011 → create(appSq 획득) → eap110A03(결재선 병합 + 양식별 form_d_tp 취득) → HP interlock 등록 3콜(GetLinkKey→saveAttendApplicationLinkKey→SetEnageGroup) → eap110A06 상신. **이 interlock 등록이 빠지면 2099(HP_HPD0110_000XX)** — 근태 상신 실패의 사실상 유일한 원인이었다(잔여 draft·날짜·payload 가설은 전부 반증됨). 성공 시 새 docId 반환하나 응답을 성공으로 단정 말고 list_approvals(sent)로 재확인. 실증 범위: 근태 4양식(연차36/출장40/외근41/휴일43) 순수 API 상신·취소 e2e. HP 비연동(비근태) 양식은 hp_application_json 없이 호출하는 경로가 있으나 **미검증**."
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
        .map_err(|e| ErrorData::internal_error(format!("상신 실패: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "상신 문서를 취소한다. 문서 상태(doc_sts)에 따라 결재취소(eap110A54)→상신취소(eap110A18)→(purge시)임시보관삭제(eap110A19)를 순차 실행. ⚠️ doc_sts=30(결재 진행중) 문서는 결재취소가 선행돼야 하며 form_id 필요(list_approvals의 formId). 상신 직후(20)면 form_id 없이 상신취소만. purge=true면 임시보관 문서까지 완전 삭제. 검증: read_approval 2385(임시저장) 또는 approval_counts의 sent 감소, 삭제는 list_approvals(draft)에서 소멸."
    )]
    async fn cancel_approval(
        &self,
        Parameters(a): Parameters<CancelApprovalArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::approval_submit::cancel_approval(
            &self.client,
            a.doc_id.trim(),
            a.form_id.trim(),
            a.purge,
        )
        .await
        .map_err(|e| ErrorData::internal_error(format!("상신취소 실패: {e}"), None))?;
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
            .map_err(|e| ErrorData::internal_error(format!("임시보관 삭제 실패: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}

#[tool_handler]
impl ServerHandler for Amaranth {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.instructions = Some(
            "이노그리드 그룹웨어 **아마란스**(gw.innogrid.com) 도구. \
             회의실·일정·메일·게시판·전자결재·근태·조직도를 다룬다. (Dooray 등 다른 그룹웨어가 아님)\n\
             \n\
             먼저 잡을 도구:\n\
             - 무언가를 '찾아야' 하면 `search` — 메일·결재·게시판·일정·자원·파일을 한 번에 훑고, \
               결과의 ID로 `read_mail`(muid)/`read_approval`(docId+formId)/`read_notice`(artSeqNo)에 바로 이어진다. \
               모듈별 전용 검색 API는 존재하지 않으므로 이것이 유일한 검색 경로다.\n\
             - '언제 회의실이 비나'는 `find_free_rooms`(빈 구간 계산 완료본). 예약 목록을 직접 훑어 계산하지 말 것.\n\
             - 사람의 empSeq가 필요하면 `find_person`, 본인 값은 `whoami`. \
               결재선·참석자·수신자가 전부 empSeq를 요구한다.\n\
             - 내 예약을 고치거나 취소하려면 `my_reservations`로 seqNum/resIdx를 먼저 얻는다.\n\
             \n\
             주의:\n\
             - 부작용 있는 도구 — `clock_in`/`clock_out`(실제 근태 기록), `submit_approval`(결재요청 발송), \
               `send_mail`, `read_notice`(조회수 증가). 사용자가 명시적으로 지시할 때만 호출한다.\n\
             - 회의실 **정원(수용인원) 데이터는 아마란스에 없다**. 'N명 회의실' 조건은 답할 수 없다.\n\
             - 날짜는 YYYYMMDD, 시각은 YYYYMMDDHHmm(입력). 조회 결과의 시각은 ISO로 정규화해 반환한다."
                .to_string(),
        );
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}

/// 도구 표면(이름·개수) 스냅샷.
///
/// `#[tool_router]` 가 만드는 `Self::tool_router()` 는 **private** 이라(rmcp-macros 2.2.0,
/// `vis` 기본값 없음) 이 테스트는 `tests/` 가 아니라 이 파일 안에 있어야 한다.
/// 목적은 커버리지가 아니라 **회귀 기준선**이다 — 파일 분해(`todo/refactor-structure` #09/#10)처럼
/// 코드를 옮기는 작업에서 도구가 사라지거나 이름이 바뀌는 것을 컴파일러가 못 잡기 때문이다.
#[cfg(test)]
mod tests {
    use super::*;

    /// 도구 목록 스냅샷. 의도적으로 도구를 추가/삭제했다면 이 목록을 함께 고치면 된다
    /// (그때 README·docs 도구표도 같이 갱신할 것).
    const EXPECTED_TOOLS: &[&str] = &[
        "approval_counts", "attendance_month", "cancel_approval", "cancel_reservation",
        "clock_in", "clock_out", "create_event", "delete_approval_line", "delete_event",
        "delete_mail", "delete_temp_approval", "download_attachment", "download_mail_attachment",
        "find_free_rooms", "find_person", "get_approval_line_schema", "get_attendance_today",
        "get_submission_guide", "list_approval_line_schemas", "list_approval_lines",
        "list_approvals", "list_attachments", "list_calendars", "list_events", "list_inbox",
        "list_mailboxes", "list_notices", "list_reservations", "list_resources",
        "list_submission_guides", "my_reservations", "org_chart", "pending_approvals",
        "read_approval", "read_approval_line", "read_mail", "read_notice", "reserve_resource",
        "save_approval_line", "search", "send_mail", "submit_approval", "suggest_approval_line",
        "update_event", "update_reservation", "whoami",
    ];

    #[test]
    fn 도구_표면이_스냅샷과_일치한다() {
        let mut names: Vec<String> = Amaranth::tool_router()
            .list_all()
            .iter()
            .map(|t| t.name.to_string())
            .collect();
        names.sort();
        let expected: Vec<String> = EXPECTED_TOOLS.iter().map(|s| s.to_string()).collect();
        assert_eq!(names, expected, "MCP 도구 표면이 변했다");
    }

    /// 라우터 생성이 네트워크·크레덴셜 없이 되는지(=핸들러 구성이 순수한지) 확인.
    /// `GwClient::new(None)` 은 필드 초기화만 한다.
    #[test]
    fn 핸들러는_크레덴셜_없이_만들어진다() {
        let a = Amaranth::new(GwClient::new(None));
        drop(a);
        assert_eq!(Amaranth::tool_router().list_all().len(), EXPECTED_TOOLS.len());
    }
}
