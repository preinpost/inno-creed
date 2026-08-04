//! M0 스모크 테스트: 크레덴셜 취득 → 서명 → 실 API 호출 검증.
//! (MCP 서버 래핑은 코어 검증 후 rmcp로 추가 예정)

use anyhow::Result;
use inno_creed::{client::GwClient, creds, modules};

#[tokio::main]
async fn main() -> Result<()> {
    let creds = creds::from_chrome()?;
    eprintln!(
        "[creds] authToken {}자, signKey {}자 취득",
        creds.auth_token.len(),
        creds.sign_key.len()
    );

    let client = GwClient::new(creds.auth_token, creds.sign_key);
    eprintln!("[session] group={} emp={}", client.group_seq(), client.emp_seq());

    // 1) 자원 목록
    let resources = modules::resource::list_resources(&client).await?;
    let rcount = resources
        .get("resultList")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);
    println!("[rs121A01] 자원 {rcount}개");

    // 2) 메일 목록(받은메일함)
    let mails = modules::mail::list_mails(&client, modules::mail::INBOX, 1, 3).await?;
    println!(
        "[mail003A01] 받은메일 총 {}통, 안읽음 {}통",
        mails.get("TotalRecordCount").unwrap_or(&serde_json::Value::Null),
        mails.get("TotalUnseenCount").unwrap_or(&serde_json::Value::Null)
    );

    println!("\n✅ 코어 동작 확인 (브라우저 없이 순수 HTTP+서명)");
    Ok(())
}
