//! 승인 게이트웨이 — 부작용 도구 실행 전 사용자 승인.
//!
//! 1. 게이트 대상 도구 핸들러가 `Gate::approve(&self.gate_ctx, ...)` 호출
//! 2. 활성화 상태면 **GUI 팝업**(내장 Glimpse 네이티브 바이너리, JSONL 프로토콜) 시도
//! 3. GUI 바이너리가 없으면(spawn 실패) **CLI 폴백** — stderr에 내용을 출력하고
//!    `/dev/tty`(Windows: CONIN$)에서 y/N 입력을 받는다
//! 4. 사용자 승인/거부/창 닫힘/무응답(타임아웃)에 따라 실행 여부 결정
//!
//! 비활성 상태(`enabled=false`, 기본값)에서는 그냥 통과시킨다 — 게이트는
//! 기본 ON(`--approval` 미지정)이며, 헤드리스/CI에서 도구를 바로 실행하려면
//! `--approval off`로 끈다. GUI도 없고 tty도 없으면 명시적 에러로 거부된다.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use base64::Engine;
use rmcp::ErrorData;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::time::timeout;

/// 기본 승인 대기 시간.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// 승인 게이트 설정 — 값 타입 하나로 들고 다닌다(Arc 불필요).
/// `Amaranth`는 이걸 필드로 보관하고, 도구 핸들러는 `Gate::approve(&self.gate_ctx, ...)`로 쓴다.
#[derive(Clone, Debug)]
pub struct GateContext {
    pub enabled: bool,
    pub binary: PathBuf,
    pub timeout: Duration,
}

/// 승인 게이트웨이 구현체 — 상태 없는 **정적 메서드** 네임스페이스.
/// 승인 로직은 Server(Amaranth)가 아니라 여기 `Gate::approve(...)`에 있다.
pub struct Gate;

/// 동시 팝업 방지용 직렬화 세마포어(프로세스당 1개).
static APPROVAL_SEM: OnceLock<Semaphore> = OnceLock::new();

fn approval_sem() -> &'static Semaphore {
    APPROVAL_SEM.get_or_init(|| Semaphore::new(1))
}

impl GateContext {
    pub fn enabled(&self) -> bool {
        self.enabled
    }
}

/// CLI 폴백 프롬프트에 필요한 문맥(텍스트용).
struct PromptCtx<'a> {
    tool: &'a str,
    kind: &'a str,
    summary: &'a str,
    rows: &'a [(String, String)],
}

impl Gate {
    /// 게이트 비활성(통과 모드). 기본값.
    pub fn disabled() -> GateContext {
        Gate::new(false, DEFAULT_TIMEOUT)
    }

    /// `enabled`가 true면 승인을 받는다. GUI 네이티브 바이너리는
    /// `GLIMPSE_BINARY_PATH`(우선) 또는 플랫폼별 내장 경로를 쓴다.
    ///
    /// 내장 경로: repo 루트 기준 — macOS `native/macos/glimpse`,
    /// Linux `native/linux/glimpse-<x86_64|aarch64>`, Windows `native/windows/glimpse.exe`.
    pub fn new(enabled: bool, timeout: Duration) -> GateContext {
        let binary = std::env::var("GLIMPSE_BINARY_PATH")
            .map(PathBuf::from)
            .unwrap_or_else(|_| builtin_native_path());
        GateContext {
            enabled,
            binary,
            timeout,
        }
    }

