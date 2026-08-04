//! MCP 서버: 코어 모듈을 rmcp 도구로 노출.

use std::sync::Arc;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router, ErrorData, ServerHandler,
};
use serde::Deserialize;

use crate::{client::GwClient, modules};

/// JSON 값을 인덱스 문자열로. 서버가 resIdx를 number("3")로도 string("3")으로도 준다.
fn json_idx(v: Option<&serde_json::Value>) -> Option<String> {
    v.map(|x| match x {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        other => other.to_string(),
    })
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReserveArgs {
    /// 자원(회의실) ID. list_resources로 확인.
    pub res_seq: String,
    /// 예약명
    pub req_text: String,
    /// 시작 시각 YYYYMMDDHHmm (예: 202608011000)
    pub start: String,
    /// 종료 시각 YYYYMMDDHHmm
    pub end: String,
    /// 내용(선택)
    #[serde(default)]
    pub desc: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CancelArgs {
    /// 자원(회의실) ID
    pub res_seq: String,
    /// 예약 ID (seqNum)
    pub seq_num: i64,
    /// 예약 인덱스 (기본 "1")
    #[serde(default)]
    pub res_idx: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateArgs {
    /// 자원(회의실) ID
    pub res_seq: String,
    /// 예약 ID (seqNum)
    pub seq_num: i64,
    /// 예약 인덱스 (기본 "1")
    #[serde(default)]
    pub res_idx: Option<String>,
    /// 새 예약명 (미지정 시 기존 유지)
    #[serde(default)]
    pub req_text: Option<String>,
    /// 새 시작 YYYYMMDDHHmm (미지정 시 기존 유지)
    #[serde(default)]
    pub start: Option<String>,
    /// 새 종료 YYYYMMDDHHmm (미지정 시 기존 유지)
    #[serde(default)]
    pub end: Option<String>,
    /// 새 내용 (미지정 시 기존 유지)
    #[serde(default)]
    pub desc: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListReservationsArgs {
    /// 시작일 YYYYMMDD
    pub start: String,
    /// 종료일 YYYYMMDD
    pub end: String,
    /// 조회할 자원 ID 목록(비우면 전체 회의실)
    #[serde(default)]
    pub res_seqs: Vec<String>,
    /// true면 서버 원본(74필드, 회의 안건 전문 포함)을 그대로 반환. 기본 false(슬림).
    #[serde(default)]
    pub verbose: bool,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct FindFreeRoomsArgs {
    /// 날짜 YYYYMMDD
    pub date: String,
    /// 필요한 시간(분). 예: 2시간=120
    pub duration_min: i64,
    /// 탐색 구간 HHmm-HHmm (기본 "0900-1800"). 오전만이면 "0900-1200"
    #[serde(default)]
    pub window: String,
    /// 건물/자원종류: ""(전체) | "본사" | "구로" | attrSeq 숫자
    #[serde(default)]
    pub group: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct MyReservationsArgs {
    /// 시작일 YYYYMMDD
    pub start: String,
    /// 종료일 YYYYMMDD
    pub end: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct FindPersonArgs {
    /// 이름·로그인ID·이메일 일부. 예: "홍길동"
    pub query: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct PendingApprovalsArgs {
    /// 조회 건수(기본 20)
    #[serde(default)]
    pub page_size: Option<i64>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SearchArgs {
    /// 검색어
    pub query: String,
    /// 범위: ""/"전체" | "메일" | "결재" | "게시판" | "일정" | "자원" | "파일"
    #[serde(default)]
    pub scope: String,
    /// 모듈당 결과 수(기본 10, 최대 50)
    #[serde(default)]
    pub limit: Option<i64>,
    /// 시작일 YYYY-MM-DD(선택)
    #[serde(default)]
    pub from: String,
    /// 종료일 YYYY-MM-DD(선택)
    #[serde(default)]
    pub to: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AttendanceMonthArgs {
    /// 조회 월 YYYYMM (예: "202608"). start/end를 주면 그쪽이 우선.
    #[serde(default)]
    pub month: String,
    /// 시작일 YYYYMMDD(선택)
    #[serde(default)]
    pub start: String,
    /// 종료일 YYYYMMDD(선택)
    #[serde(default)]
    pub end: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListEventsArgs {
    /// 시작일 YYYYMMDD
    pub start: String,
    /// 종료일 YYYYMMDD
    pub end: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CreateEventArgs {
    /// 일정 제목
    pub title: String,
    /// 시작 시각 YYYYMMDDHHmm (예: 202608071100)
    pub start: String,
    /// 종료 시각 YYYYMMDDHHmm
    pub end: String,
    /// 메모/내용(선택)
    #[serde(default)]
    pub contents: String,
    /// 종일 일정 여부 "Y"/"N" (기본 "N")
    #[serde(default)]
    pub allday: Option<String>,
    /// 등록할 캘린더 — mcalSeq 또는 캘린더 이름(부분 일치). 미지정 시 본인 개인 캘린더.
    /// 공용 캘린더에 등록하려면 list_calendars로 확인 후 지정. ⚠️ 공용은 다른 사람에게도 보인다.
    #[serde(default)]
    pub calendar: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct UpdateEventArgs {
    /// 일정 ID (schSeq). list_events로 확인.
    pub sch_seq: String,
    /// 대상 일정이 걸친 날짜 YYYYMMDD (원본 조회·소유권 확인용)
    pub date: String,
    /// 새 제목 (미지정 시 기존 유지)
    #[serde(default)]
    pub title: Option<String>,
    /// 새 내용 (미지정 시 기존 유지)
    #[serde(default)]
    pub contents: Option<String>,
    /// 새 시작 YYYYMMDDHHmm (미지정 시 기존 유지)
    #[serde(default)]
    pub start: Option<String>,
    /// 새 종료 YYYYMMDDHHmm (미지정 시 기존 유지)
    #[serde(default)]
    pub end: Option<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteEventArgs {
    /// 일정 ID (schSeq). list_events로 확인.
    pub sch_seq: String,
    /// 대상 일정이 걸친 날짜 YYYYMMDD (원본 조회·소유권 확인용)
    pub date: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SendMailArgs {
    /// 받는사람 (표시형 "이름 <email>" 또는 email). 미지정 시 본인에게 발송.
    #[serde(default)]
    pub to: Option<String>,
    /// 제목
    pub subject: String,
    /// 본문 HTML(선택)
    #[serde(default)]
    pub html: String,
    /// 첨부할 로컬 파일 경로 목록(선택, 절대경로). 비우면 첨부 없음.
    #[serde(default)]
    pub attachments: Vec<String>,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteMailArgs {
    /// 삭제할 메일 muid 목록(콤마 구분). list_inbox의 muid 사용.
    pub uids: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReadMailArgs {
    /// 메일 muid. list_inbox 결과의 muid 사용.
    pub muid: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DownloadMailAttachmentArgs {
    /// 메일 muid. read_mail/list_inbox의 muid.
    pub muid: String,
    /// 첨부 fileSn. read_mail attachments[].fileSn 사용.
    pub file_sn: String,
    /// 저장 경로(절대경로 권장). 예: /tmp/attach.png
    pub out_path: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListNoticesArgs {
    /// 페이지 번호(기본 1)
    #[serde(default = "one")]
    pub page: i64,
    /// 페이지 크기(기본 20)
    #[serde(default = "twenty")]
    pub page_size: i64,
    /// 검색어(선택). field로 대상 지정.
    #[serde(default)]
    pub search: String,
    /// 검색 대상(선택): "title"(제목)/"content"(내용)/"author"(작성자). 그 외/미지정은 통합검색.
    #[serde(default)]
    pub field: String,
    /// 등록일 시작(선택, YYYY-MM-DD)
    #[serde(default)]
    pub start_date: String,
    /// 등록일 종료(선택, YYYY-MM-DD)
    #[serde(default)]
    pub end_date: String,
}

fn one() -> i64 {
    1
}
fn twenty() -> i64 {
    20
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReadNoticeArgs {
    /// 게시글 ID(artSeqNo). list_notices 결과의 artSeqNo 사용.
    pub art_seq_no: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListAttachmentsArgs {
    /// 게시글 번호(art_seq_no). list_notices/read_notice 결과의 artSeqNo 사용.
    pub art_seq_no: String,
    /// 게시글 첨부 uid(attachmentUid). list_notices/read_notice 결과의 attachmentUid 사용.
    pub uid: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DownloadAttachmentArgs {
    /// 게시글 번호(art_seq_no). list_attachments와 동일.
    pub art_seq_no: String,
    /// 게시글 첨부 uid(attachmentUid). list_attachments와 동일.
    pub uid: String,
    /// 파일 순번(0-base). list_attachments 결과 배열의 인덱스. 기본 0.
    #[serde(default)]
    pub file_sn: i64,
    /// 저장 경로(절대경로 권장). 예: /tmp/notice.pdf
    pub out_path: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ListApprovalsArgs {
    /// 함: pending(미결)/approved(기결)/approved_ongoing(기결진행)/approved_done(기결종결)/reference(수신참조)/enforcement(시행)/sent(상신)/draft(임시보관). 기본 pending.
    #[serde(default = "box_pending")]
    pub box_name: String,
    /// 페이지 번호(기본 1)
    #[serde(default = "one")]
    pub page: i64,
    /// 페이지 크기(기본 30)
    #[serde(default = "thirty")]
    pub page_size: i64,
    /// 기간 시작(선택, YYYY-MM-DD). 빈값이면 서버 기본 최근 3개월.
    #[serde(default)]
    pub from: String,
    /// 기간 종료(선택, YYYY-MM-DD)
    #[serde(default)]
    pub to: String,
}

fn box_pending() -> String {
    "pending".to_string()
}
fn thirty() -> i64 {
    30
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReadApprovalArgs {
    /// 문서 ID(docId). list_approvals 결과의 docId.
    pub doc_id: String,
    /// 양식 ID(formId). list_approvals 결과의 formId.
    pub form_id: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct OrgChartArgs {
    /// 부서 ID(deptId). 지정하면 그 부서의 **사원+직책 목록**(gw102A02). 비우면 부서 트리.
    #[serde(default)]
    pub dept_id: String,
    /// 트리 조회 시 시작 노드(deptId). 비우면 전사 트리(전체 펼침). 특정 deptId면 그 하위 서브트리. dept_id가 지정되면 무시됨.
    #[serde(default)]
    pub parent_seq: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetApprovalSchemaArgs {
    /// 양식명 또는 form_id. 예: "외근신청", "외근신청서", "41", "연차휴가신청", "출장신청", "휴일주말근무". list_approval_line_schemas로 목록 확인.
    pub doc_type: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct AttendanceTodayArgs {
    /// 조회할 날짜 YYYYMMDD(선택, 비우면 오늘 KST).
    #[serde(default)]
    pub work_dt: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct GetSubmissionGuideArgs {
    /// 양식명 또는 form_id. 예: "외근신청", "외근", "41", "연차휴가신청", "출장신청", "휴일주말근무". list_submission_guides로 목록 확인.
    pub doc_type: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct ReadApprovalLineArgs {
    /// 라인 ID(lineId). list_approval_lines 결과의 lineId 사용.
    pub line_id: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SaveApprovalLineArgs {
    /// 라인 ID. 0이면 신규 생성, 기존 lineId면 수정. 기본 0.
    #[serde(default)]
    pub line_id: i64,
    /// 라인 이름(예: "외근-표준").
    pub line_nm: String,
    /// 양식 ID(formId). 예: 41(외근)/36(연차). get_approval_line_schema/list_approvals의 formId.
    pub form_id: i64,
    /// 결재자 객체 배열의 JSON 문자열. ⚠️ **배열 순서 = 결재 순서**. read_approval_line 결과의 members 객체를 원하는 순서로 담을 것(user_id/co_id/duty_cd/dept_id/grade_cd/grade_nm/act_id 등 필드 포함). act_id 3000=결재/4000=합의. 순서 필드(doc_line_seq/doc_line_m_seq/line_seq)는 자동 주입됨. org_chart로 새 인물을 만들 땐 user_id=empSeq, co_id="1000"이고 grade_cd(직급코드)만 org에 없어 표시용으로 추정치를 넣어도 됨(라우팅 무관).
    pub detail_line_json: String,
    /// 프로세스 ID(기본 "1000" 기본프로세스).
    #[serde(default)]
    pub proc_id: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteApprovalLineArgs {
    /// 삭제할 라인의 행 객체 JSON 문자열. ⚠️ lineId 숫자가 아니라 list_approval_lines 결과의 `_row` 객체를 그대로 넣을 것.
    pub row_json: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SubmitApprovalArgs {
    /// 양식 ID(formId). 41(외근)/36(연차) 등.
    pub form_id: i64,
    /// 문서 제목.
    pub doc_title: String,
    /// 사용할 개인결재라인 ID. 이 라인의 결재자가 결재(3000) 노드로 실림. save_approval_line으로 준비.
    pub line_id: i64,
    /// HP 근태신청 저장(0hr00011) 요청 body JSON. 근태 양식(외근/연차 등)은 상신 전 이 콜로 HP draft를 먼저 만들어야 함(안 하면 eap110A06가 2099로 실패). 형식: `{"applicationList":[{coCd,deptCd,empCd,linkAtCd,atCd,atYm,atDt,startDt,endDt,startTm,endTm,appDyFg,appDy,appTm,taskDc,workTp,atNm,...}],"employeeList":[{empCd,korNm,deptCd,deptNm,divNm}]}` (07 §8.11). 외근 종일=atCd"3101"/linkAtCd"3010". 빈 문자열이면 이 단계 생략(비근태 양식).
    pub hp_application_json: String,
    /// KISS 폼 본문 데이터 JSON 텍스트. 외근=`{"ITEMS":{...},"TABLE":{"dbTable1":{...},"dbTable2":{"group":[]}}}` (07 문서 §8.10 참조). 서버엔 이중인코딩되어 전송됨.
    pub bind_data_json: String,
    /// 표시용 본문 HTML(raw). 내부에서 encodeURIComponent로 인코딩해 전송.
    pub doc_contents_html: String,
    /// 채번 규칙 ID(기본 "1001").
    #[serde(default)]
    pub numbering_id: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CancelApprovalArgs {
    /// 상신취소할 문서의 docId(list_approvals/read_approval의 docId).
    pub doc_id: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteTempApprovalArgs {
    /// 삭제할 임시보관 문서 docId. 여러 건은 콤마구분(예 "140764,140716"). list_approvals(box_name:"draft")의 docId.
    pub doc_ids: String,
}

#[derive(Clone)]
pub struct Amaranth {
    client: Arc<GwClient>,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Amaranth {
    pub fn new(client: GwClient) -> Self {
        Self {
            client: Arc::new(client),
            tool_router: Self::tool_router(),
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
        // res_seqs 비면 전체 회의실 자동 채움
        let seqs: Vec<String> = if a.res_seqs.is_empty() {
            let resources = modules::resource::list_resources(&self.client)
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
            resources
                .get("resultList")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|r| r.get("resSeq").and_then(|s| s.as_str()).map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            a.res_seqs
        };
        let refs: Vec<&str> = seqs.iter().map(|s| s.as_str()).collect();
        let data = modules::resource::list_reservations(&self.client, &a.start, &a.end, &refs)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        // 기본은 슬림(원본은 74필드 + 회의 안건 전문까지 실려 와 토큰을 크게 먹는다).
        let out = if a.verbose {
            data
        } else {
            let rows: Vec<serde_json::Value> = data
                .get("resultList")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().map(modules::resource::slim_reservation).collect())
                .unwrap_or_default();
            serde_json::json!({
                "period": format!("{}~{}", a.start, a.end),
                "count": rows.len(),
                "reservations": rows
            })
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(out.to_string())]))
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
        description = "[아마란스] 로그인한 본인 정보(empSeq/부서/이메일 및 근태용 empCd 등)를 반환한다. '내 예약', '내가 결재할 것' 류 필터의 기준값."
    )]
    async fn whoami(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let c = &self.client;
        let info = serde_json::json!({
            "empSeq": c.emp_seq(),          // UC 계열 사원 ID(결재선·참석자·예약자에 사용)
            "empName": c.emp_name(),
            "deptSeq": c.dept_seq(),
            "compSeq": c.comp_seq(),
            "groupSeq": c.group_seq(),
            "email": format!("{}@{}", c.email_addr(), c.email_domain()),
            "empCd": c.emp_cd(),            // ERP(근태) 계열 — UC seq와 별개 체계
            "deptCd": c.dept_cd(),
            "coCd": c.co_cd()
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
        let reg = modules::resource::create_reservation(
            &self.client,
            &a.res_seq,
            &a.req_text,
            &a.start,
            &a.end,
            &a.desc,
        )
        .await
        .map_err(|e| ErrorData::internal_error(format!("예약 등록 실패: {e}"), None))?;

        let seq_num = reg.get("seqNum").and_then(|v| v.as_i64()).ok_or_else(|| {
            ErrorData::internal_error("등록 응답에 seqNum 없음", None)
        })?;
        let res_idx = json_idx(reg.get("resIdx")).unwrap_or_else(|| "1".to_string());

        // read-back: 실제 생성·반영 확인
        let detail =
            modules::resource::get_reservation(&self.client, &a.res_seq, seq_num, &res_idx)
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("등록 후 재조회 실패: {e}"), None)
                })?;
        let reflected = detail.get("reqText").and_then(|v| v.as_str()) == Some(a.req_text.as_str());

        let msg = serde_json::json!({
            "ok": reflected,
            "seqNum": seq_num,
            "resIdx": res_idx,
            "resSeq": a.res_seq,
            "reqText": a.req_text,
            "period": format!("{}~{}", a.start, a.end),
            "verified_by_readback": reflected
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(
        description = "회의실 예약을 수정한다(본인 소유만; 변경분만 지정, 수정 후 재조회로 확인). 시각은 YYYYMMDDHHmm"
    )]
    async fn update_reservation(
        &self,
        Parameters(a): Parameters<UpdateArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let res_idx = a.res_idx.as_deref().unwrap_or("1");
        // 현재 스냅샷 + 소유권 확인
        let detail =
            modules::resource::get_reservation(&self.client, &a.res_seq, a.seq_num, res_idx)
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("예약 조회 실패(없거나 접근불가): {e}"), None)
                })?;
        let owner = detail
            .get("empSeq")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if owner != self.client.emp_seq() {
            return Err(ErrorData::invalid_params(
                format!(
                    "본인 소유 예약이 아니라 수정할 수 없습니다 (소유자 empSeq={owner}, 본인={})",
                    self.client.emp_seq()
                ),
                None,
            ));
        }
        let get = |k: &str| detail.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let orig_start = get("startDate"); // YYYYMMDDHHmm (원본 = 식별키 startDatePk)
        let create_date = get("createDate");
        let res_name = get("resName");

        // 변경분만 덮어쓰기, 나머지 기존값 유지
        let new_req = a.req_text.clone().unwrap_or_else(|| get("reqText"));
        let new_start = a.start.clone().unwrap_or_else(|| orig_start.clone());
        let new_end = a.end.clone().unwrap_or_else(|| get("endDate"));
        let new_desc = a.desc.clone().unwrap_or_else(|| get("descText"));

        let upd = modules::resource::update_reservation(
            &self.client,
            &a.res_seq,
            a.seq_num,
            res_idx,
            &new_req,
            &new_start,
            &new_end,
            &new_desc,
            &orig_start,
            &create_date,
            &res_name,
        )
        .await
        .map_err(|e| ErrorData::internal_error(format!("예약 수정 실패: {e}"), None))?;

        // ⚠️ 시간 변경 시 예약이 재발급되어 seqNum/resIdx가 바뀐다. 응답의 새 ID를 사용.
        let new_seq = upd.get("seqNum").and_then(|v| v.as_i64()).unwrap_or(a.seq_num);
        let new_idx = json_idx(upd.get("resIdx")).unwrap_or_else(|| res_idx.to_string());

        // read-back: 새 seqNum으로 실제 반영 확인
        let after =
            modules::resource::get_reservation(&self.client, &a.res_seq, new_seq, &new_idx)
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("수정 후 재조회 실패: {e}"), None)
                })?;
        let ag = |k: &str| after.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let reflected =
            ag("startDate") == new_start && ag("endDate") == new_end && ag("reqText") == new_req;

        let msg = serde_json::json!({
            "ok": reflected,
            "seqNum": new_seq,
            "resIdx": new_idx,
            "prev_seqNum": a.seq_num,
            "reissued": new_seq != a.seq_num,
            "reqText": ag("reqText"),
            "period": format!("{}~{}", ag("startDate"), ag("endDate")),
            "verified_by_readback": reflected
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(description = "회의실 예약을 취소한다(본인 소유만; 취소 후 재조회로 확인)")]
    async fn cancel_reservation(
        &self,
        Parameters(a): Parameters<CancelArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let res_idx = a.res_idx.as_deref().unwrap_or("1");
        // 스냅샷 + 소유권 확인
        let detail =
            modules::resource::get_reservation(&self.client, &a.res_seq, a.seq_num, res_idx)
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("예약 조회 실패(없거나 접근불가): {e}"), None)
                })?;
        let owner = detail
            .get("empSeq")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if owner != self.client.emp_seq() {
            return Err(ErrorData::invalid_params(
                format!(
                    "본인 소유 예약이 아니라 취소할 수 없습니다 (소유자 empSeq={owner}, 본인={})",
                    self.client.emp_seq()
                ),
                None,
            ));
        }
        let get = |k: &str| detail.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        modules::resource::delete_reservation(
            &self.client,
            &a.res_seq,
            a.seq_num,
            res_idx,
            &get("reqText"),
            &get("startDate"),
            &get("endDate"),
            &get("createDate"),
            &get("resName"),
        )
        .await
        .map_err(|e| ErrorData::internal_error(format!("예약 취소 실패: {e}"), None))?;

        // read-back: 조회 시 실패(=삭제됨)여야 정상
        let gone = modules::resource::get_reservation(&self.client, &a.res_seq, a.seq_num, res_idx)
            .await
            .is_err();
        let msg = serde_json::json!({
            "ok": gone,
            "seqNum": a.seq_num,
            "canceled": true,
            "verified_by_readback": gone
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    // ── 일정 헬퍼(비-도구) ──

    /// 내가 볼 수 있는 전체 캘린더를 sc111A03 `calList` 형식으로 구성.
    async fn all_cal_list(&self) -> Result<Vec<serde_json::Value>, ErrorData> {
        let cals = modules::calendar::calendars(&self.client)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        Ok(cals
            .iter()
            .map(|c| {
                serde_json::json!({
                    "mcalSeq": c.mcal_seq,
                    "calType": if c.cal_type.is_empty() { "E" } else { &c.cal_type },
                    "adminYn": "Y",
                    "color": c.color
                })
            })
            .collect())
    }

    /// 등록 대상 캘린더 결정: 지정 없으면 본인 개인 캘린더, 지정하면 mcalSeq/이름으로 해석.
    /// 등록 권한(insertRwGbn)이 없는 캘린더는 선택 가능한 목록과 함께 명시적 에러.
    async fn resolve_target_cal(
        &self,
        key: Option<&str>,
    ) -> Result<modules::calendar::Calendar, ErrorData> {
        let cals = modules::calendar::calendars(&self.client)
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        let writable = || {
            cals.iter()
                .filter(|c| c.can_insert)
                .map(|c| format!("{}({})", c.title, c.mcal_seq))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let cal = match key {
            Some(k) => modules::calendar::find(&cals, k).ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("'{k}' 에 해당하는 캘린더 없음. 등록 가능: {}", writable()),
                    None,
                )
            })?,
            None => modules::calendar::personal(&cals, &self.client.emp_seq()).ok_or_else(|| {
                ErrorData::internal_error(
                    format!("본인 개인 캘린더를 찾지 못했습니다. 등록 가능: {}", writable()),
                    None,
                )
            })?,
        };
        if !cal.can_insert {
            return Err(ErrorData::invalid_params(
                format!(
                    "'{}' 캘린더는 일정 등록 권한이 없습니다. 등록 가능: {}",
                    cal.title,
                    writable()
                ),
                None,
            ));
        }
        Ok(cal.clone())
    }

    /// 특정 날짜(YYYYMMDD)의 일정 목록에서 schSeq 매칭 원본 이벤트를 찾는다.
    async fn find_event(&self, sch_seq: &str, date: &str) -> Result<serde_json::Value, ErrorData> {
        let cal_list = self.all_cal_list().await?;
        let events = modules::calendar::list_events(&self.client, date, date, cal_list)
            .await
            .map_err(|e| ErrorData::internal_error(format!("일정 조회 실패: {e}"), None))?;
        events
            .get("resultList")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|e| e.get("schSeq").and_then(|v| v.as_str()) == Some(sch_seq))
                    .cloned()
            })
            .ok_or_else(|| {
                ErrorData::invalid_params(
                    format!("일정을 찾을 수 없음 (schSeq={sch_seq}, date={date})"),
                    None,
                )
            })
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
        let cal_list = self.all_cal_list().await?;
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
        let allday = a.allday.as_deref().unwrap_or("N");
        let target = self.resolve_target_cal(a.calendar.as_deref()).await?;
        let reg = modules::calendar::upsert_event(
            &self.client,
            "",
            &target.mcal_seq,
            &target.cal_type,
            &a.title,
            &a.start,
            &a.end,
            &a.contents,
            allday,
        )
        .await
        .map_err(|e| ErrorData::internal_error(format!("일정 등록 실패: {e}"), None))?;
        let sch_seq = reg
            .get("schSeq")
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| ErrorData::internal_error("등록 응답에 schSeq 없음", None))?;

        // read-back: 시작일 기준 재조회로 실제 생성 확인
        let day = &a.start[..a.start.len().min(8)];
        let ev = self.find_event(&sch_seq, day).await.ok();
        let reflected = ev
            .as_ref()
            .map(|e| e.get("schTitle").and_then(|v| v.as_str()) == Some(a.title.as_str()))
            .unwrap_or(false);
        let msg = serde_json::json!({
            "ok": reflected,
            "schSeq": sch_seq,
            "title": a.title,
            "calendar": format!("{}({})", target.title, target.mcal_seq),
            "period": format!("{}~{}", a.start, a.end),
            "verified_by_readback": reflected
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(
        description = "일정의 제목/내용/시간을 수정한다(본인 작성만; 변경분만 지정, 수정 후 재조회 확인). 시각 YYYYMMDDHHmm"
    )]
    async fn update_event(
        &self,
        Parameters(a): Parameters<UpdateEventArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        if a.title.is_none() && a.contents.is_none() && a.start.is_none() && a.end.is_none() {
            return Err(ErrorData::invalid_params(
                "변경할 항목(title/contents/start/end)을 하나 이상 지정하세요.",
                None,
            ));
        }
        let orig = self.find_event(&a.sch_seq, &a.date).await?;
        let g = |k: &str| orig.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let owner = g("createSeq");
        if owner != self.client.emp_seq() {
            return Err(ErrorData::invalid_params(
                format!(
                    "본인 작성 일정이 아니라 수정할 수 없습니다 (작성자 empSeq={owner}, 본인={})",
                    self.client.emp_seq()
                ),
                None,
            ));
        }
        let mcal = g("mcalSeq");

        // 변경분만 itemList로 (실측 item 형식)
        let mut items = Vec::new();
        if let Some(t) = &a.title {
            items.push(serde_json::json!({ "item": "schTitle", "schTitle": t }));
        }
        if let Some(c) = &a.contents {
            // ⚠️ 내용 item은 item명(schContents)과 값 필드명(contents)이 다름(실측).
            items.push(serde_json::json!({ "item": "schContents", "contents": c }));
        }
        // 시간 변경: {item:"schDate", schDate:{startDate,endDate,allDay,lunar,lunarDate}} (실측)
        let new_start = a.start.clone().unwrap_or_else(|| g("startDate"));
        let new_end = a.end.clone().unwrap_or_else(|| g("endDate"));
        if a.start.is_some() || a.end.is_some() {
            let allday = {
                let ad = g("alldayYn");
                if ad.is_empty() { "N".to_string() } else { ad }
            };
            items.push(serde_json::json!({
                "item": "schDate",
                "schDate": {
                    "startDate": new_start,
                    "endDate": new_end,
                    "allDay": allday,
                    "lunar": "N",
                    "lunarDate": ""
                }
            }));
        }

        modules::calendar::update_event_items(&self.client, &a.sch_seq, &mcal, items)
            .await
            .map_err(|e| ErrorData::internal_error(format!("일정 수정 실패: {e}"), None))?;

        // read-back: schSeq 유지(in-place). 지정한 필드가 반영됐는지 확인.
        let after = self.find_event(&a.sch_seq, &a.date).await.ok();
        let reflected = after
            .as_ref()
            .map(|e| {
                let eq = |k: &str, v: &Option<String>| {
                    v.as_ref()
                        .map(|x| e.get(k).and_then(|v| v.as_str()) == Some(x.as_str()))
                        .unwrap_or(true)
                };
                eq("schTitle", &a.title)
                    && eq("contents", &a.contents)
                    && eq("startDate", &a.start)
                    && eq("endDate", &a.end)
            })
            .unwrap_or(false);
        let ag = |k: &str| {
            after
                .as_ref()
                .and_then(|e| e.get(k).and_then(|v| v.as_str()))
                .unwrap_or("")
                .to_string()
        };
        let msg = serde_json::json!({
            "ok": reflected,
            "schSeq": a.sch_seq,
            "title": ag("schTitle"),
            "period": format!("{}~{}", ag("startDate"), ag("endDate")),
            "verified_by_readback": reflected
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(
        description = "일정을 삭제한다(본인 작성만; 소프트 삭제 30일 휴지통, 삭제 후 재조회 확인)"
    )]
    async fn delete_event(
        &self,
        Parameters(a): Parameters<DeleteEventArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let orig = self.find_event(&a.sch_seq, &a.date).await?;
        let g = |k: &str| orig.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let owner = g("createSeq");
        if owner != self.client.emp_seq() {
            return Err(ErrorData::invalid_params(
                format!(
                    "본인 작성 일정이 아니라 삭제할 수 없습니다 (작성자 empSeq={owner}, 본인={})",
                    self.client.emp_seq()
                ),
                None,
            ));
        }
        let mcal = g("mcalSeq");
        modules::calendar::delete_event(&self.client, &mcal, &a.sch_seq, "")
            .await
            .map_err(|e| ErrorData::internal_error(format!("일정 삭제 실패: {e}"), None))?;

        // read-back: 목록에서 사라졌으면 성공
        let gone = self.find_event(&a.sch_seq, &a.date).await.is_err();
        let msg = serde_json::json!({
            "ok": gone,
            "schSeq": a.sch_seq,
            "deleted": true,
            "verified_by_readback": gone
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
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
        description = "전자결재 함별 문서 목록을 조회한다. box_name=pending(미결)/approved(기결)/approved_ongoing/approved_done/reference(수신참조)/enforcement(시행)/sent(상신)/draft(임시보관). draft는 상신 안 된 임시저장·상신취소 복귀 문서 — 같은 form_id의 draft가 남아있으면 신규 상신이 막힐 수 있어, 상신 실패/미반영 시 여기서 확인 후 delete_temp_approval로 정리."
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
        description = "조직도를 조회한다. dept_id 지정 시 그 부서의 사원+직책(duty=dutyName) 목록. dept_id 미지정 시 부서 트리(전체 펼침 — 인사총무팀/인사지원실 등 말단팀까지 나옴); parent_seq로 특정 부서 하위 서브트리만 볼 수도 있음. 결재선 직책→담당자 해석용 재료. ⚠️ 직책으로 담당자를 '확정'하지 말고 후보로만 쓸 것(dutyName 권위, dutyCode 숫자 매핑 불안정). ⚠️ 결재라인 등록용 user_id/co_id/grade_cd는 여기 없음 — 그건 read_approval_line에서 얻을 것."
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

    // 결재라인 스키마(직책 기반, 번들+override 논리 병합). 로컬 데이터라 세션/인증 불필요.
    #[tool(
        description = "문서 종류별 결재라인 스키마(직책 기반)를 조회한다. 반환된 line[]은 act(결재/합의)+pos(직책)만 담는다 — 각 pos를 org_chart로 담당자(후보)로 해석한 뒤 상신 전 사람 확인 필수. ⛔ 서버 자동 결재선 신뢰 금지. 현재 근태 계열(외근/연차/출장/휴일근무)만 수록. 버전은 출처 위임전결 PDF 날짜, 사용자 override(~/.config/inno-creed/approval_line.json)가 더 최신이면 그쪽 사용."
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

    // ── 신청 가이드(양식별 본문 필수항목 + 상신 절차). 본문/상신은 MCP 미자동화 → 사람 안내용. ──
    #[tool(
        description = "양식별 '신청 가이드'를 반환한다 — 문서 본문에 채워야 할 필수항목(requiredBody)·상신 절차(steps)·주의(notes)·결재라인 힌트. ⚠️ 근태 양식 본문 입력과 최종 상신은 아직 MCP로 자동화 안 됨(KISS 근태폼 React·상신 API 미캡처) → 이 가이드대로 사람이 아마란스에서 직접 작성. 결재라인만 get_approval_line_schema+save_approval_line로 준비 가능."
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
        description = "개인결재라인 1건의 결재자 구성을 조회한다(eap102A05). 반환 members[]는 등록에 필요한 원본 결재자 객체(user_id/co_id/grade_cd/duty_cd/act_id 등) — 신규 라인 만들 때 이걸 재사용해 detail_line에 넣는다. ⭐ org_chart에 없는 user_id 매핑의 출처."
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
        description = "문서를 상신(제출)한다(근태 2-phase: 0hr00011 HP저장 → eap110A06 상신). ⚠️ 실제 결재요청·수신참조 통지가 나감 — 테스트는 반드시 테스트 결재라인으로 하고 끝나면 cancel_approval로 취소. 흐름: hp_application_json으로 HP 근태 draft 저장 → approkey 발급 → eap110A03(양식필수 합의자/수신참조 자동해석) → line_id 결재자 병합 → 상신. hp_application_json(0hr00011 body)·bind_data_json(eap110A06 본문, 07 §8.10)·doc_contents_html(표시 HTML)은 호출부가 제공. 성공 시 새 docId 반환."
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
        description = "상신된 문서를 상신취소한다(eap110A98 사전조회 + eap110A18 실행). 문서는 임시보관으로 복귀하고 채번 삭제. read_approval이 2385(임시저장) 반환 또는 approval_counts의 sent 감소로 확인."
    )]
    async fn cancel_approval(
        &self,
        Parameters(a): Parameters<CancelApprovalArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::approval_submit::cancel_approval(&self.client, a.doc_id.trim())
            .await
            .map_err(|e| ErrorData::internal_error(format!("상신취소 실패: {e}"), None))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "임시보관 전자결재 문서를 삭제한다(eap107A25). doc_ids는 콤마구분 docId. ⚠️ 실제 삭제(복구 불가). 같은 form_id의 잔여 임시보관 문서가 신규 상신을 막을 때(상신 후 sent 목록에 안 뜰 때) list_approvals(box_name:\"draft\")로 확인 후 정리하는 용도. 삭제 후 draft 재조회로 검증."
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
        info.instructions =
            Some("아마란스(gw.innogrid.com) 그룹웨어 도구. 자원/메일 등 조회.".to_string());
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
}
