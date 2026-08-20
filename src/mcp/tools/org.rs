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
        description = "[아마란스] 사람을 찾는다. **이름·로그인ID·이메일에 더해 조직정보(부서명·부서경로·직책·직급)까지** 부분일치(대소문자 무시)로 훑는다 — \"홍길동\"뿐 아니라 \"네이티브 플랫폼팀\"·\"팀장\"·\"책임연구원\"으로도 찾을 수 있다. 결재선 구성·회의 참석자·메일 수신자에 필요한 empSeq의 진입점. 사람마다 오는 필드 — `empSeq`(결재선·참석자·수신자에 넣는 사원 ID) / `name` / `loginId` / `email` / `mobile`, **조직정보** `deptId`·`deptName`·`deptPath`(이름 경로: 회사>부문>본부>센터>팀)·`duty`(직책: 팀원/팀장/센터장…)·`position`(직급: 책임연구원/부장…), `deptChain`(회사→…→본인 부서를 `{deptId,name}` 배열로 — 이 deptId를 org_chart의 parent_seq/dept_id에 그대로 넣을 수 있다), `note`(부재 표시 — 예: 육아휴직). **query에 숫자만 주면 empSeq 완전일치**로 그 사람을 되짚는다(결재선·일정·예약이 담고 있는 empSeq → 사람). 정렬은 empSeq·이름 완전일치 → 이름 부분일치 → 그 밖의 필드 순. **기본 20명까지만** 반환하고 잘리면 `truncated:true`·`matched`(전체 수)·`notice`가 붙는다 — 더 보려면 `limit`을 올리거나 `no_limit:true`. ⚠️ `dutyCode`도 오지만 **숫자 매핑이 불안정**하니 판단 근거로 쓰지 말 것 — 권위 필드는 `duty`(dutyName)다. ⚠️ 이름에 직책이 붙은 계정이 있다(예: \"홍길동 팀장\") — 완전일치를 전제하지 말 것. ⚠️ 명부는 부서 단위 순회로 조립해 **전사 인원보다 적다**(응답의 `rosterSize`로 확인) — **'명부에 없음'을 '재직하지 않음'으로 단정하지 말 것**. 첫 호출은 명부 조립에 수 초, 이후 30분 캐시."
    )]
    async fn find_person(
        &self,
        Parameters(a): Parameters<FindPersonArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::org::find_person(&self.client, &a.query, a.limit, a.no_limit)
            .await
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "조직도를 조회한다. dept_id 미지정 시 **중첩 부서 트리**(`tree`: 노드마다 deptId/name/gubun/userCount + `children`, 전체 펼침이라 말단팀까지 한 번에 나온다) — 평평한 목록을 받아 조립할 필요가 없다. `flat:true`면 대신 평면 목록(`depts`: path/parentSeq/level 포함)을 주니, 조직 구조를 보려면 트리·조건으로 훑거나 세려면 flat을 쓸 것. parent_seq에 deptId를 주면 그 부서와 하위만 잘라 준다(두 형태 모두 적용). dept_id 지정 시엔 그 부서의 사원+직책(duty=dutyName) 목록. ⚠️ userCount는 하위 부서를 포함한 **누적** 인원이다(부모 − 자식합 = 그 조직 직속). 결재선 직책→담당자 해석용 재료이자 본인 직급(grade) 확인 경로(dept_id=whoami.deptSeq). ⚠️ 직책으로 담당자를 '확정'하지 말고 후보로만 쓸 것(dutyName 권위, dutyCode 숫자 매핑 불안정). ℹ️ 결재라인 등록용 값 중 user_id=여기의 empSeq, co_id=\"1000\" 고정이고 grade_cd(직급코드)만 없다 — 정확한 값이 필요하면 read_approval_line의 기존 결재자 객체를 재사용."
    )]
    async fn org_chart(
        &self,
        Parameters(a): Parameters<OrgChartArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = if a.dept_id.trim().is_empty() {
            if a.flat {
                modules::org::dept_tree_flat(&self.client, &a.parent_seq).await
            } else {
                modules::org::dept_tree_nested(&self.client, &a.parent_seq).await
            }
        } else {
            modules::org::dept_members(&self.client, a.dept_id.trim()).await
        }
        .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