    /// 부작용 도구 실행 전 호출.
    ///
    /// - 비활성: 즉시 `Ok(())` (통과)
    /// - 활성 + 승인(GUI 또는 CLI 폴백): `Ok(())`
    /// - 활성 + 거부·무응답·창 닫힘: `Err` (도구 실행 안 됨)
    /// - 활성 + GUI도 CLI도 불가: `Err` (명시적, 조용히 통과하지 않음)
    pub async fn approve(
        ctx: &GateContext,
        tool: &str,
        kind: &str,
        summary: &str,
        rows: &[(String, String)],
    ) -> Result<(), ErrorData> {
        if !ctx.enabled {
            return Ok(());
        }
        // 승인 직렬화 — 먼저 띄운 승인창/프롬프트가 끝날 때까지 대기.
        let _permit = approval_sem()
            .acquire()
            .await
            .map_err(|_| ErrorData::internal_error("승인 게이트웨이 오류: 세마포어 획득 실패", None))?;

        let prompt_ctx = PromptCtx {
            tool,
            kind,
            summary,
            rows,
        };
        let html = build_html(tool, kind, summary, rows);

        // GUI 우선 → 실패 시 CLI 폴백.
        let decision = match Self::run_gui_popup(ctx, &html).await {
            Ok(d) => d,
            Err(gui_err) => match Self::run_cli_prompt(ctx, &prompt_ctx).await {
                Ok(d) => d,
                // GUI도 없고 tty도 없음 — 명시적 거부.
                Err(cli_err) => {
                    return Err(ErrorData::internal_error(
                        format!("{gui_err}\n{cli_err}"),
                        None,
                    ))
                }
            },
        };

        match decision {
            Decision::Approve => Ok(()),
            Decision::Deny(cause) => Err(ErrorData::internal_error(
                Self::deny_message(tool, cause, ctx.timeout.as_secs()),
                None,
            )),
        }
    }

    /// 에이전트가 읽는 거부 메시지 — 재시도 루프를 막도록 사유를 구분하고
    /// "자동 재시도하지 말 것 / 사용자에게 승인 요청"을 명시적으로 지시한다.
    fn deny_message(tool: &str, cause: DenyCause, timeout_secs: u64) -> String {
        // 사유별 머리말 — 꼬리(재시도 금지 지시)는 Prompt 공통 상수를 쓴다.
        let head = match cause {
            DenyCause::Clicked => format!("사용자가 '{tool}' 실행을 거부했습니다."),
            DenyCause::Closed => format!("'{tool}' 승인 창이 닫혀 거부 처리되었습니다."),
            DenyCause::Timeout => format!(
                "'{tool}' 승인이 {timeout_secs}초 안에 완료되지 않아 거부 처리되었습니다."
            ),
        };
        format!("{head} {}", Prompt::AgentNoRetry.text())
    }

