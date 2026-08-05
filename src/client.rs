//! gw API HTTP 클라이언트: 헤더 서명 + 표준 응답봉투({resultCode,resultMsg,resultData}) 파싱.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::creds::{self, Creds};
use crate::sign;

/// 세션 정보 인메모리 캐시 TTL(10분). 만료 시 `ensure_session()`이 gw050A02로 재조회.
const SESSION_TTL: Duration = Duration::from_secs(600);

/// 캘린더 목록 인메모리 캐시 TTL(10분). 조회(list_events)·등록(create_event) 양쪽이 같은
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
            .signed(reqwest::Method::POST, path, path)?
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await?;

        let status = resp.status();
        let v: Value = resp.json().await?;
        let code = v.get("resultCode").and_then(|c| c.as_i64()).unwrap_or(-1);
        if !status.is_success() || !(code == 0 || code == 200) {
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
        let mut req = self
            .signed(reqwest::Method::POST, path, path)?
            .header("Content-Type", "application/json");
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
        let resp = req.json(body).send().await?;
        let status = resp.status();
        let v: Value = resp.json().await?;
        Ok(json!({ "http": status.as_u16(), "response": v }))
    }

    /// multipart/form-data POST. 서명은 동일(4종 헤더), Content-Type은 reqwest가 boundary와
    /// 함께 자동 설정. 응답은 표준 봉투가 아닐 수 있어(예: mail014A04) raw Value로 반환 →
    /// 성공 판정은 호출부 몫.
    pub async fn call_multipart(&self, path: &str, form: reqwest::multipart::Form) -> Result<Value> {
        let resp = self
            .signed(reqwest::Method::POST, path, path)?
            .multipart(form)
            .send()
            .await?;

        let status = resp.status();
        let v: Value = resp.json().await?;
        if !status.is_success() {
            bail!("api {path} 실패: http={status} body={v}");
        }
        Ok(v)
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
            .signed(reqwest::Method::POST, path, path)?
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_urlencode(params))
            .send()
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
        let bytes = resp.bytes().await?;

        if !status.is_success() || ct.contains("json") {
            let snippet = String::from_utf8_lossy(&bytes[..bytes.len().min(300)]);
            bail!("다운로드 실패({path}): http={status} ct={ct} body={snippet}");
        }
        std::fs::write(out_path, &bytes)?;
        Ok((bytes.len() as u64, filename))
    }

    /// x-www-form-urlencoded POST(gw050A02 등). 서명 헤더는 call()과 동일(body 무관).
    pub async fn call_form(&self, path: &str, params: &[(&str, &str)]) -> Result<Value> {
        let resp = self
            .signed(reqwest::Method::POST, path, path)?
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(form_urlencode(params))
            .send()
            .await?;

        let status = resp.status();
        let v: Value = resp.json().await?;
        let code = v.get("resultCode").and_then(|c| c.as_i64()).unwrap_or(-1);
        if !status.is_success() || !(code == 0 || code == 200) {
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
            .signed(reqwest::Method::GET, sign_path, path_with_query)?
            .header("Accept", "text/event-stream")
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            bail!("api {sign_path} 실패: http={status} body={}", &text[..text.len().min(300)]);
        }
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
