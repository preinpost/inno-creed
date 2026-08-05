//! 전자결재 도구 인자 스키마.
//!
//! ⚠️ **이 파일의 doc comment는 그대로 LLM에게 전달된다** — MCP 도구 스키마의 `description`이 되어
//! 모델이 인자를 채우는 유일한 근거가 된다. 문구 변경은 주석 수정이 아니라 **동작 변경**이다.

use serde::Deserialize;
use super::{box_pending, one, thirty};


#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct PendingApprovalsArgs {
    /// 조회 건수(기본 20)
    #[serde(default)]
    pub page_size: Option<i64>,
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
pub struct GetApprovalSchemaArgs {
    /// 양식명 또는 form_id. 예: "외근신청", "외근신청서", "41", "연차휴가신청", "출장신청", "휴일주말근무". list_approval_line_schemas로 목록 확인.
    pub doc_type: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct SuggestApprovalLineArgs {
    /// 양식명 또는 form_id. 예: "외근신청", "41", "연차휴가신청", "출장신청", "휴일주말근무".
    pub doc_type: String,
    /// 출장신청 전용 — "국내" 또는 "해외"(결재선이 갈린다). 다른 양식은 비워둘 것. 빈값이면 해당 양식의 국내·해외 branch를 모두 반환한다.
    #[serde(default)]
    pub trip: String,
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
    /// 문서 제목. ⭐ 양식별 권장 형식은 `get_submission_guide(form_id).draftHelp.defaultDocTitle`/`titleHelp`(예 연차 `[휴가신청] 00/00 오후반차_홍길동(인사&총무팀)`). 사내 관례이므로 사용자 확인 후 확정할 것.
    pub doc_title: String,
    /// 사용할 개인결재라인 ID(save_approval_line으로 준비). 이 라인의 결재자는 eap110A03가 **양식필수 합의자·수신참조·시행자와 병합**해 돌려주고, 그 병합 결과가 그대로 결재선으로 실린다. 즉 라인에는 **결재(3000)만** 담으면 되고 양식필수 합의자를 또 넣으면 중복될 수 있음.
    pub line_id: i64,
    /// HP 근태신청 저장 요청 body JSON(0hr00011 + create 두 콜에 쓰임). **근태 양식 전용** — 이걸 넘기면 상신 전에 HP 신청 레코드 생성 + interlock 등록(GetLinkKey→saveAttendApplicationLinkKey→SetEnageGroup)까지 수행한다. ⭐ **채우는 법·양식별 고정코드·복사용 예시는 `get_submission_guide(form_id).draftHelp.hpApplicationExample`**(예: 출장 linkAtCd"2010"/atCd"2101", 외근 종일 atCd"3101"/linkAtCd"3010"). 신원 필드(coCd/deptCd/empCd/empNm/korNm)는 **submit_approval이 로그인 사용자 값으로 자동 덮어씀** — 예시값 그대로 둬도 됨. 형식: `{"applicationList":[{...,linkAtCd,atCd,atDt,startDt,endDt,startTm,endTm,appDyFg,appDy,appTm,...}],"employeeList":[{...}]}`. 빈 문자열이면 이 단계 전체 생략(= 비근태 양식 경로, 아직 미검증).
    pub hp_application_json: String,
    /// 폼 본문 데이터 JSON 텍스트. `{"ITEMS":{...},"TABLE":{"dbTable1":{...},"dbTable2":{...}}}`. ⭐ **양식별 예시는 `get_submission_guide(form_id).draftHelp.bindDataExample`**. 실제 결재문서에 렌더되는 값이 이것(doc_contents_html이 아님). 서버엔 이중인코딩되어 전송됨.
    pub bind_data_json: String,
    /// 표시용 본문 HTML(raw). 내부에서 encodeURIComponent로 인코딩해 전송. 근태 양식은 본문이 bindData/HP연동으로 채워지므로 **한 줄 요약 HTML(예 `<div>2026-12-16 종일외근</div>`)로도 상신이 통과**한다(4양식 실증). 브라우저는 양식 표 전체를 조립해 보내므로, 문서 뷰 표시 품질까지 맞추려면 표 HTML이 필요(미검증). 빈 문자열 가능 여부는 미확인.
    pub doc_contents_html: String,
    /// 채번 규칙 ID. 빈 문자열이면 "1001"(기본 채번)이 자동 적용된다 — 보통 그대로 두면 됨.
    #[serde(default)]
    pub numbering_id: String,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct CancelApprovalArgs {
    /// 취소할 문서의 docId(list_approvals/read_approval의 docId).
    pub doc_id: String,
    /// form_id(list_approvals의 formId). doc_sts=30(결재 진행중) 문서의 결재취소(eap110A54)에 필요. 상신 직후(20) 문서만 취소할 땐 생략 가능.
    #[serde(default)]
    pub form_id: String,
    /// true면 결재취소→상신취소 후 임시보관 문서까지 완전 삭제(eap110A19). false(기본)면 임시보관에 남긴다.
    #[serde(default)]
    pub purge: bool,
}

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars")]
pub struct DeleteTempApprovalArgs {
    /// 삭제할 임시보관 문서 docId. 여러 건은 콤마구분(예 "140764,140716"). list_approvals(box_name:"draft")의 docId.
    pub doc_ids: String,
}
