//! gw API HTTP 클라이언트: 헤더 서명 + 표준 응답봉투({resultCode,resultMsg,resultData}) 파싱.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::creds::{self, Creds};
use crate::sign;

/// 세션 정보 인메모리 캐시 TTL(10분). 만료 시 `ensure_session()`이 gw050A02로 재조회.
const SESSION_TTL: Duration = Duration::from_secs(600);

/// 캘린더 목록 인메모리 캐시 TTL(10분). 조회(list_events)·등록(create_calendar_event) 양쪽이 같은
/// 목록을 쓰므로, 도구 호출마다 sc111A02를 반복하지 않도록 캐시한다. 값 자체는 서버가
/// 진실의 출처 — TTL 만료 시 자동 재조회하므로 캘린더 추가/삭제도 10분 내 반영된다.
const CALENDAR_TTL: Duration = Duration::from_secs(600);

/// 전사 사원 명부 캐시 TTL(30분). 명부 조립은 부서 수만큼 gw102A02를 호출해야 해서 비싸다
/// (실측 75개 부서). 인사이동이 분 단위로 바뀌지 않으므로 캘린더보다 긴 TTL을 쓴다.
const ROSTER_TTL: Duration = Duration::from_secs(1800);

/// application/x-www-form-urlencoded 바디 인코딩(reqwest default-features=false라 `.form()` 미제공).
fn form_urlencode(params: &[(&str, &str)]) -> String {
    fn enc(s: &str) -> String {
        let mut out = String::new();
        for b in s.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char)
                }
                b' ' => out.push('+'),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
    params
        .iter()
        .map(|(k, v)| format!("{}={}", enc(k), enc(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Content-Disposition 헤더에서 파일명 추출. `filename*=UTF-8''...`(RFC5987) 우선, 없으면
/// `filename="..."`. 최소 파서(에이전트 표시용).
fn parse_cd_filename(cd: &str) -> Option<String> {
    if let Some(i) = cd.find("filename*=") {
        let rest = &cd[i + "filename*=".len()..];
        let val = rest.split(';').next().unwrap_or("").trim();
        // UTF-8''<pct-encoded>
        if let Some(enc) = val.split("''").nth(1) {
            return Some(pct_decode(enc.trim_matches('"')));
        }
    }
    if let Some(i) = cd.find("filename=") {
        let rest = &cd[i + "filename=".len()..];
        let val = rest.split(';').next().unwrap_or("").trim().trim_matches('"');
        if !val.is_empty() {
            return Some(pct_decode(val));
        }
    }
    None
}

/// 퍼센트 디코드(파일명용, 최소 구현).
fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len()
            && let Ok(h) = u8::from_str_radix(&s[i + 1..i + 3], 16)
        {
            out.push(h);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 401 재시도 정책의 뼈대. `send`를 호출해 응답을 받고, 그것이 인증 실패(`unauthorized`)면
/// `reacquire`로 크레덴셜을 갈아끼운 뒤 **정확히 한 번만** 다시 보낸다.
///
/// 재시도 상한을 재귀·루프가 아니라 **이 함수의 구조**로 못박는다 — `send` 호출 지점이 2개뿐이고
/// 두 번째 결과는 상태와 무관하게 그대로 반환한다. 전송·재취득을 클로저로 분리한 덕에
/// 네트워크 없이 호출 횟수를 검증할 수 있다(아래 테스트).
///
/// `reacquire`가 false(재취득 실패 — 브라우저 미로그인 등)면 재시도해봐야 같은 결과라
/// 첫 응답을 그대로 돌려준다. 호출부가 평소의 실패 처리(로그인 안내)를 하도록.
async fn retry_once_on_401<T, F, Fut>(
    unauthorized: impl Fn(&T) -> bool,
    reacquire: impl FnOnce() -> bool,
    mut send: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let first = send().await?;
    if !unauthorized(&first) || !reacquire() {
        return Ok(first);
    }
    send().await
}

/// 로그인 세션에서 동적으로 취득하는 사용자/조직 정보. groupSeq/empSeq는 authToken에서,
/// 나머지는 `ensure_session()`이 gw050A02(sessionInfo.ucUserInfo)로 채운다(하드코딩 없음 — 배포용).
#[derive(Default, Clone)]
pub struct SessionInfo {
    // 캘린더/메일(UC) 계열
    pub comp_seq: String,
    pub dept_seq: String,
    pub emp_name: String,
    pub email_addr: String,
    pub email_domain: String,
    // 근태(human/ERP) 계열 — UC seq와 별개 코드 체계(ucUserInfo.erp*Seq)
    pub emp_cd: String,
    pub dept_cd: String,
    pub co_cd: String,
}

#[derive(Default)]
struct SessionCache {
    info: SessionInfo,
    fetched_at: Option<Instant>,
}

/// 캘린더 목록 캐시. 저장은 여기(클라이언트 상태), 조회 API 호출은 `modules::calendar`가 한다.
#[derive(Default)]
struct CalendarCache {
    list: Vec<serde_json::Value>,
    fetched_at: Option<Instant>,
}

/// 전사 사원 명부 캐시. 조립(부서 전수 순회)은 `modules::org`가 한다.
#[derive(Default)]
struct RosterCache {
    list: Vec<serde_json::Value>,
    fetched_at: Option<Instant>,
}

/// 본인 표시정보(부서명/직책/직급) 캐시. 조회(gw102A02)는 `modules::org::my_profile`이 한다.
/// gw050A02 세션정보에는 **직책·직급·부서명이 없어서**(코드/seq만) 조직도에서 한 번 더 가져온다.
/// 인사이동 주기를 고려해 명부와 같은 TTL(30분).
#[derive(Default)]
struct ProfileCache {
    profile: Option<serde_json::Value>,
    fetched_at: Option<Instant>,
}

pub struct GwClient {
    /// 크레덴셜은 lazy 로드/캐시. 시작 시 취득 성공하면 Some로 seed, 실패면 None으로 시작(서버는 뜬다).
    /// 미로드 상태에서 도구 호출 시 브라우저 쿠키에서 재취득 시도 → 실패하면 로그인 안내를 반환.
    creds: RwLock<Option<Creds>>,
    http: reqwest::Client,
    base: String,
    session: RwLock<SessionCache>,
    calendars: RwLock<CalendarCache>,
    roster: RwLock<RosterCache>,
    profile: RwLock<ProfileCache>,
}

impl GwClient {
    pub fn new(initial: Option<Creds>) -> Self {
        Self {
            creds: RwLock::new(initial),
            http: reqwest::Client::new(),
            base: "https://gw.innogrid.com".to_string(),
            session: RwLock::new(SessionCache::default()),
            calendars: RwLock::new(CalendarCache::default()),
            roster: RwLock::new(RosterCache::default()),
            profile: RwLock::new(ProfileCache::default()),
        }
    }

    /// 유효한(TTL 내) 본인 표시정보 캐시. 없거나 만료면 None → `modules::org::my_profile`이 재조회.
    pub fn cached_profile(&self) -> Option<serde_json::Value> {
        let cache = self.profile.read().ok()?;
        cache
            .fetched_at
            .is_some_and(|t| t.elapsed() < ROSTER_TTL)
            .then(|| cache.profile.clone())
            .flatten()
    }

    /// 조회한 본인 표시정보를 캐시에 넣는다.
    pub fn set_profile(&self, p: serde_json::Value) {
        if let Ok(mut cache) = self.profile.write() {
            cache.profile = Some(p);
            cache.fetched_at = Some(Instant::now());
        }
    }

    /// 유효한(TTL 내) 사원 명부 캐시. 없거나 만료면 None → 호출부가 부서 순회로 재조립한다.
    pub fn cached_roster(&self) -> Option<Vec<serde_json::Value>> {
        let cache = self.roster.read().ok()?;
        cache
            .fetched_at
            .is_some_and(|t| t.elapsed() < ROSTER_TTL)
            .then(|| cache.list.clone())
    }

    /// 조립한 사원 명부를 캐시에 넣는다.
    pub fn set_roster(&self, list: Vec<serde_json::Value>) {
        if let Ok(mut cache) = self.roster.write() {
            cache.list = list;
            cache.fetched_at = Some(Instant::now());
        }
    }

    /// 유효한(TTL 내) 캘린더 목록 캐시. 없거나 만료면 None → 호출부가 sc111A02로 조회한다.
    pub fn cached_calendars(&self) -> Option<Vec<serde_json::Value>> {
        let cache = self.calendars.read().ok()?;
        cache
            .fetched_at
            .is_some_and(|t| t.elapsed() < CALENDAR_TTL)
            .then(|| cache.list.clone())
    }

    /// 조회한 캘린더 목록을 캐시에 넣는다.
    pub fn set_calendars(&self, list: Vec<serde_json::Value>) {
        if let Ok(mut cache) = self.calendars.write() {
            cache.list = list;
            cache.fetched_at = Some(Instant::now());
        }
    }

    /// 크레덴셜을 lazy 보장. 캐시에 있으면 반환, 없으면 브라우저 쿠키에서 취득 시도.
    /// 실패 시 로그인 안내 에러를 그대로 전파(도구 응답으로 사용자에게 노출). 실패는 캐시하지
    /// 않으므로, 사용자가 로그인한 뒤엔 재시작 없이 다음 호출에서 자동 복구된다.
    fn creds(&self) -> Result<Creds> {
        if let Ok(guard) = self.creds.read()
            && let Some(c) = guard.as_ref()
        {
            return Ok(c.clone());
        }
        let fresh = creds::from_browser()?;
        if let Ok(mut guard) = self.creds.write() {
            *guard = Some(fresh.clone());
        }
        Ok(fresh)
    }

    /// 캐시된 크레덴셜을 버리고 브라우저에서 다시 읽는다. 성공하면 true.
    /// 401을 받은 직후 `retry_once_on_401`이 호출당 최대 한 번 부른다.
    ///
    /// 실패해도 캐시는 비운 채로 둔다 — `creds()`가 다음 도구 호출에서 다시 시도하므로,
    /// 사용자가 그 사이 로그인하면 재시작 없이 복구된다.
    /// 주의: `INNO_CREED_AUTH_TOKEN`/`INNO_CREED_SIGN_KEY`를 쓰는 환경은 재취득해도 **같은 값**이
    /// 돌아온다 — 그래서 재시도 상한(1회)이 정책의 핵심이다.
    fn reacquire_creds(&self) -> bool {
        if let Ok(mut guard) = self.creds.write() {
            *guard = None;
        }
        let Ok(fresh) = creds::from_browser() else {
            return false;
        };
        if let Ok(mut guard) = self.creds.write() {
            *guard = Some(fresh);
        }
        true
    }

    /// **모든 전송 함수의 공통 출구** — `signed()`로 요청을 만들어 보내고, 401이면 크레덴셜을
    /// 재취득해 같은 요청을 1회 재시도한다(`retry_once_on_401`).
    ///
    /// `decorate`는 Content-Type·바디처럼 호출부마다 다른 마무리를 붙인다. 재시도 때 요청을
    /// **다시 만들어야** 하므로(빌더는 `send()`가 소비한다) 클로저로 받는다.
    async fn send_signed(
        &self,
        method: reqwest::Method,
        sign_path: &str,
        url_path: &str,
        decorate: impl Fn(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    ) -> Result<reqwest::Response> {
        retry_once_on_401(
            |r: &reqwest::Response| r.status() == reqwest::StatusCode::UNAUTHORIZED,
            || self.reacquire_creds(),
            || async {
                let req = decorate(self.signed(method.clone(), sign_path, url_path)?);
                Ok(req.send().await?)
            },
        )
        .await
    }

    /// 비 2xx 응답을 사용자용 에러로 만든다. **상태코드를 먼저 보고 그 다음 본문을 읽는다** —
    /// 본문이 JSON이 아니어도 상태·원문을 잃지 않는다.
    ///
    /// 401은 크레덴셜 문제가 확정이므로(실측: 토큰 훼손·빈 토큰·서명키 오류가 모두 401) 서버
    /// 원문 대신 재로그인 안내를 앞세운다. 서버 메시지("인증 값을 레디스에서 찾을 수 없습니다" 등)는
    /// 사용자가 해석할 수 없어 진단용으로만 덧붙인다. `resultCode`(140=토큰 없음, 112=서명 불일치)도
    /// 처방이 같아 분기하지 않고 문구에만 싣는다.
    async fn http_error(path: &str, status: reqwest::StatusCode, resp: reqwest::Response) -> anyhow::Error {
        let body = resp.text().await.unwrap_or_default();
        let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let code = v.get("resultCode").and_then(|c| c.as_i64()).unwrap_or(-1);
        let msg = v
            .get("resultMsg")
            .and_then(|m| m.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| body.chars().take(300).collect());
        if status == reqwest::StatusCode::UNAUTHORIZED {
            anyhow!(
                "로그인이 필요합니다 — gw.innogrid.com 세션이 만료되었거나 유효하지 않습니다({path}).\n\
                 Chrome 또는 Firefox로 https://gw.innogrid.com 에 로그인하면 서버 재시작 없이 다음 호출에서 복구됩니다.\n\
                 (재로그인 후에도 같은 안내가 나오면 INNO_CREED_AUTH_TOKEN/INNO_CREED_SIGN_KEY 환경변수가 옛 값으로 고정돼 있는지 확인하세요.)\n\
                 서버 응답: http=401 resultCode={code} msg={msg}"
            )
        } else {
            anyhow!("api {path} 실패: http={status} resultCode={code} msg={msg}")
        }
    }

    /// 세션 정보를 lazy 보장. 캐시가 유효(10분 TTL)하면 그대로 반환, 없거나 만료면 gw050A02로
    /// 재조회 후 캐시. 모든 tool 핸들러가 진입 시 1회 호출한다(값이 필요할 때 알아서 채움).
    pub async fn ensure_session(&self) -> Result<()> {
        if let Ok(cache) = self.session.read()
            && cache.fetched_at.is_some_and(|t| t.elapsed() < SESSION_TTL)
        {
            return Ok(());
        }
        // fetch는 잠금을 잡지 않은 채로(await 동안 락 보유 금지). 동시 진입 시 중복 조회는
        // 허용(gw050A02는 부작용 없는 조회) — 마지막 쓰기가 캐시를 갱신.
        let info = self.fetch_session().await?;
        let mut cache = self
            .session
            .write()
            .map_err(|_| anyhow!("세션 캐시 잠금 실패"))?;
        cache.info = info;
        cache.fetched_at = Some(Instant::now());
        Ok(())
    }

    /// gw050A02: SSO 세션정보 조회. Bearer 인증만으로 "이미 로그인된 사용자"의 sessionInfo를
    /// 반환한다(별도 CSRF 토큰 불필요). ucUserInfo 하나에 UC(compSeq/deptSeq/email) +
    /// ERP(erp*Seq = 근태 empCd/deptCd/coCd)가 모두 들어있다.
    async fn fetch_session(&self) -> Result<SessionInfo> {
        let data = self
            .call_form("/gw/gw050A02", &[("a10Domain", self.base.as_str())])
            .await?;
        let uc = data
            .get("sessionInfo")
            .and_then(|s| s.get("ucUserInfo"))
            .ok_or_else(|| anyhow!("세션 취득 실패: gw050A02 응답에 sessionInfo.ucUserInfo 없음"))?;
        let g = |k: &str| uc.get(k).and_then(|v| v.as_str()).unwrap_or_default().to_string();
        Ok(SessionInfo {
            comp_seq: g("compSeq"),
            dept_seq: g("deptSeq"),
            emp_name: g("empName"),
            email_addr: g("emailAdd"),
            email_domain: g("emailDomain"),
            emp_cd: g("erpEmpSeq"),
            dept_cd: g("erpDeptSeq"),
            co_cd: g("erpCompSeq"),
        })
    }

    /// **모든 요청의 단일 관문** — 크레덴셜 취득 → transaction-id/timestamp 생성 →
    /// `wehago-sign` 계산 → 인증 헤더 4종 주입까지 마친 요청 빌더를 돌려준다.
    ///
    /// 서명 규격(`docs/architecture.md` §5)이 두 곳에 있으면 한쪽만 고쳤을 때 **그 경로만 401**이
    /// 되고, 잘 안 쓰이는 경로(예: probe 전용 `call_raw`)일수록 한참 뒤에야 드러난다.
    /// 그래서 아래 전송 함수 6개는 전부 이 함수를 거친다. **새 전송 함수도 반드시 여기를 통과할 것.**
    ///
    /// - `sign_path`: 서명 대상 pathname(**쿼리 제외** — 프론트 관례).
    /// - `url_path`: 실제 요청 경로. SSE처럼 쿼리스트링이 붙는 경우만 `sign_path`와 달라진다.
    /// - Content-Type·바디는 붙이지 않는다 — JSON/multipart/form-urlencoded로 제각각이라 호출부 몫.
    fn signed(
        &self,
        method: reqwest::Method,
        sign_path: &str,
        url_path: &str,
    ) -> Result<reqwest::RequestBuilder> {
        let cr = self.creds()?;
        let tid = sign::transaction_id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
            .to_string();
        let sig = sign::wehago_sign(&cr.auth_token, &tid, &ts, sign_path, &cr.sign_key);
        Ok(self
            .http
            .request(method, format!("{}{}", self.base, url_path))
            .header("Authorization", format!("Bearer {}", cr.auth_token))
            .header("timestamp", ts)
            .header("transaction-id", tid)
            .header("wehago-sign", sig))
    }

    /// JSON body POST 후 성공 판정(resultCode ∈ {0,200}) → resultData 반환.
    pub async fn call(&self, path: &str, body: &Value) -> Result<Value> {
        let resp = self
            .send_signed(reqwest::Method::POST, path, path, |b| {
                b.header("Content-Type", "application/json").json(body)
            })
            .await?;

        let status = resp.status();
        if !status.is_success() {
            return Err(Self::http_error(path, status, resp).await);
        }
        let v: Value = resp.json().await?;
        let code = v.get("resultCode").and_then(|c| c.as_i64()).unwrap_or(-1);
        if !(code == 0 || code == 200) {
            let msg = v
                .get("resultMsg")
                .and_then(|m| m.as_str())
                .unwrap_or("(no msg)");
            bail!("api {path} 실패: http={status} resultCode={code} msg={msg}");
        }
        Ok(v.get("resultData").cloned().unwrap_or(Value::Null))
    }

    /// call()과 동일 서명·전송이지만 **성공판정 없이 전체 응답 봉투(resultCode/resultMsg/resultData 포함)를
    /// 그대로 반환**. 2099 같은 실패 응답의 resultData까지 보려는 디버그/probe 용도.
    pub async fn call_raw(&self, path: &str, body: &Value) -> Result<Value> {
        let resp = self
            .send_signed(reqwest::Method::POST, path, path, |b| {
                let mut req = b.header("Content-Type", "application/json");
                // 진단용: 브라우저 헤더 재현 실험(Cookie/Referer/User-Agent).
                if let Ok(cookie) = std::env::var("PROBE_COOKIE")
                    && !cookie.is_empty()
                {
                    req = req.header("Cookie", cookie);
                }
                if std::env::var("PROBE_BROWSER_HDR").is_ok() {
                    req = req
                        .header("Referer", "https://gw.innogrid.com/")
                        .header("User-Agent", "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0.0.0 Safari/537.36")
                        .header("sec-ch-ua", "\"Not;A=Brand\";v=\"8\", \"Chromium\";v=\"150\", \"Google Chrome\";v=\"150\"")
                        .header("sec-ch-ua-mobile", "?0")
                        .header("sec-ch-ua-platform", "\"macOS\"");
                }
                req.json(body)
            })
            .await?;
        let status = resp.status();
        // 성공 판정을 하지 않는 진단용이라 상태로 걸러내지 않는다 — 실패 봉투를 보는 것이 목적.
        let v: Value = resp.json().await?;
        Ok(json!({ "http": status.as_u16(), "response": v }))
    }

    /// multipart/form-data POST. 서명은 동일(4종 헤더), Content-Type은 reqwest가 boundary와
    /// 함께 자동 설정. 응답은 표준 봉투가 아닐 수 있어(예: mail014A04) raw Value로 반환 →
    /// 성공 판정은 호출부 몫.
    /// `make_form`은 **폼을 만드는 방법**이지 만들어진 폼이 아니다 — `multipart::Form`은 `Clone`이
    /// 아니고 `send()`가 소비하므로, 401 재시도 때 같은 요청을 다시 만들려면 재조립이 필요하다.
    /// (호출당 최대 2회 불린다.)
    pub async fn call_multipart(
        &self,
        path: &str,
        make_form: impl Fn() -> reqwest::multipart::Form,
    ) -> Result<Value> {
        let resp = self
            .send_signed(reqwest::Method::POST, path, path, |b| b.multipart(make_form()))
            .await?;

        let status = resp.status();
        if !status.is_success() {
            return Err(Self::http_error(path, status, resp).await);
        }
        Ok(resp.json().await?)
    }

    /// x-www-form-urlencoded POST 후 바이너리 응답을 파일로 저장(ECM 파일 다운로드).
    /// 성공 시 (저장 바이트수, content-disposition 파일명). 실패 시 서버가 JSON 봉투를
    /// 주므로 content-type이 json이면 에러로 처리.
    pub async fn download_form(
        &self,
        path: &str,
        params: &[(&str, &str)],
        out_path: &str,
    ) -> Result<(u64, Option<String>)> {
        let resp = self
            .send_signed(reqwest::Method::POST, path, path, |b| {
                b.header("Content-Type", "application/x-www-form-urlencoded")
                    .body(form_urlencode(params))
            })
            .await?;

        let status = resp.status();
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let filename = resp
            .headers()
            .get("content-disposition")
            .and_then(|v| v.to_str().ok())
            .and_then(parse_cd_filename);
        if !status.is_success() {
            return Err(Self::http_error(path, status, resp).await);
        }
        let bytes = resp.bytes().await?;

        // 200인데 JSON이면 서버가 실패 봉투를 준 것(파일이 아님).
        if ct.contains("json") {
            let snippet = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]);
            bail!("다운로드 실패({path}): http={status} ct={ct} body={snippet}");
        }
        std::fs::write(out_path, &bytes)?;
        Ok((bytes.len() as u64, filename))
    }

    /// x-www-form-urlencoded POST(gw050A02 등). 서명 헤더는 call()과 동일(body 무관).
    pub async fn call_form(&self, path: &str, params: &[(&str, &str)]) -> Result<Value> {
        let resp = self
            .send_signed(reqwest::Method::POST, path, path, |b| {
                b.header("Content-Type", "application/x-www-form-urlencoded")
                    .body(form_urlencode(params))
            })
            .await?;

        let status = resp.status();
        if !status.is_success() {
            return Err(Self::http_error(path, status, resp).await);
        }
        let v: Value = resp.json().await?;
        let code = v.get("resultCode").and_then(|c| c.as_i64()).unwrap_or(-1);
        if !(code == 0 || code == 200) {
            let msg = v
                .get("resultMsg")
                .and_then(|m| m.as_str())
                .unwrap_or("(no msg)");
            bail!("api {path} 실패: http={status} resultCode={code} msg={msg}");
        }
        Ok(v.get("resultData").cloned().unwrap_or(Value::Null))
    }

    /// GET + SSE(text/event-stream) 호출. eap107A25(임시보관 삭제)처럼 파라미터가 쿼리스트링에
    /// 있고 응답이 `data:{...}` 스트림인 엔드포인트용. 서명은 pathname만(쿼리 제외 — 프론트 관례).
    /// 스트림의 모든 `data:` JSON 이벤트를 파싱해 Vec로 반환(성공 판정은 호출부 몫).
    pub async fn call_get_sse(&self, sign_path: &str, path_with_query: &str) -> Result<Vec<Value>> {
        // 서명은 pathname(sign_path)으로, 요청은 쿼리 포함 경로로 — 유일하게 둘이 다른 경로다.
        let resp = self
            .send_signed(reqwest::Method::GET, sign_path, path_with_query, |b| {
                b.header("Accept", "text/event-stream")
            })
            .await?;

        let status = resp.status();
        if !status.is_success() {
            return Err(Self::http_error(sign_path, status, resp).await);
        }
        let text = resp.text().await?;
        let events: Vec<Value> = text
            .lines()
            .filter_map(|l| l.trim_start().strip_prefix("data:"))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| serde_json::from_str::<Value>(s).ok())
            .collect();
        if events.is_empty() {
            bail!("SSE 응답 파싱 실패({sign_path}): body={}", &text[..text.len().min(300)]);
        }
        Ok(events)
    }

    /// 공통 companyInfo. groupSeq/empSeq는 authToken, 나머지는 세션 캐시(ensure_session).
    pub fn company_info(&self) -> Value {
        let c = self.session.read().unwrap();
        json!({
            "compSeq": c.info.comp_seq,
            "groupSeq": self.group_seq(),
            "deptSeq": c.info.dept_seq,
            "emailAddr": c.info.email_addr,
            "emailDomain": c.info.email_domain
        })
    }

    /// authToken 원문. 크리덴셜 미취득이면 빈 문자열(후속 call()이 로그인 안내를 반환).
    pub fn auth_token(&self) -> String {
        self.creds().map(|c| c.auth_token).unwrap_or_default()
    }

    pub fn group_seq(&self) -> String {
        self.auth_token().split('|').next().unwrap_or("").to_string()
    }

    pub fn emp_seq(&self) -> String {
        self.auth_token().split('|').nth(1).unwrap_or("").to_string()
    }

    /// 로그인 ID(=메일 로컬파트). 세션 캐시(ensure_session)에서.
    pub fn email_addr(&self) -> String {
        self.session.read().unwrap().info.email_addr.clone()
    }

    pub fn email_domain(&self) -> String {
        self.session.read().unwrap().info.email_domain.clone()
    }

    pub fn emp_name(&self) -> String {
        self.session.read().unwrap().info.emp_name.clone()
    }

    pub fn comp_seq(&self) -> String {
        self.session.read().unwrap().info.comp_seq.clone()
    }

    pub fn dept_seq(&self) -> String {
        self.session.read().unwrap().info.dept_seq.clone()
    }

    /// 근태(human/ERP) 사원 코드. 세션 캐시(ensure_session)에서.
    pub fn emp_cd(&self) -> String {
        self.session.read().unwrap().info.emp_cd.clone()
    }

    pub fn dept_cd(&self) -> String {
        self.session.read().unwrap().info.dept_cd.clone()
    }

    pub fn co_cd(&self) -> String {
        self.session.read().unwrap().info.co_cd.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// reqwest가 default-features=false라 `.form()`이 없어 직접 구현한 인코더.
    /// 규칙: 비예약 문자는 그대로, 공백은 `+`, 나머지는 %XX(대문자 hex).
    /// ⚠️ `approval_submit::encode_uri_component`(공백→%20)와 **규칙이 다르다** — 섞으면 안 된다.
    #[test]
    fn form_urlencode는_공백을_플러스로_바꾼다() {
        assert_eq!(form_urlencode(&[("a", "b c")]), "a=b+c");
        assert_eq!(form_urlencode(&[("k", "-_.~")]), "k=-_.~"); // 비예약 문자는 보존
        assert_eq!(form_urlencode(&[("a", "1"), ("b", "2")]), "a=1&b=2");
        assert_eq!(form_urlencode(&[("q", "가")]), "q=%EA%B0%80"); // UTF-8 바이트별 %XX
        assert_eq!(form_urlencode(&[("v", "a&b=c")]), "v=a%26b%3Dc"); // 구분자 이스케이프
        assert_eq!(form_urlencode(&[]), "");
    }

    #[test]
    #[allow(non_snake_case)] // 이름 속 `RFC5987` — 대문자를 살려야 뜻이 통하는 표기라 소문자로 풀지 않는다
    fn parse_cd_filename은_RFC5987을_우선한다() {
        // filename* 이 있으면 그쪽(퍼센트 디코드)
        assert_eq!(
            parse_cd_filename("attachment; filename=\"fallback.txt\"; filename*=UTF-8''%EA%B0%80.txt"),
            Some("가.txt".to_string())
        );
        // 없으면 filename=
        assert_eq!(
            parse_cd_filename("attachment; filename=\"보고서.pdf\""),
            Some("보고서.pdf".to_string())
        );
        assert_eq!(parse_cd_filename("attachment"), None);
        assert_eq!(parse_cd_filename(""), None);
    }

    /// 재시도 정책 테스트용 대역. `T`를 상태코드(u16)로 두고 전송 횟수를 센다.
    /// 실제 전송·크레덴셜 재취득에서 분리돼 있어 네트워크도 브라우저 쿠키도 필요 없다.
    struct Spy {
        /// 매 전송이 돌려줄 상태코드(앞에서부터 하나씩 소비).
        responses: std::cell::RefCell<std::collections::VecDeque<Result<u16>>>,
        sends: std::cell::Cell<u32>,
        reacquires: std::cell::Cell<u32>,
    }

    impl Spy {
        fn new(responses: Vec<Result<u16>>) -> Self {
            Self {
                responses: std::cell::RefCell::new(responses.into()),
                sends: std::cell::Cell::new(0),
                reacquires: std::cell::Cell::new(0),
            }
        }
        async fn run(&self, reacquire_ok: bool) -> Result<u16> {
            retry_once_on_401(
                |s: &u16| *s == 401,
                || {
                    self.reacquires.set(self.reacquires.get() + 1);
                    reacquire_ok
                },
                || async {
                    self.sends.set(self.sends.get() + 1);
                    self.responses
                        .borrow_mut()
                        .pop_front()
                        .unwrap_or_else(|| bail!("전송 횟수 상한 초과 — 대역에 준비된 응답이 없다"))
                },
            )
            .await
        }
    }

    #[tokio::test]
    async fn 성공응답은_재시도도_재취득도_하지_않는다() {
        let spy = Spy::new(vec![Ok(200)]);
        assert_eq!(spy.run(true).await.unwrap(), 200);
        assert_eq!(spy.sends.get(), 1);
        assert_eq!(spy.reacquires.get(), 0);
    }

    /// 401 외의 실패(403·500 등)는 크레덴셜 문제가 아니므로 건드리지 않는다.
    #[tokio::test]
    async fn 사백일이_아닌_실패는_재시도하지_않는다() {
        for status in [400u16, 403, 404, 500] {
            let spy = Spy::new(vec![Ok(status)]);
            assert_eq!(spy.run(true).await.unwrap(), status);
            assert_eq!(spy.sends.get(), 1, "http={status}");
            assert_eq!(spy.reacquires.get(), 0, "http={status}");
        }
    }

    #[tokio::test]
    async fn 사백일이면_재취득후_한번_재시도한다() {
        let spy = Spy::new(vec![Ok(401), Ok(200)]);
        assert_eq!(spy.run(true).await.unwrap(), 200);
        assert_eq!(spy.sends.get(), 2);
        assert_eq!(spy.reacquires.get(), 1);
    }

    /// 재시도까지 401이면 **거기서 끝난다**. 세 번째 전송이 있었다면 대역이 준비한 응답이
    /// 떨어져 Err가 났을 것이므로, Ok(401)이라는 사실 자체가 상한을 증명한다.
    #[tokio::test]
    async fn 재시도도_사백일이면_더_재시도하지_않는다() {
        let spy = Spy::new(vec![Ok(401), Ok(401)]);
        assert_eq!(spy.run(true).await.unwrap(), 401);
        assert_eq!(spy.sends.get(), 2);
        assert_eq!(spy.reacquires.get(), 1);
    }

    /// 브라우저 미로그인 등으로 재취득이 실패하면 재시도는 무의미 — 첫 401을 그대로 돌려준다.
    #[tokio::test]
    async fn 재취득_실패하면_재시도하지_않고_첫응답을_돌려준다() {
        let spy = Spy::new(vec![Ok(401), Ok(200)]);
        assert_eq!(spy.run(false).await.unwrap(), 401);
        assert_eq!(spy.sends.get(), 1);
        assert_eq!(spy.reacquires.get(), 1);
    }

    /// 전송 자체가 실패(네트워크 오류 등)하면 인증 문제가 아니므로 재취득으로 새지 않는다.
    #[tokio::test]
    async fn 전송_에러는_그대로_전파된다() {
        let spy = Spy::new(vec![Err(anyhow!("연결 실패"))]);
        assert!(spy.run(true).await.is_err());
        assert_eq!(spy.sends.get(), 1);
        assert_eq!(spy.reacquires.get(), 0);
    }

    #[test]
    fn pct_decode는_잘린_시퀀스를_원문으로_둔다() {
        assert_eq!(pct_decode("%EA%B0%80"), "가");
        assert_eq!(pct_decode("plain"), "plain");
        assert_eq!(pct_decode("a%2"), "a%2");   // 잘린 %XX → 그대로(패닉 없음)
        assert_eq!(pct_decode("%zz"), "%zz");   // hex 아님 → 그대로
        assert_eq!(pct_decode(""), "");
    }
}