    /// GUI 팝업 실행 → 결정 대기. `Ok(true)`=승인, `Ok(false)`=거부·닫힘·무응답.
    /// `Err`=팝업 자체를 못 띄움(GUI 바이너리 없음 등) → 호출자가 CLI 폴백 처리.
    ///
    /// 내장 네이티브 `glimpse` 바이너리와 직접 대화한다(Node 불필요):
    /// stdin으로 `{"type":"html","html":<base64>}`를 보내고,
    /// stdout의 `{"type":"message","data":{...}}`(사용자 버튼)을 기다린다.
    /// ⚠️ **stdin EOF가 창을 닫는다** — HTML 전송 후 stdin을 닫으면 안 된다.
    async fn run_gui_popup(ctx: &GateContext, html: &str) -> Result<Decision, ErrorData> {
        let mut child = match Command::new(&ctx.binary)
            .args(["--auto-close", "--floating", "--frameless"])
            .args(["--width", "560", "--height", "620"])
            .arg("--title")
            .arg("inno-creed — 승인 게이트웨이")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                return Err(ErrorData::internal_error(
                    format!(
                        "GUI 승인 팝업 실행 실패({}): {e} — CLI 폴백으로 전환",
                        ctx.binary.display()
                    ),
                    None,
                ))
            }
        };

        let mut stdin = match child.stdin.take() {
            Some(s) => s,
            None => {
                return Err(ErrorData::internal_error("승인 팝업 오류: stdin 획득 실패", None))
            }
        };
        let mut stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                return Err(ErrorData::internal_error(
                    "승인 팝업 오류: stdout 획득 실패",
                    None,
                ))
            }
        };

        // HTML 주입 — base64로 감싸 JSON 파싱 충돌을 피한다. stdin은 열어둔다.
        if let Err(e) = stdin.write_all(html_command(html).as_bytes()).await {
            // 팝업이 일찍 죽어 파이프가 깨졌을 수 있다 → GUI 실패로 처리(폴백).
            return Err(ErrorData::internal_error(format!(
                "승인 팝업 HTML 전달 실패: {e} — CLI 폴백으로 전환"
            ), None));
        }
        stdin.flush().await.ok();

        let mut lines = BufReader::new(&mut stdout).lines();
        let decided = timeout(ctx.timeout, async {
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        if let Some(d) = parse_decision(&line) {
                            break d;
                        }
                    }
                    // stdout EOF = 창이 닫힘(메시지 없음) → 거부(닫힘).
                    Ok(None) => break Decision::Deny(DenyCause::Closed),
                    // 파이프 깨짐 → 거부(보수적으로, 닫힘으로 취급).
                    Err(_) => break Decision::Deny(DenyCause::Closed),
                }
            }
        })
        .await;

        // 창 닫기: auto-close가 이미 처리했을 수 있지만, 거부·타임아웃에도 확실히 닫는다.
        let _ = stdin.write_all(b"{\"type\":\"close\"}\n").await;
        let _ = stdin.flush().await;
        drop(stdin); // EOF — 네이티브가 창을 닫고 종료
        let _ = child.kill().await;
        let _ = child.wait().await;

        let decision = match decided {
            Ok(d) => d,
            // 타임아웃(무응답) → 거부.
            Err(_) => Decision::Deny(DenyCause::Timeout),
        };
        Ok(decision)
    }

    /// CLI 폴백 — 내용을 stderr로 출력하고 tty에서 y/N 입력을 받는다.
    /// `Ok(true)`=승인, `Ok(false)`=거부·무응답(타임아웃), `Err`=입력 불가(GUI도 없음).
    async fn run_cli_prompt(ctx: &GateContext, prompt_ctx: &PromptCtx<'_>) -> Result<Decision, ErrorData> {
        eprintln!("{}", prompt_text(ctx.binary.display(), prompt_ctx));

        // 터미널 입력: Unix /dev/tty, Windows CONIN$ — 도구를 실행한 MCP 서버의
        // stdin/stdout은 프로토콜이라 터미널에서 직접 읽는다.
        let tty_path = if cfg!(windows) {
            "CONIN$"
        } else {
            "/dev/tty"
        };

        let tty = match tokio::fs::File::options()
            .read(true)
            .write(true)
            .open(tty_path)
            .await
        {
            Ok(f) => f,
            Err(e) => {
                return Err(ErrorData::internal_error(format!(
                    "CLI 승인 폴백 실패({tty_path}): {e} — GUI 바이너리도 없고 터미널 입력도 열 수 없습니다"
                ), None))
            }
        };

        let mut reader = BufReader::new(tty);
        // 프롬프트 한 번 출력 후 한 줄 읽기. 타임아웃·EOF(입력 없음)면 거부.
        let mut buf = String::new();
        let answer = timeout(ctx.timeout, async { reader.read_line(&mut buf).await })
            .await;

        match answer {
            Ok(Ok(n)) if n > 0 => {
                if parse_yes_no(&buf) {
                    Ok(Decision::Approve)
                } else {
                    Ok(Decision::Deny(DenyCause::Clicked))
                }
            }
            // EOF(입력 없음) → 창 닫힘 취급.
            Ok(Ok(_)) => Ok(Decision::Deny(DenyCause::Closed)),
            // 오류·타임아웃 → 거부(타임아웃 취급).
            Ok(Err(_)) | Err(_) => Ok(Decision::Deny(DenyCause::Timeout)),
        }
    }
}

