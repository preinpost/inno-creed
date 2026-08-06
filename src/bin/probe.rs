//! 서명 API probe — 임의의 아마란스 엔드포인트를 wehago-sign 서명으로 호출하고 전체 응답 봉투를 출력.
//! MCP 재부팅/브라우저 없이 Bash로 API를 즉시 찔러보는 디버그 REPL.
//!
//! 사용:
//!   cargo run --quiet --bin probe -- /human/attendapplication/0hr00001 '{"coCd":"1000"}'
//!   cargo run --quiet --bin probe -- /eap/eap110A03 @body.json
//!   (body 생략 시 {}. body가 '@경로'면 파일에서 읽음.)
//!
//! 성공판정 없이 {http, response:{resultCode,resultMsg,resultData}} 전체를 그대로 찍는다(2099 진단용).

use anyhow::{anyhow, Result};
use inno_creed::{client::GwClient, creds};
use serde_json::Value;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    // 진단: `probe sign <apipath> <ts> <tid> <token> <signkey>` → wehago-sign 계산값 출력.
    if args.get(1).map(|s| s.as_str()) == Some("sign") {
        let p = &args[2];
        let ts = &args[3];
        let tid = &args[4];
        let token = &args[5];
        let key = &args[6];
        let sig = inno_creed::sign::wehago_sign(token, tid, ts, p, key);
        println!("{sig}");
        return Ok(());
    }

    // 진단: `probe seq @steps.json` → [{path, body, approkey?}] 를 **단일 GwClient(세션 연속)** 로 순차 실행.
    // approkey:"NEW" 는 첫 등장 시 생성해 이후 재사용(a03↔a06 일치용).
    if args.get(1).map(|s| s.as_str()) == Some("seq") {
        let spec = args.get(2).ok_or_else(|| anyhow!("usage: probe seq @steps.json"))?;
        let txt = if let Some(f) = spec.strip_prefix('@') {
            std::fs::read_to_string(f)?
        } else {
            spec.clone()
        };
        let steps: Vec<Value> = serde_json::from_str(&txt)?;
        let client = GwClient::new(creds::from_browser().ok());
        client.ensure_session().await?;
        let approkey = format!(
            "ERP_{:08x}-1111-2222-3333-444455556666",
            (args.len() as u32).wrapping_mul(2654435761).wrapping_add(7)
        );
        for (i, st) in steps.iter().enumerate() {
            let p = st.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let mut body = st.get("body").cloned().unwrap_or(serde_json::json!({}));
            // approkey 주입(문자열 "NEW" 대체)
            fn inject(v: &mut Value, ak: &str) {
                match v {
                    Value::String(s) if s == "NEW" => *v = Value::String(ak.to_string()),
                    Value::Array(a) => a.iter_mut().for_each(|x| inject(x, ak)),
                    Value::Object(o) => o.values_mut().for_each(|x| inject(x, ak)),
                    _ => {}
                }
            }
            inject(&mut body, &approkey);
            let res = client.call_raw(p, &body).await;
            match res {
                Ok(v) => {
                    let code = v.pointer("/response/resultCode").cloned().unwrap_or(Value::Null);
                    let msg = v
                        .pointer("/response/resultMsg")
                        .and_then(|m| m.as_str())
                        .unwrap_or("");
                    let short: String = msg.chars().take(70).collect();
                    println!("[{i}] {p} -> code={code} {short}");
                    if code == serde_json::json!(2099) {
                        println!("    resultData: {}", v.pointer("/response/resultData").cloned().unwrap_or(Value::Null));
                    }
                    if p.contains("create") {
                        println!("    create.resultData: {}", v.pointer("/response/resultData").cloned().unwrap_or(Value::Null));
                    }
                }
                Err(e) => println!("[{i}] {p} -> ERR {e}"),
            }
        }
        return Ok(());
    }

    // 진단: `probe submit @args.json` → submit_approval 직접 호출(상신 검증). args={form_id,doc_title,line_id,hp_application_json,bind_data_json,doc_contents_html,numbering_id}
    if args.get(1).map(|s| s.as_str()) == Some("submit") {
        let spec = args.get(2).ok_or_else(|| anyhow!("usage: probe submit @args.json"))?;
        let txt = if let Some(f) = spec.strip_prefix('@') { std::fs::read_to_string(f)? } else { spec.clone() };
        let a: Value = serde_json::from_str(&txt)?;
        let client = GwClient::new(creds::from_browser().ok());
        client.ensure_session().await?;
        let g = |k: &str| a.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let out = inno_creed::modules::approval_submit::submit_approval(
            &client,
            a.get("form_id").and_then(|v| v.as_i64()).unwrap_or(0),
            &g("doc_title"),
            a.get("line_id").and_then(|v| v.as_i64()).unwrap_or(0),
            &g("hp_application_json"),
            &g("bind_data_json"),
            &g("doc_contents_html"),
            &g("numbering_id"),
        )
        .await;
        match out {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v)?),
            Err(e) => println!("ERR {e}"),
        }
        return Ok(());
    }

    // 진단: `probe cancel <docId> [formId] [purge]` → cancel_and_verify 직접 호출.
    if args.get(1).map(|s| s.as_str()) == Some("cancel") {
        let doc_id = args.get(2).ok_or_else(|| anyhow!("usage: probe cancel <docId> [formId] [purge]"))?;
        let form_id = args.get(3).map(|s| s.as_str()).unwrap_or("");
        let purge = args.get(4).map(|s| s == "purge" || s == "true").unwrap_or(false);
        let client = GwClient::new(creds::from_browser().ok());
        client.ensure_session().await?;
        let out = inno_creed::modules::approval_submit::cancel_and_verify(&client, doc_id, form_id, purge).await;
        match out {
            Ok(v) => println!("{}", serde_json::to_string_pretty(&v)?),
            Err(e) => println!("ERR {e}"),
        }
        return Ok(());
    }

    let path = args
        .get(1)
        .ok_or_else(|| anyhow!("usage: probe <path> [json|@file]"))?;

    let body: Value = match args.get(2) {
        None => serde_json::json!({}),
        Some(s) if s.is_empty() => serde_json::json!({}),
        Some(s) if s.starts_with('@') => {
            let txt = std::fs::read_to_string(&s[1..])
                .map_err(|e| anyhow!("body 파일 읽기 실패 {}: {e}", &s[1..]))?;
            serde_json::from_str(&txt).map_err(|e| anyhow!("body JSON 파싱 실패: {e}"))?
        }
        Some(s) => serde_json::from_str(s).map_err(|e| anyhow!("body JSON 파싱 실패: {e}"))?,
    };

    let client = GwClient::new(creds::from_browser().ok());
    client.ensure_session().await?;
    let res = client.call_raw(path, &body).await?;
    println!("{}", serde_json::to_string_pretty(&res)?);
    Ok(())
}
