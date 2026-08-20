//! inno-creed MCP 서버 (stdio).
//! 크레덴셜(Chrome 쿠키 복호화) 취득 → gw API 도구를 rmcp로 노출.
//!
//! 옵션:
//! - `--approval on|off` — 승인 게이트웨이. 기본 **on** — 부작용 도구(`reserve_resource`
//!   등) 호출 시 Glimpse 팝업으로 사용자 승인을 받는다. 헤드리스/CI에서는 `off`로 꺼라.
//! - `--approval-timeout <초>` — 승인 대기 시간(기본 60). 무응답은 거부 처리.
//!   Glimpse 바이너리는 내장 경로(`native/<os>/glimpse`) 또는 `GLIMPSE_BINARY_PATH`로 지정.

use std::time::Duration;

use anyhow::Result;
use inno_creed::{client::GwClient, creds, mcp::{gate::Gate, Amaranth}};
use rmcp::{transport::stdio, ServiceExt};

#[tokio::main]
async fn main() -> Result<()> {
    // CLI 인자 파싱 — 현재는 승인 게이트웨이 설정만 받는다.
    // 기본값: 승인 게이트웨이 **켜짐** — 부작용 도구는 기본적으로 실행 전 승인을 받는다.
    let mut approval: Option<bool> = None;
    let mut timeout_secs: u64 = 60;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--approval" => match args.next().as_deref() {
                Some("on" | "true") => approval = Some(true),
                Some("off" | "false") => approval = Some(false),
                _ => {
                    eprintln!("[inno-creed] --approval은 on/off만 받습니다");
                    std::process::exit(2);
                }
            },
            "--approval-timeout" => {
                if let Some(v) = args.next().and_then(|v| v.parse::<u64>().ok()) {
                    timeout_secs = v;
                } else {
                    eprintln!("[inno-creed] --approval-timeout은 초 단위 숫자를 받습니다");
                    std::process::exit(2);
                }
            }
            other => {
                eprintln!("[inno-creed] 알 수 없는 인자: {other}");
                std::process::exit(2);
            }
        }
    }

    let gate = Gate::new(approval.unwrap_or(true), Duration::from_secs(timeout_secs));
    if gate.enabled() {
        eprintln!(
            "[inno-creed] ⚠️ 승인 게이트웨이 ON — 부작용 도구 호출 시 팝업으로 승인받습니다 (대기 {timeout_secs}초). 헤드리스라면 --approval off로 끌 것"
        );
    }

    // 크리덴셜 취득 실패해도 서버는 뜬다(비치명적). 실패 시 도구 호출 시점에 로그인 안내를
    // tool 응답으로 반환한다(사용자가 채팅에서 볼 수 있게). 성공하면 캐시를 seed.
    let initial = match creds::from_browser() {
        Ok(c) => {
            eprintln!(
                "[inno-creed] 크레덴셜 취득 완료 (authToken {}자). MCP 서버 시작 (stdio)",
                c.auth_token.len()
            );
            Some(c)
        }
        Err(e) => {
            eprintln!(
                "[inno-creed] ⚠️ 크레덴셜 미취득 — 서버는 시작하되 도구 호출 시 로그인 안내를 반환합니다.\n{e}"
            );
            None
        }
    };

    // 세션 정보(compSeq/deptSeq/근태 empCd 등)는 첫 도구 호출 시 gw050A02로 lazy 취득 후
    // 10분 TTL 캐시된다(ensure_session). 시작 시 선취득하지 않는다.
    let client = GwClient::new(initial);

    let service = Amaranth::new(client, gate).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}