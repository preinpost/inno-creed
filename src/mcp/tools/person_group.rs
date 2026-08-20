//! 사람 그룹 도구 — `~/.config/inno-creed/person_groups.json` 을 읽어 준다.
//!
//! 라우터는 `person_group_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//!
//! 도구 3개 — `person_group`(목록·조회) · `save_person_group`(생성·수정) · `delete_person_group`.
//!
//! ⚠️ **쓰기는 이 파일이 유일한 경로다.** 사용자가 같은 파일을 손으로 편집하기도 하므로
//! 모듈이 "읽고 → 고치고 → 통째로 다시 쓰기"로 다른 그룹을 보존한다(`modules::person_group`).

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::args::person_group::*;
use crate::mcp::{map_domain_err, Amaranth};
use crate::modules;

#[tool_router(router = person_group_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(
        description = "자주 함께 지정하는 사람들의 **그룹**을 조회한다(`~/.config/inno-creed/person_groups.json`). 아마란스에 그룹메일이 없어서 두는 것이고, 메일 전용이 아니라 **범용**이다 — 메일 수신자·참조, 캘린더 참여자, 결재선 어디에나 쓴다. name을 비우면 그룹 목록(이름/인원수/메모), 주면 그 그룹의 멤버를 조직도 명부로 풀어서 준다: `members[]`(empSeq/name/email/dept/duty/status) + 바로 쓸 재료인 `empSeqs`(→ create_calendar_event의 participants, save_approval_line의 user_id)와 `emails`(→ **콤마로 이어** send_mail의 to/cc/bcc). ⚠️ 조립은 호출자가 한다 — 소비처마다 모양이 달라 도구가 미리 이어 붙이지 않는다. ⚠️ 명부에서 못 찾은 사람은 `status:\"not_found\"` 로 남고 `missing`에 실리며 **emails에서 빠진다** — 그대로 보내면 그 사람만 누락되니 사용자에게 알릴 것. 그룹을 만들거나 고치려면 응답의 `path` 파일을 직접 편집한다(이 도구는 쓰지 않는다). 멤버의 정본은 empSeq이며 find_person으로 얻는다."
    )]
    async fn person_group(
        &self,
        Parameters(a): Parameters<PersonGroupArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let name = a.name.trim();
        let data = if name.is_empty() {
            // 목록은 파일만 읽으면 끝난다 — 세션도 명부도 필요 없다.
            modules::person_group::list().map_err(map_domain_err)?
        } else {
            // 멤버 해석은 조직도 명부(gw102A02 순회, 30분 캐시)를 타므로 세션이 필요하다.
            self.ensure_session().await?;
            modules::person_group::get(&self.client, name)
                .await
                .map_err(map_domain_err)?
        };
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "사람 그룹을 **만들거나 고친다**(`~/.config/inno-creed/person_groups.json`). 사용자가 '이 사람들 그룹으로 묶어줘' / '누구누구 넣어줘' 라고 하면 이 도구로 저장하고, 이후 person_group(name)으로 꺼내 메일·일정에 쓴다. members는 **이름 또는 empSeq** 목록을 그대로 주면 된다(`[\"김철수\",\"3166\"]`) — 조직도 명부로 해석해 empSeq로 저장한다. mode: `replace`(기본, 새로 만들거나 멤버 통째 교체) · `add`(더하기) · `remove`(빼기); add/remove는 그룹이 이미 있어야 한다. note를 비우면 기존 메모를 유지한다. ⚠️ **저장 시점에 검증한다** — 명부에 없는 사람이거나 **동명이인이면 저장하지 않고 후보를 돌려준다**(그대로 저장하면 나중에 그 사람만 조용히 빠진다). 사용자가 파일을 손으로 편집할 수도 있으므로 이 도구는 다른 그룹을 건드리지 않는다."
    )]
    async fn save_person_group(
        &self,
        Parameters(a): Parameters<SavePersonGroupArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        // 멤버 해석이 조직도 명부를 타므로 세션이 필요하다.
        self.ensure_session().await?;
        let members = a.members.unwrap_or_default();
        let data = modules::person_group::save(&self.client, &a.name, &members, &a.note, &a.mode)
            .await
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "사람 그룹을 지운다. 그룹 정의만 사라지고 사람·메일·일정에는 아무 영향이 없다. 없는 이름이면 실패한다(조용히 성공으로 보고하지 않는다). 멤버 일부만 빼려면 이 도구가 아니라 save_person_group(mode=\"remove\")를 쓴다."
    )]
    async fn delete_person_group(
        &self,
        Parameters(a): Parameters<DeletePersonGroupArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        // 파일만 고친다 — 세션도 명부도 필요 없다.
        let data = modules::person_group::delete(&a.name).map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
