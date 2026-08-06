//! 조직도·본인 정보 도구.
//!
//! 라우터는 `org_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::org`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::{map_domain_err, Amaranth};
use crate::mcp::args::org::*;
use crate::modules;

#[tool_router(router = org_router, vis = "pub(crate)")]
impl Amaranth {
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
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

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
        .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
