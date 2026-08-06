//! 메일 도구.
//!
//! 라우터는 `mail_router`로 생성돼 `super::Amaranth::all_tools()`에서 합성된다.
//! 담당 도메인 로직은 `modules::mail`에 있고, 여기 핸들러는 **`ensure_session` → 모듈 호출 → 감싸기**만 한다.

use rmcp::{handler::server::wrapper::Parameters, model::{CallToolResult, ContentBlock}, tool, tool_router, ErrorData};

use crate::mcp::{map_domain_err, map_domain_err_ctx, Amaranth};
use crate::mcp::args::mail::*;
use crate::modules;

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

    #[tool(description = "메일을 삭제한다(휴지통 이동). uids=콤마구분 muid. list_mail_inbox의 muid 사용.")]
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