/// CLI 폴백 프롬프트 텍스트.
/// 승인 게이트웨이가 사용자·에이전트에게 노출하는 문구 모음.
/// 문구가 바뀌면 여기만 고치면 팝업/CLI 폴백/거부 메시지가 함께 따라온다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Prompt {
    /// 팝업 상단 배지
    BadgeNeedApproval,
    /// 승인 버튼 라벨
    ApproveButton,
    /// 거부 버튼 라벨
    DenyButton,
    /// 팝업 하단 안내(무응답=자동 거부). HTML 포함.
    PopupFooter,
    /// CLI 폴백 배너 첫 줄
    CliBanner,
    /// CLI 폴백 라벨: 도구
    CliToolLabel,
    /// CLI 폴백 라벨: 동작
    CliActionLabel,
    /// CLI 폴백 라벨: 내용
    CliContentLabel,
    /// CLI 폴백 질문 접미사
    CliQuestion,
    /// 에이전트 재시도 금지 공통 지시 — 모든 거부 사유에 함께 붙는다
    AgentNoRetry,
}

impl Prompt {
    pub(crate) fn text(self) -> &'static str {
        use Prompt::*;
        match self {
            BadgeNeedApproval => "승인 필요",
            ApproveButton => "✅ 승인",
            DenyButton => "✕ 거부",
            PopupFooter =>
                "이 작업이 맞으면 <b>승인</b>, 아니면 <b>거부</b>를 누르세요. 무응답 시 자동으로 거부 처리됩니다.",
            CliBanner => "========== [inno-creed] 승인 게이트웨이 (CLI 폴백) ==========",
            CliToolLabel => "도구",
            CliActionLabel => "동작",
            CliContentLabel => "내용",
            CliQuestion => "위 작업을 실행할까요? [y/N] ",
            AgentNoRetry =>
                "에이전트: 승인 없이는 자동 재시도하지 말고, 사용자에게 승인을 요청하세요.",
        }
    }
}

fn prompt_text(binary: impl std::fmt::Display, ctx: &PromptCtx<'_>) -> String {
    let mut s = String::new();
    s.push('\n');
    s.push_str(Prompt::CliBanner.text());
    s.push('\n');
    s.push_str(&format!("GUI 바이너리를 찾을 수 없습니다: {binary}\n"));
    s.push_str(&format!("{}: {}\n", Prompt::CliToolLabel.text(), ctx.tool));
    s.push_str(&format!("{}: {}\n", Prompt::CliActionLabel.text(), ctx.kind));
    s.push_str(&format!("{}: {}\n", Prompt::CliContentLabel.text(), ctx.summary));
    for (k, v) in ctx.rows {
        s.push_str(&format!("  - {k}: {v}\n"));
    }
    s.push_str(&"=".repeat(60));
    s.push('\n');
    s.push_str(Prompt::CliQuestion.text());
    s
}

/// y/N 입력 파싱 — y/Y/yes만 승인, 그 외(빈 줄 포함)는 거부.
fn parse_yes_no(line: &str) -> bool {
    let t = line.trim().to_ascii_lowercase();
    matches!(t.as_str(), "y" | "yes" | "o" | "ㅇ" | "예")
}

/// 현재 OS/아키에 해당하는 내장 바이너리의 상대 경로 (`native/<os>/...`).
fn native_rel_path() -> PathBuf {
    let rel = if cfg!(target_os = "macos") {
        "macos/glimpse"
    } else if cfg!(target_os = "windows") {
        "windows/glimpse.exe"
    } else if cfg!(target_os = "linux") {
        if cfg!(target_arch = "x86_64") {
            "linux/glimpse-x86_64"
        } else if cfg!(target_arch = "aarch64") {
            "linux/glimpse-aarch64"
        } else {
            "linux/glimpse"
        }
    } else {
        return PathBuf::from("glimpseui");
    };
    PathBuf::from(rel)
}

