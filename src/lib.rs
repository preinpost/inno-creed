//! inno-creed: 아마란스(gw.innogrid.com) 내부 API를 헤더 서명으로 직접 호출하는 MCP 코어.
//! 브라우저 불필요 — 순수 HTTP + HMAC(wehago-sign)로 완결(실증됨).
#![recursion_limit = "512"]

pub mod client;
pub mod creds;
pub mod error;
pub mod mcp;
pub mod modules;
pub mod sign;
pub mod util;
