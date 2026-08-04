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
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(h) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(h);
                i += 3;
                continue;
            }
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

pub struct GwClient {
    /// 크레덴셜은 lazy 로드/캐시. 시작 시 취득 성공하면 Some로 seed, 실패면 None으로 시작(서버는 뜬다).
    /// 미로드 상태에서 도구 호출 시 브라우저 쿠키에서 재취득 시도 → 실패하면 로그인 안내를 반환.
    creds: RwLock<Option<Creds>>,
    http: reqwest::Client,
    base: String,
    session: RwLock<SessionCache>,
    calendars: RwLock<CalendarCache>,
}

impl GwClient {
    pub fn new(initial: Option<Creds>) -> Self {
        Self {
            creds: RwLock::new(initial),
            http: reqwest::Client::new(),
            base: "https://gw.innogrid.com".to_string(),
            session: RwLock::new(SessionCache::default()),
            calendars: RwLock::new(CalendarCache::default()),
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
        if let Ok(guard) = self.creds.read() {
            if let Some(c) = guard.as_ref() {
                return Ok(c.clone());
            }
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
        if let Ok(cache) = self.session.read() {
            if cache.fetched_at.is_some_and(|t| t.elapsed() < SESSION_TTL) {
                return Ok(());
            }
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

    /// JSON body POST 후 성공 판정(resultCode ∈ {0,200}) → resultData 반환.
    pub async fn call(&self, path: &str, body: &Value) -> Result<Value> {
        let cr = self.creds()?;
        let tid = sign::transaction_id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
            .to_string();
        let sig = sign::wehago_sign(&cr.auth_token, &tid, &ts, path, &cr.sign_key);

        let resp = self
            .http
            .post(format!("{}{}", self.base, path))
            .header("Authorization", format!("Bearer {}", cr.auth_token))
            .header("timestamp", &ts)
            .header("transaction-id", &tid)
            .header("wehago-sign", sig)
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

    /// multipart/form-data POST. 서명은 동일(4종 헤더), Content-Type은 reqwest가 boundary와
    /// 함께 자동 설정. 응답은 표준 봉투가 아닐 수 있어(예: mail014A04) raw Value로 반환 →
    /// 성공 판정은 호출부 몫.
    pub async fn call_multipart(&self, path: &str, form: reqwest::multipart::Form) -> Result<Value> {
        let cr = self.creds()?;
        let tid = sign::transaction_id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
            .to_string();
        let sig = sign::wehago_sign(&cr.auth_token, &tid, &ts, path, &cr.sign_key);

        let resp = self
            .http
            .post(format!("{}{}", self.base, path))
            .header("Authorization", format!("Bearer {}", cr.auth_token))
            .header("timestamp", &ts)
            .header("transaction-id", &tid)
            .header("wehago-sign", sig)
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
        let cr = self.creds()?;
        let tid = sign::transaction_id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
            .to_string();
        let sig = sign::wehago_sign(&cr.auth_token, &tid, &ts, path, &cr.sign_key);

        let resp = self
            .http
            .post(format!("{}{}", self.base, path))
            .header("Authorization", format!("Bearer {}", cr.auth_token))
            .header("timestamp", &ts)
            .header("transaction-id", &tid)
            .header("wehago-sign", sig)
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
        let cr = self.creds()?;
        let tid = sign::transaction_id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
            .to_string();
        let sig = sign::wehago_sign(&cr.auth_token, &tid, &ts, path, &cr.sign_key);

        let resp = self
            .http
            .post(format!("{}{}", self.base, path))
            .header("Authorization", format!("Bearer {}", cr.auth_token))
            .header("timestamp", &ts)
            .header("transaction-id", &tid)
            .header("wehago-sign", sig)
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
        let cr = self.creds()?;
        let tid = sign::transaction_id();
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs()
            .to_string();
        let sig = sign::wehago_sign(&cr.auth_token, &tid, &ts, sign_path, &cr.sign_key);

        let resp = self
            .http
            .get(format!("{}{}", self.base, path_with_query))
            .header("Authorization", format!("Bearer {}", cr.auth_token))
            .header("timestamp", &ts)
            .header("transaction-id", &tid)
            .header("wehago-sign", sig)
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