/// 내장 네이티브 바이너리 경로 탐색.
/// MCP 클라이언트가 임의의 CWD에서 서버를 띄워도 동작해야 하므로 CWD에 의존하지 않는다.
/// 1. 실행 파일(exe_dir) 옆의 `native/` — 배포 레이아웃: `<bin>/inno-creed` + `<bin>/native/<os>/…`
/// 2. exe_dir의 상위 `native/` (cargo `target/debug|release` 레이아웃: repo 루트)
/// 3. 현재 디렉토리의 `native/` (repo 루트에서 실행)
/// 어디서도 못 찾으면 3번 경로를 기본값으로 돌려준다(에러 메시지가 경로를 보여주도록).
fn builtin_native_path() -> PathBuf {
    let rel = native_rel_path();
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // 배포: <bin>/native/<os>/… , cargo 레이아웃: <repo>/target/<profile>/… 에서
            // repo 루트는 2단계 위. 모두 후보에 넣는다.
            candidates.push(dir.join("native").join(&rel));
            candidates.push(dir.join("..").join("native").join(&rel));
            candidates.push(dir.join("../..").join("native").join(&rel));
        }
    }
    candidates.push(PathBuf::from("native").join(&rel));
    candidates.into_iter().find(|c| c.exists()).unwrap_or_else(|| {
        PathBuf::from("native").join(rel)
    })
}

/// 네이티브 프로토콜 `html` 명령: `{"type":"html","html":<base64>}\n`.
/// 줄바꿈 필수 — 네이티브는 `readLine()`으로 한 줄씩 파싱한다.
fn html_command(html: &str) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(html.as_bytes());
    format!(r#"{{"type":"html","html":"{b64}"}}"#) + "\n"
}

/// 거부 사유 — 에이전트가 읽는 메시지를 구분한다.
#[derive(PartialEq, Debug, Clone, Copy)]
enum DenyCause {
    /// 사용자가 거부 버튼을 눌렀다.
    Clicked,
    /// 승인 창이 닫혔다(메시지 없이).
    Closed,
    /// 대기 시간 내 무응답.
    Timeout,
}

#[derive(PartialEq, Debug)]
enum Decision {
    Approve,
    Deny(DenyCause),
}

/// 팝업에서 온 stdout JSONL 한 줄을 파싱. 네이티브 형식은
/// `{"type":"message","data":{"action":"approve"}}` — 승인/거부만 인식하고
/// `ready`·`info` 등 다른 메시지·깨진 JSON은 `None`(계속 대기).
fn parse_decision(line: &str) -> Option<Decision> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let action = if let Some(data) = v.get("data") {
        data.get("action")?.as_str()?
    } else {
        v.get("action")?.as_str()?
    };
    match action {
        "approve" => Some(Decision::Approve),
        "deny" => Some(Decision::Deny(DenyCause::Clicked)),
        _ => None,
    }
}

/// YYYYMMDDHHmm → "YYYY-MM-DD HH:mm". 12자리 숫자가 아니면 원문 그대로.
pub fn fmt_ts(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() == 12 && b.iter().all(|c| c.is_ascii_digit()) {
        format!(
            "{}-{}-{} {}:{}",
            &s[0..4],
            &s[4..6],
            &s[6..8],
            &s[8..10],
            &s[10..12]
        )
    } else {
        s.to_string()
    }
}

