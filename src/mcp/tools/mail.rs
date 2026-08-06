//! 메일 도구.
//!
//! 라우터는 `mail_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::mail`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::{map_domain_err, map_domain_err_ctx, Amaranth};
use crate::client::GwClient;
use crate::mcp::args::mail::*;
use crate::modules;

/// 받는사람 미지정 시 본인 앞(표시형)으로. 발송·임시저장이 같은 규칙을 쓴다.
fn recipient_or_self(c: &GwClient, to: &Option<String>) -> String {
    to.clone().unwrap_or_else(|| {
        format!("{} <{}@{}>", c.emp_name(), c.email_addr(), c.email_domain())
    })
}

#[tool_router(router = mail_router, vis = "pub(crate)")]
impl Amaranth {
    #[tool(description = "메일함(폴더) 목록을 조회한다")]
    async fn list_mailboxes(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::mail::list_mailboxes(&self.client)
            .await
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "받은메일함 최근 20통을 조회한다. ⚠️ 응답은 서버 원본 봉투 그대로다 — 메일 배열은 `Records`(다른 목록 도구처럼 정규화돼 있지 않음), 각 항목의 `muid`가 read_mail/delete_mail 키, `attach`(bool)가 첨부 유무.")]
    async fn list_mail_inbox(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::mail::list_mails(&self.client, modules::mail::INBOX, 1, 20)
            .await
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(description = "임시보관함(DRAFTS) 최근 20통을 조회한다 — save_mail_draft로 저장한 초안을 발송 전에 사용자에게 확인받는 경로다. ⚠️ 응답은 서버 원본 봉투 그대로다 — 메일 배열은 `Records`(list_mail_inbox와 동일), 각 항목의 `muid`가 read_mail/delete_mail의 키다. 메일함 번호는 계정마다 달라 이름(DRAFTS)으로 해석한다. ⚠️ 전자결재 임시보관함과는 무관하다(그쪽은 list_approvals(box_name=\"draft\")).")]
    async fn list_mail_drafts(&self) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let data = modules::mail::list_drafts(&self.client, 1, 20)
            .await
            .map_err(map_domain_err)?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "메일을 발송한다(2단계: 작성폼 초기화→발송). 받는사람 미지정 시 본인에게. attachments에 로컬 파일 경로를 주면 첨부 발송. ⚠️ 발송은 되돌릴 수 없다(수신자에게 나가면 회수 불가) — 곧바로 보내지 말고, 먼저 save_mail_draft로 보낼 형상을 임시보관함에 만들고 list_mail_drafts로 사용자에게 확인을 요청한 뒤, 확인받고 나서 이 도구로 발송하는 흐름을 권장한다(발송 후 남은 초안은 delete_mail로 정리). ⚠️ 이 도구는 초안을 꺼내 보내는 것이 아니라 새로 조립해 보낸다 — 확인받은 초안과 to/subject/html을 글자 그대로 같게 넣어야 사용자가 승인한 그대로 나간다. 사용자가 즉시 발송을 명시적으로 지시했다면 그때는 곧바로 이 도구를 쓴다."
    )]
    async fn send_mail(
        &self,
        Parameters(a): Parameters<SendMailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let to = recipient_or_self(&self.client, &a.to);
        modules::mail::send_mail(&self.client, &to, &a.subject, &a.html, &a.attachments)
            .await
            .map_err(map_domain_err_ctx("메일 발송 실패"))?;
        let msg = serde_json::json!({
            "ok": true,
            "to": to,
            "subject": a.subject,
            "attachments": a.attachments.len(),
            "note": "발송 성공(result:true). 도착 확인은 list_mail_inbox/보낸메일함 재조회 권장"
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(
        description = "메일을 임시보관함(DRAFTS)에 저장한다 — **발송하지 않는다**(수신자에게 아무것도 가지 않는다). 발송 전 사람 확인을 받는 표준 경로라 send_mail보다 이 도구를 먼저 쓴다 — 초안을 만들고 list_mail_drafts로 사용자에게 확인받은 뒤, 확인되면 send_mail로 발송(남은 초안은 delete_mail로 정리)하거나 사용자가 아마란스 웹에서 직접 보낸다. 다만 사용자가 즉시 발송을 명시적으로 지시했다면 초안을 거치지 말고 곧바로 send_mail을 쓴다. 받는사람 미지정 시 본인. attachments에 로컬 파일 경로를 주면 첨부까지 붙여 저장. 반환 draft_muid = 저장된 임시보관 메일의 muid. ⚠️ 전자결재 임시보관함과는 무관하다(그쪽은 list_approvals(box_name=\"draft\"))."
    )]
    async fn save_mail_draft(
        &self,
        Parameters(a): Parameters<SaveMailDraftArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        let to = recipient_or_self(&self.client, &a.to);
        let data =
            modules::mail::save_mail_draft(&self.client, &to, &a.subject, &a.html, &a.attachments)
                .await
                .map_err(map_domain_err_ctx("메일 임시저장 실패"))?;
        let msg = serde_json::json!({
            "ok": true,
            "to": to,
            "subject": a.subject,
            "attachments": a.attachments.len(),
            "draft_muid": data.get("draft_muid"),
            "mail_key": data.get("mail_key"),
            "sent": false,
            // 임시보관함을 재조회해 그 muid를 실제로 찾았는지. false여도 저장 자체가 실패한 것은
            // 아니지만(조회가 막혔을 수 있다), 그 경우 사람이 임시보관함을 눈으로 확인해야 한다.
            "verified_by_readback": data.get("verified_by_readback"),
            "note": "임시보관함에 저장만 됨(발송 아님). 목록 확인은 list_mail_drafts"
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(description = "메일을 삭제한다(휴지통 이동). uids=콤마구분 muid. muid 출처는 list_mail_inbox, 또는 임시보관함 정리라면 list_mail_drafts(= save_mail_draft가 낸 draft_muid).")]
    async fn delete_mail(
        &self,
        Parameters(a): Parameters<DeleteMailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        self.ensure_session().await?;
        modules::mail::delete_mails(&self.client, &a.uids)
            .await
            .map_err(map_domain_err_ctx("메일 삭제 실패"))?;
        let msg = serde_json::json!({
            "ok": true,
            "uids": a.uids,
            "deleted": true,
            "note": "휴지통 이동됨(muid 재부여 — 이후 추적은 재조회 필요)"
        });
        Ok(CallToolResult::success(vec![ContentBlock::text(msg.to_string())]))
    }

    #[tool(
        description = "메일 1건의 본문(평문)·헤더·첨부목록을 조회한다. 본문 HTML은 렌더링하지 않고 평문화(외부 이미지 자동로드 안 함, remoteResourceCount로 경고). muid=list_mail_inbox의 muid."
    )]
    async fn read_mail(
        &self,
        Parameters(a): Parameters<ReadMailArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data = modules::mail::read_mail(&self.client, &a.muid)
            .await
            .map_err(map_domain_err_ctx("메일 조회 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }

    #[tool(
        description = "메일 첨부파일을 다운로드해 out_path에 저장한다(실행하지 않고 저장만). **file_sn 은 순번이 아니라 read_mail 응답 `attachments[].fileSn` 의 긴 토큰 문자열을 그대로** 넣는다 — 숫자(0,1)를 넣으면 서버가 422로 거절한다."
    )]
    async fn download_mail_attachment(
        &self,
        Parameters(a): Parameters<DownloadMailAttachmentArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let data =
            modules::mail::download_attachment(&self.client, &a.muid, &a.file_sn, &a.out_path)
                .await
                .map_err(map_domain_err_ctx("첨부 다운로드 실패"))?;
        Ok(CallToolResult::success(vec![ContentBlock::text(data.to_string())]))
    }
}