/// HTML 이스케이프 — 팝업에 사용자·도구 데이터를 안전하게 삽입.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// 승인 팝업 HTML. frameless+floating 창, 승인/거부 버튼 + Enter/Escape 단축키.
fn build_html(tool: &str, kind: &str, summary: &str, rows: &[(String, String)]) -> String {
    let tool = esc(tool);
    let kind = esc(kind);
    let summary = esc(summary);
    let badge = Prompt::BadgeNeedApproval.text();
    let approve_btn = format!(
        "{} <span class=\"kbd\">Enter</span>",
        Prompt::ApproveButton.text()
    );
    let deny_btn = format!(
        "{} <span class=\"kbd\">Esc</span>",
        Prompt::DenyButton.text()
    );
    let footer = Prompt::PopupFooter.text();
    let rows_html: String = rows
        .iter()
        .map(|(k, v)| {
            format!(
                "<tr><td class=\"k\">{}</td><td class=\"v\">{}</td></tr>",
                esc(k),
                esc(v)
            )
        })
        .collect();

    format!(
        r#"<!DOCTYPE html>
<html lang="ko">
<head>
<meta charset="utf-8">
<style>
  * {{ box-sizing: border-box; margin: 0; }}
  body {{ font-family: -apple-system, "Apple SD Gothic Neo", sans-serif; background: #14141f; color: #e8e8f0; padding: 20px 24px; height: 100vh; display: flex; flex-direction: column; gap: 14px; }}
  .head {{ display: flex; align-items: center; gap: 10px; }}
  .badge {{ background: linear-gradient(135deg, #e94560, #c23152); color: #fff; font-size: 12px; font-weight: 700; padding: 4px 10px; border-radius: 999px; }}
  .tool {{ font-size: 12px; color: #9aa0b5; font-family: ui-monospace, Menlo, monospace; }}
  h2 {{ font-size: 19px; font-weight: 700; }}
  p.summary {{ font-size: 13.5px; color: #cfd3e3; line-height: 1.55; }}
  table {{ width: 100%; border-collapse: collapse; background: #1c1c2a; border-radius: 10px; overflow: hidden; font-size: 13px; }}
  td {{ padding: 9px 12px; border-bottom: 1px solid #26263a; vertical-align: top; }}
  tr:last-child td {{ border-bottom: none; }}
  td.k {{ color: #8b90a8; width: 110px; white-space: nowrap; }}
  td.v {{ color: #e8e8f0; word-break: break-all; font-family: ui-monospace, Menlo, monospace; font-size: 12.5px; }}
  .footer {{ margin-top: auto; font-size: 11.5px; color: #6d7289; }}
  .buttons {{ display: flex; gap: 10px; }}
  button {{ flex: 1; padding: 13px; font-size: 14px; font-weight: 700; border: none; border-radius: 10px; cursor: pointer; transition: transform .12s; }}
  button:hover {{ transform: scale(1.02); }}
  button:active {{ transform: scale(.97); }}
  .approve {{ background: linear-gradient(135deg, #2ec4b6, #1f9d92); color: #fff; }}
  .deny {{ background: rgba(255,255,255,.09); color: #b9becf; }}
  .kbd {{ font-family: ui-monospace, Menlo, monospace; font-size: 10.5px; border: 1px solid #3a3f55; border-radius: 4px; padding: 1px 5px; color: #9aa0b5; }}
</style>
</head>
<body>
  <div class="head">
    <span class="badge">{badge}</span>
    <span class="tool">{tool}</span>
  </div>
  <h2>{kind}</h2>
  <p class="summary">{summary}</p>
  <table>{rows_html}</table>
  <div class="footer">{footer}</div>
  <div class="buttons">
    <button class="approve" onclick="approve()">{approve_btn}</button>
    <button class="deny" onclick="deny()">{deny_btn}</button>
  </div>
  <script>
    function approve() {{ window.glimpse.send({{ action: 'approve' }}); }}
    function deny() {{ window.glimpse.send({{ action: 'deny' }}); }}
    document.addEventListener('keydown', e => {{
      if (e.key === 'Enter') approve();
      else if (e.key === 'Escape') deny();
    }});
  </script>
</body>
</html>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::Amaranth;

    #[test]
    fn 프롬프트_상수가_빈_문구면_안_된다() {
        let all = [
            Prompt::BadgeNeedApproval,
            Prompt::ApproveButton,
            Prompt::DenyButton,
            Prompt::PopupFooter,
            Prompt::CliBanner,
            Prompt::CliToolLabel,
            Prompt::CliActionLabel,
            Prompt::CliContentLabel,
            Prompt::CliQuestion,
            Prompt::AgentNoRetry,
        ];
        for p in all {
            assert!(!p.text().is_empty(), "{p:?} 빈 문구");
        }
    }

    #[test]
    fn 거부_메시지는_공통_재시도금지_지시를_붙인다() {
        for (cause, expect_head) in [
            (DenyCause::Clicked, "거부했습니다"),
            (DenyCause::Closed, "창이 닫혀"),
            (DenyCause::Timeout, "완료되지 않아"),
        ] {
            let m = Gate::deny_message("reserve_resource", cause, 30);
            assert!(m.contains(expect_head), "{m}");
            assert!(
                m.contains(Prompt::AgentNoRetry.text()),
                "공통 지시 누락: {m}"
            );
        }
    }

    #[test]
    fn html_이스케이프_한다() {
        let s = esc(r#"<script>&"'</script>"#);
        assert_eq!(s, "&lt;script&gt;&amp;&quot;&#39;&lt;/script&gt;");
    }

    #[test]
    fn 시각_포맷() {
        assert_eq!(fmt_ts("202608051300"), "2026-08-05 13:00");
        assert_eq!(fmt_ts("2026"), "2026");
        assert_eq!(fmt_ts(""), "");
    }

    #[test]
    fn html_명령은_base64로_감싼다() {
        let cmd = html_command("<b>hi</b>");
        assert!(cmd.starts_with(r#"{"type":"html","html":""#));
        assert!(cmd.trim_end().ends_with(r#""}"#));
        let v: serde_json::Value = serde_json::from_str(&cmd).unwrap();
        let b64 = v["html"].as_str().unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(String::from_utf8(decoded).unwrap(), "<b>hi</b>");
    }

    #[test]
    fn 결정_파싱_네이티브_형식() {
        assert_eq!(
            parse_decision(r#"{"type":"message","data":{"action":"approve"}}"#),
            Some(Decision::Approve)
        );
        assert_eq!(
            parse_decision(r#"{"type":"message","data":{"action":"deny"}}"#),
            Some(Decision::Deny(DenyCause::Clicked))
        );
        assert_eq!(
            parse_decision(r#"{"type":"ready","screen":{}}"#),
            None,
            "ready는 무시하고 계속 대기"
        );
        assert_eq!(parse_decision("not json"), None);
    }

    #[test]
    fn yN_파싱() {
        assert!(parse_yes_no("y"));
        assert!(parse_yes_no("Y"));
        assert!(parse_yes_no("yes"));
        assert!(parse_yes_no(" 예 "));
        assert!(!parse_yes_no("n"));
        assert!(!parse_yes_no(""));
        assert!(!parse_yes_no("no"));
        assert!(!parse_yes_no("ㅇㅖ"));
    }

    #[test]
    fn cli_폴백_프롬프트에_내용이_담긴다() {
        let ctx = PromptCtx {
            tool: "reserve_resource",
            kind: "회의실 예약 등록",
            summary: "이 예약이 반영됩니다.",
            rows: &[("시작 시각".into(), "2026-08-05 13:00".into())],
        };
        let text = prompt_text("native/macos/glimpse", &ctx);
        assert!(text.contains("reserve_resource"));
        assert!(text.contains("회의실 예약 등록"));
        assert!(text.contains("2026-08-05 13:00"));
        assert!(text.contains("[y/N]"));
    }

    #[tokio::test]
    async fn 비활성이면_승인_없이_통과() {
        let ctx = GateContext {
            enabled: false,
            binary: PathBuf::from("/nonexistent/glimpse"),
            timeout: DEFAULT_TIMEOUT,
        };
        assert!(Gate::approve(&ctx, "t", "k", "s", &[]).await.is_ok());
    }

    #[tokio::test]
    async fn 활성인데_바이너리_없고_tty도_없으면_명시적_에러() {
        // CI 환경(테스트)에는 tty가 없음 → GUI 실패 후 CLI 폴백도 실패 → 에러.
        let ctx = GateContext {
            enabled: true,
            binary: PathBuf::from("/nonexistent/glimpse"),
            timeout: Duration::from_secs(2),
        };
        let err = Gate::approve(&ctx, "t", "k", "s", &[]).await.unwrap_err();
        // tty가 없는 CI에서는 "폴백 실패", 터미널에서 실행하면 입력 대기 후 타임아웃 거부.
        let m = err.message.clone();
        assert!(
            m.contains("CLI 승인 폴백 실패") || m.contains("승인되지 않았습니다"),
            "{m}"
        );
    }

    #[test]
    fn 팝업_html_에_도구명과_요약이_들어간다() {
        let html = build_html(
            "reserve_resource",
            "회의실 예약 등록",
            "회의실을 새로 예약합니다.",
            &[("자원 ID".into(), "123".into()), ("예약명".into(), "TF <팀> 회의".into())],
        );
        assert!(html.contains("reserve_resource"));
        assert!(html.contains("회의실 예약 등록"));
        assert!(html.contains("&lt;팀&gt;"));
        assert!(html.contains("window.glimpse.send"));
    }

    // ── spawn→stdin→stdout 왕복 통합 테스트 (unix) ──────────────────────────
    // GUI 없이 실제 프로세스 파이프 경로를 검증하기 위해 결정만 출력하는
    // 가짜 네이티브 스크립트를 binary로 쓴다.

    #[cfg(unix)]
    fn script(content: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
            "inno-creed-gate-test-{}-{}",
            std::process::id(),
            content.len()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("fake-glimpse.sh");
        std::fs::write(&p, content).unwrap();
        let mut perm = std::fs::metadata(&p).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&p, perm).unwrap();
        p
    }

    #[cfg(unix)]
    fn gate_with_binary(p: PathBuf) -> GateContext {
        GateContext {
            enabled: true,
            binary: p,
            timeout: Duration::from_secs(5),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn 승인_스크립트면_통과한다() {
        // 네이티브처럼 stdin을 한 줄 읽고(EOF까지 기다리지 않음) 결정을 출력한다.
        let p = script(
            "#!/bin/sh\nread _line\necho '{\"type\":\"message\",\"data\":{\"action\":\"approve\"}}'\n",
        );
        assert!(Gate::approve(&gate_with_binary(p), "t", "k", "s", &[]).await.is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn 거부_스크립트면_에러가_난다() {
        let p = script(
            "#!/bin/sh\nread _line\necho '{\"type\":\"message\",\"data\":{\"action\":\"deny\"}}'\n",
        );
        let err = Gate::approve(&gate_with_binary(p), "t", "k", "s", &[]).await.unwrap_err();
        // 거부 사유(참여) + 에이전트 재시도 금지 지시가 담긴다.
        assert!(err.message.contains("거부했습니다"), "{}", err.message);
        assert!(err.message.contains("재시도하지 말"), "{}", err.message);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn 출력_없이_종료하면_거부된다() {
        // 메시지 없이 종료 → stdout EOF → 거부.
        let p = script("#!/bin/sh\nread _line\nexit 0\n");
        assert!(Gate::approve(&gate_with_binary(p), "t", "k", "s", &[]).await.is_err());
    }

    /// 수동 데모(무시됨): 실제 내장 팝업을 띄우고 사용자 승인을 받는다.
    /// 실행: cargo test -- 실제_팝업 --ignored --nocapture
    /// (repo 루트의 native/macos/glimpse 필요 — 없으면 CLI 폴백으로 동작)
    #[cfg(unix)]
    #[ignore]
    #[tokio::test]
    async fn 실제_팝업_수동_데모() {
        let ctx = Gate::new(true, Duration::from_secs(120));
        let result = Gate::approve(&ctx, 
                "reserve_resource",
                "회의실 예약 등록",
                "이 예약이 반영됩니다 — 아래 내용이 맞는지 확인해 주세요.",
                &[
                    ("자원 ID (res_seq)".into(), "RES-0001234".into()),
                    ("예약명".into(), "TF 개발 주간회의".into()),
                    ("시작 시각".into(), fmt_ts("202608051300")),
                    ("종료 시각".into(), fmt_ts("202608051400")),
                    ("내용".into(), "진행 상황 공유 및 다음 스프린트 계획".into()),
                ],
            )
            .await;
        match result {
            Ok(()) => println!("✅ 승인됨 — Popup 확인"),
            Err(e) => println!("🚫 승인 안 됨 — {e}"),
        }
    }
}