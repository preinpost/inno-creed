//! gw API HTTP 클라이언트: 헤더 서명 + 표준 응답봉투({resultCode,resultMsg,resultData}) 파싱.

use std::sync::{Condvar, Mutex, RwLock};
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

/// 진행 중인 취득에 편승한 쪽이 기다리는 상한(기본값 — 실제 값은 `GwClient::wait_cap` 필드).
/// 취득 자체를 취소하지는 못하고(블로킹 syscall이다) **기다리는 쪽만** 포기한다.
/// 30초인 이유: macOS 키체인 프롬프트처럼 사람 입력을 기다리는 경로가 있어 몇 초로는 짧고,
/// 그렇다고 무한정 매달리면 도구 호출이 응답을 잃는다.
const ACQUIRE_WAIT_CAP: Duration = Duration::from_secs(30);

/// 취득자가 패닉해 언와인드했을 때 편승자에게 돌려주는 사유.
const ACQUIRE_PANICKED: &str = "크레덴셜 취득이 도중에 중단됐습니다(패닉) — 같은 호출을 다시 시도하세요.";

/// 비행 **하나**의 결과 슬롯. 한 번만 쓰이고 그 뒤로 불변이다.
///
/// ⚠️ **비행마다 새로 만들고, 편승자는 진입할 때 이 `Arc`를 복제해 들고 간다.** 이것이 핵심이다 —
/// 결과를 `Flight` 안의 슬롯 **하나**에 두고 비행마다 갈아끼우던 구조는, 뒤늦게 깬 편승자가
/// **성공한 비행에 편승하고도 결과를 잃는** 회귀를 냈다(다음 비행이 그 자리를 비워버린다).
/// 자기 비행의 슬롯을 손에 쥐고 있으면 다음 비행이 무엇을 하든 영향받지 않는다.
type FlightSlot = std::sync::Arc<std::sync::OnceLock<Result<Creds, String>>>;

/// 크레덴셜 취득의 단일 비행 상태.
///
/// 취득은 파일 복사 + 외부 프로세스 spawn + SQLite라 값싸지 않은데, 401을 만난 요청들이
/// 각자 부르면 그 비용이 동시 요청 수만큼 곱해진다. 한 번만 실행하고 결과를 나눠 갖는다.
#[derive(Default)]
struct Flight {
    /// 지금 누군가 취득 중인가.
    running: bool,
    /// 완료될 때마다 증가.
    ///
    /// ⚠️ **역할을 정확히 적는다** — 슬롯 도입 뒤로 이 값은 **판정에 쓰이지 않는다.** 편승자는
    /// 자기 슬롯에서 결과를 읽으므로 결과의 정확성은 이 값과 무관하다. 남은 역할은 하나다:
    /// 편승자가 깼을 때 **다음 비행이 이미 시작돼 `running`이 다시 참**인 경우를 `running`만으로는
    /// 구분할 수 없어, 자기 비행이 끝났다는 것을 여기서 안다(없으면 남의 비행이 끝날 때까지
    /// 불필요하게 더 기다리고, 비행이 끊임없이 이어지면 대기 상한까지 굶을 수 있다).
    /// 즉 **정확성이 아니라 대기 시간의 상한**을 지킨다.
    generation: u64,
    /// 지금 이 비행에 편승해 대기 중인 수. 진단·테스트 관측용이며 판정에는 쓰지 않는다
    /// (테스트가 "N-1개가 실제로 게이트에 들어왔다"를 sleep 가정 없이 확인하는 근거).
    waiters: usize,
    /// **지금 돌고 있는** 비행의 결과 슬롯. 새 비행이 시작할 때 새 슬롯으로 교체하므로
    /// 새 편승자는 옛 비행의 결과를 절대 집지 않고(② 성질), 이미 편승한 쪽은 자기 슬롯을
    /// 들고 있어 교체와 무관하게 자기 결과를 받는다(① 성질).
    slot: Option<FlightSlot>,
}

/// 비행권 — 취득자가 잡고, **`Drop`에서** 결과를 슬롯에 채우고 `running`을 내리고 `generation`을
/// 올리고 편승자를 깨운다.
///
/// 왜 RAII인가: 락을 놓은 채로 취득을 실행하므로(그래야 편승자가 진입한다) 취득이 패닉하면
/// `running`을 내릴 주체가 없다. 뮤텍스는 이미 풀려 있어 **poison조차 되지 않아** 게이트가
/// 프로세스 수명 내내 잠긴 채 남는다 — 이후 모든 요청이 편승자가 되어 상한까지 기다렸다 실패한다.
/// 언와인드에서도 도는 `Drop`이 유일하게 확실한 해제 지점이다(`creds::TempCopy`와 같은 이유).
struct FlightGuard<'a> {
    client: &'a GwClient,
    /// 이 비행의 결과 슬롯(편승자들이 같은 것을 들고 있다).
    slot: FlightSlot,
    /// 정상 완료 시 `finish()`가 채운다. `None`인 채 `Drop`에 도달했다면 = 언와인드다.
    outcome: Option<Result<Creds, String>>,
}

impl<'a> FlightGuard<'a> {
    fn new(client: &'a GwClient, slot: FlightSlot) -> Self {
        Self { client, slot, outcome: None }
    }

    fn finish(&mut self, r: &Result<Creds>) {
        self.outcome = Some(match r {
            Ok(c) => Ok(c.clone()),
            Err(e) => Err(format!("{e:#}")),
        });
    }
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        // 결과를 **generation을 올리기 전에** 채운다 — 편승자는 락을 잡아야 새 generation을 보므로,
        // 그때는 이 쓰기가 이미 보인다. 결과가 없으면 패닉이다(편승자를 빈손으로 두지 않는다).
        let outcome = self
            .outcome
            .take()
            .unwrap_or_else(|| Err(ACQUIRE_PANICKED.to_string()));
        let _ = self.slot.set(outcome); // 비행당 한 번뿐이라 Err일 수 없다
        {
            let mut st = self.client.flight.lock().unwrap_or_else(|e| e.into_inner());
            st.running = false;
            st.generation = st.generation.wrapping_add(1);
        }
        // 통지는 락을 놓고 — **`notify_all`이어야 한다.** `notify_one`이면 나머지 편승자들이
        // 통지를 못 받고 각자 상한까지 매달린다(동시 401 N건이면 N-1건이 그만큼 얼어붙는다).
        self.client.flight_done.notify_all();
    }
}

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
    /// 취득을 한 번에 하나로 묶는 게이트(`Flight` 주석).
    flight: Mutex<Flight>,
    /// 비행 완료 통지. 편승한 쪽이 여기서 깬다.
    flight_done: Condvar,
    /// 실제 취득 동작. 프로덕션은 `creds::from_browser`이고, 테스트가 호출 횟수를 세는
    /// 대역을 꽂을 수 있도록 필드로 둔다(자유 함수 직접 호출이면 주입 지점이 없다).
    acquirer: Box<dyn Fn() -> Result<Creds> + Send + Sync>,
    /// 편승 대기 상한. 상수를 그대로 쓰지 않고 필드로 둔 것은 **상한 자체를 검증**하기
    /// 위해서다 — 30초짜리 상수는 테스트로 만료를 확인할 수 없다(기본값은 `ACQUIRE_WAIT_CAP`).
    wait_cap: Duration,
    /// 크레덴셜 캐시에 쓴 횟수. 단일 비행의 불변식 **"취득 1회 = 캐시 쓰기 1회, 편승자는 캐시를
    /// 쓰지 않는다"**를 관측 가능하게 만든다(그 불변식이 깨지는 것은 값이 같아 눈에 안 보인다).
    creds_writes: std::sync::atomic::AtomicU64,
    http: reqwest::Client,
    base: String,
    session: RwLock<SessionCache>,
    calendars: RwLock<CalendarCache>,
    roster: RwLock<RosterCache>,
    profile: RwLock<ProfileCache>,
}

impl GwClient {
    pub fn new(initial: Option<Creds>) -> Self {
        Self::with_acquirer(initial, Box::new(creds::from_browser))
    }

    /// `new`의 취득 동작 주입 버전. 프로덕션 경로는 `new` 하나뿐이다.
    fn with_acquirer(
        initial: Option<Creds>,
        acquirer: Box<dyn Fn() -> Result<Creds> + Send + Sync>,
    ) -> Self {
        Self {
            creds: RwLock::new(initial),
            flight: Mutex::new(Flight::default()),
            flight_done: Condvar::new(),
            acquirer,
            wait_cap: ACQUIRE_WAIT_CAP,
            creds_writes: std::sync::atomic::AtomicU64::new(0),
            http: reqwest::Client::new(),
            base: "https://gw.innogrid.com".to_string(),
            session: RwLock::new(SessionCache::default()),
            calendars: RwLock::new(CalendarCache::default()),
            roster: RwLock::new(RosterCache::default()),
            profile: RwLock::new(ProfileCache::default()),
        }
    }

    /// 편승 대기 상한을 바꾼다(테스트 전용). 기본 30초로는 만료 자체를 검증할 수 없다.
    #[cfg(test)]
    fn with_wait_cap(mut self, cap: Duration) -> Self {
        self.wait_cap = cap;
        self
    }

    /// 크레덴셜 캐시에 쓴 누적 횟수(테스트 전용 관측).
    #[cfg(test)]
    fn creds_writes(&self) -> u64 {
        self.creds_writes
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 편승자 `n`명이 **실제로 게이트에 진입할 때까지** 기다린다(테스트 전용).
    /// sleep 길이로 "그때쯤이면 도착했겠지"를 가정하는 대신 상태를 본다 — 부하가 높은 머신에서
    /// 취득자의 hold보다 스레드 기동이 늦어지면 그 가정이 깨져 늦은 스레드가 두 번째 취득자가 된다.
    #[cfg(test)]
    fn wait_for_waiters(&self, n: usize, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            let now = {
                let st = self.flight.lock().unwrap_or_else(|e| e.into_inner());
                st.waiters
            };
            if now >= n {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "편승자 {n}명을 기다렸으나 {now}명만 게이트에 들어왔다"
            );
            std::thread::sleep(Duration::from_millis(1));
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
        if let Some(c) = self.cached_creds() {
            return Ok(c);
        }
        // 캐시 미스도 **게이트를 지난다.** 재취득이 캐시를 비운 사이(아래 `reacquire_creds`)에
        // 401을 만나지도 않은 다른 in-flight 요청들이 `None`을 보고 각자 취득을 시작하는 경로가
        // 여기다 — 한쪽만 막으면 그 창으로 새어 들어온다.
        self.acquire_creds()
    }

    /// 캐시된 크레덴셜(없으면 `None`).
    ///
    /// 락이 poison되면 **값을 그대로 꺼내 쓴다**. 이 락이 지키는 것은 `Option<Creds>` 하나뿐이라
    /// 패닉이 중간 상태를 남길 수 없다(대입 한 줄이 전부다) — 여기서 실패로 처리하면 회복
    /// 불가능한 서버가 되는 대신 얻는 것이 없다.
    fn cached_creds(&self) -> Option<Creds> {
        let guard = self.creds.read().unwrap_or_else(|e| e.into_inner());
        guard.clone()
    }

    fn set_cached_creds(&self, v: Option<Creds>) {
        self.creds_writes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut guard = self.creds.write().unwrap_or_else(|e| e.into_inner());
        *guard = v;
    }

    /// **취득 단일 비행 게이트.** 동시 요청 N개가 취득을 요구해도 실제 취득은 1회이고,
    /// 나머지는 그 결과(성공이든 실패든)를 그대로 나눠 갖는다. 성공하면 캐시까지 여기서 갱신한다
    /// — 캐시 쓰기를 호출부에 맡기면 늦게 깬 편승자가 이후의 새 값을 옛 값으로 덮을 수 있다.
    ///
    /// **근거의 강도를 갈라 적는다(격상 금지).**
    /// - *확인된 것*: 서버측 병렬 dispatch — rmcp 소스로 확정. 그리고 MCP 클라이언트가 한 메시지에
    ///   여러 도구 호출을 담아 보내는 **형태가 존재한다**는 것(이 프로젝트 개발 중 상시 관측).
    /// - *여전히 미관측*: 그 동시 호출이 **이 서버에 동시 요청으로 도달하는지 계측한 기록이 없다.**
    ///   그것들이 **실제로 401을 동시에 만나는 장면**도 관측된 적이 없다.
    ///
    /// 즉 이 게이트는 실측된 사고에 대한 대응이 아니라, 가능성 위에 세운 방어다. 값싸고 되돌릴 수
    /// 있어서 넣었을 뿐이다 — 과대평가하지 말 것. **"동시 요청이 실제로 일어나는지 계측한다"는
    /// 착수 확인은 아직 이행되지 않았다.**
    ///
    /// **블로킹을 그대로 둔 판단**: 취득은 파일 복사·프로세스 spawn·SQLite라 블로킹이고 이 함수는
    /// sync다(호출부 `signed()`도 sync). 그래서 편승자는 **tokio 워커 스레드 위에서** condvar를
    /// 기다린다 — 동시 401이 N건이면 **묶이는 워커는 여전히 N개다**(단일 비행이 줄인 것은
    /// 워커 점유 수가 아니라 실제 취득 *작업량*이다). `spawn_blocking`/전면 async화는 `creds.rs`
    /// 전체를 건드리는 별개 규모라 보류했고, 그때까지 워커 고갈을 막는 것은 대기 상한뿐이다.
    fn acquire_creds(&self) -> Result<Creds> {
        self.acquire_creds_inner(false)
    }

    /// `invalidate=true`면 **게이트 안에서** 캐시를 비운 뒤 취득한다(401 재취득 경로).
    ///
    /// 무효화를 게이트 밖에서 하면 안 되는 이유: 취득자가 새 값을 캐시에 넣고 `running`을
    /// 내리기까지의 창에 다른 스레드가 끼어들어 **방금 채운 값을 지우고** 편승할 수 있다.
    /// 편승자는 캐시를 쓰지 않으므로 캐시가 빈 채로 남고, 다음 호출이 또 새 비행을 만든다 —
    /// 단일 비행이 막으려던 바로 그 중복 취득이다. 진행 중인 비행이 있으면 그 결과가 곧 새 값이라
    /// 애초에 비울 이유도 없다.
    fn acquire_creds_inner(&self, invalidate: bool) -> Result<Creds> {
        let mut st = self.flight.lock().unwrap_or_else(|e| e.into_inner());

        if st.running {
            // 진행 중인 비행에 편승 — 그 비행이 끝날 때까지(=generation이 바뀔 때까지) 기다린다.
            let joined_at = st.generation;
            // ⭐ **자기 비행의 슬롯을 지금 손에 쥔다.** 깬 뒤에 공유 자리를 다시 보면, 그 사이
            // 다음 비행이 시작돼 그 자리가 이미 갈렸을 수 있다(그러면 성공한 비행에 편승하고도
            // 결과를 잃는다 — 연속 재취득에서 대량으로 터졌던 회귀다).
            let slot = st.slot.clone();
            let deadline = Instant::now() + self.wait_cap;
            st.waiters += 1;
            let timed_out = loop {
                if !(st.running && st.generation == joined_at) {
                    break false;
                }
                let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                    break true;
                };
                let (guard, _) = self
                    .flight_done
                    .wait_timeout(st, left)
                    .unwrap_or_else(|e| e.into_inner());
                st = guard;
            };
            st.waiters -= 1;
            if timed_out {
                bail!(
                    "크레덴셜 취득 대기 시간 초과({}초) — 다른 요청의 취득이 끝나지 않았습니다. \
                     브라우저 키체인/키링 프롬프트가 떠 있는지 확인한 뒤 다시 시도하세요.",
                    self.wait_cap.as_secs()
                );
            }
            // 슬롯은 **내가 편승한 그 비행**의 것이다 — `FlightGuard::drop`이 generation을 올리기
            // 전에 채우므로, 새 generation을 본 시점에 값은 이미 들어 있다.
            drop(st); // 남의 결과를 읽는 데 게이트 락이 필요 없다
            return match slot.as_ref().and_then(|s| s.get()) {
                Some(Ok(c)) => Ok(c.clone()),
                Some(Err(msg)) => Err(anyhow!("{msg}")),
                // `running`이면 슬롯이 반드시 있고, 완료 시 반드시 채워진다(패닉이어도).
                None => Err(anyhow!("크레덴셜 취득 결과를 받지 못했습니다")),
            };
        }

        // 내가 취득자다. 무효화는 여기서 — 게이트 밖이면 위 주석의 창이 열린다.
        if invalidate {
            self.set_cached_creds(None);
        }
        // 이번 비행 전용 슬롯을 걸어 둔다. **옛 비행의 결과는 여기서 시야에서 사라진다** —
        // 이제 막 편승하는 쪽은 이 빈 슬롯을 집으므로 지난 결과를 물려받을 수 없고,
        // 이미 편승한 쪽은 자기 슬롯을 들고 있어 이 교체에 영향받지 않는다.
        let slot: FlightSlot = std::sync::Arc::new(std::sync::OnceLock::new());
        st.slot = Some(slot.clone());
        st.running = true;
        drop(st); // 비싼 취득은 **락 밖에서** — 편승자들이 진입해 기다릴 수 있어야 한다.

        // 이 지점 이후로는 어떤 경로로 빠져나가든(패닉 포함) 게이트가 풀린다.
        let mut flight = FlightGuard::new(self, slot);
        let result = (self.acquirer)();

        // 성공이면 캐시를 먼저 채우고, 그 다음(`FlightGuard::drop`) 편승자를 깨운다.
        if let Ok(c) = result.as_ref() {
            self.set_cached_creds(Some(c.clone()));
        }
        flight.finish(&result);

        result
    }

    /// 캐시된 크레덴셜을 버리고 브라우저에서 다시 읽는다. 성공하면 true.
    /// 401을 받은 직후 `retry_once_on_401`이 호출당 최대 한 번 부른다.
    ///
    /// 실패해도 캐시는 비운 채로 둔다 — `creds()`가 다음 도구 호출에서 다시 시도하므로,
    /// 사용자가 그 사이 로그인하면 재시작 없이 복구된다.
    /// 주의: `INNO_CREED_AUTH_TOKEN`/`INNO_CREED_SIGN_KEY`를 쓰는 환경은 재취득해도 **같은 값**이
    /// 돌아온다 — 그래서 재시도 상한(1회)이 정책의 핵심이다.
    ///
    /// 동시에 여러 요청이 401을 만나도 실제 취득은 `acquire_creds`의 단일 비행이 1회로 묶는다.
    /// **재시도 상한과는 무관하다** — 이건 "취득 횟수"를 줄이는 것이고, 요청당 재시도는 여전히 1회다.
    fn reacquire_creds(&self) -> bool {
        // 낡은 값으로 서명하는 요청을 줄이려고 캐시를 비우되, **비우는 것도 게이트 안에서** 한다
        // (`acquire_creds_inner`의 주석 — 밖에서 비우면 취득자가 방금 채운 값을 지우는 창이 생긴다).
        self.acquire_creds_inner(true).is_ok()
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

    // ── 크레덴셜 취득 단일 비행 ────────────────────────────────────────────────
    //
    // 취득 동작을 `GwClient::with_acquirer`로 주입해 **실제 호출 횟수**를 센다. 브라우저도
    // 네트워크도 쓰지 않는다.
    //
    // ⚠️ tokio가 아니라 `std::thread`를 쓴다. `acquire_creds`는 sync 함수라 편승자가 **스레드를
    // 블로킹**하는데, tokio 멀티스레드 런타임에서 태스크 수가 워커 수를 넘으면 뒤늦은 태스크가
    // 아예 스케줄되지 않아 취득자가 영원히 기다린다(테스트가 굳는다). std 스레드는 그 상한이 없어
    // 진짜 동시성을 만든다 — 목적("정말 병렬로 부딪히게 한다")에는 이쪽이 더 강하다.

    /// 편승 대기 상한(테스트 공통). 기본 30초를 쓰면 깨우기가 깨졌을 때 **테스트가 통과하면서도**
    /// 스위트가 30초씩 늘어져 회귀가 "느려짐"으로만 드러난다. 3초면 통지 회귀가 곧바로 눈에 띄고,
    /// 정상 경로는 밀리초라 여유가 넘친다.
    const TEST_WAIT_CAP: Duration = Duration::from_secs(3);

    fn creds_of(tag: &str) -> Creds {
        Creds { auth_token: format!("AT-{tag}"), sign_key: format!("HK-{tag}") }
    }

    /// 취득 대역: 호출 횟수를 세고, 첫 취득이 시작됐음을 알린 뒤 `hold` 동안 붙잡아 둔다
    /// (그 사이 다른 스레드가 게이트에 도착해 편승한다).
    struct Acquirer {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        started: std::sync::Arc<std::sync::atomic::AtomicBool>,
        /// `make_gated`가 쓰는 해제 신호. 테스트가 놓아줄 때까지 취득자를 붙잡는다.
        released: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl Acquirer {
        fn new() -> Self {
            Self {
                calls: Default::default(),
                started: Default::default(),
                released: Default::default(),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
        /// 취득 결과를 고정으로 주는 클로저. `hold` 동안 취득자를 붙잡는다.
        fn make(&self, out: Result<Creds, &'static str>, hold: Duration)
            -> Box<dyn Fn() -> Result<Creds> + Send + Sync>
        {
            let (calls, started) = (self.calls.clone(), self.started.clone());
            Box::new(move || {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                started.store(true, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(hold);
                match out {
                    Ok(ref c) => Ok(c.clone()),
                    Err(m) => Err(anyhow!("{m}")),
                }
            })
        }

        /// `make`의 **시간 가정 없는** 판. `release()`를 부를 때까지 취득자가 붙잡혀 있으므로
        /// "편승자들이 도착하기 전에 취득이 끝나버렸다"는 실패 모드가 원천적으로 없다.
        fn make_gated(&self, out: Result<Creds, &'static str>)
            -> Box<dyn Fn() -> Result<Creds> + Send + Sync>
        {
            let (calls, started, released) =
                (self.calls.clone(), self.started.clone(), self.released.clone());
            Box::new(move || {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                started.store(true, std::sync::atomic::Ordering::SeqCst);
                let deadline = Instant::now() + Duration::from_secs(10);
                while !released.load(std::sync::atomic::Ordering::SeqCst) {
                    assert!(Instant::now() < deadline, "해제 신호가 오지 않았다");
                    std::thread::sleep(Duration::from_millis(1));
                }
                match out {
                    Ok(ref c) => Ok(c.clone()),
                    Err(m) => Err(anyhow!("{m}")),
                }
            })
        }

        /// 붙잡아 둔 취득자를 놓아준다(`make`로 만든 대역에는 아무 영향 없음).
        fn release(&self) {
            self.released.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        /// 첫 취득이 실제로 시작될 때까지 기다린다(그 뒤에 붙는 스레드는 반드시 편승자가 된다).
        fn wait_started(&self) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while !self.started.load(std::sync::atomic::Ordering::SeqCst) {
                assert!(Instant::now() < deadline, "취득이 시작되지 않았다");
                std::thread::sleep(Duration::from_millis(1));
            }
        }
    }

    /// 취득자 1 + 편승자 N-1 을 만들어 `acquire_creds`를 동시에 때린다.
    fn hammer(
        client: std::sync::Arc<GwClient>,
        spy: &Acquirer,
        n: usize,
        call: impl FnMut(usize, &GwClient) -> Result<Creds> + Send + Clone + 'static,
    ) -> Vec<Result<Creds>> {
        let first = {
            let (c, mut call) = (client.clone(), call.clone());
            std::thread::spawn(move || call(0, &c))
        };
        spy.wait_started(); // 이 시점 이후 도착하는 스레드는 전부 편승자다
        let rest: Vec<_> = (1..n)
            .map(|i| {
                let (c, mut call) = (client.clone(), call.clone());
                std::thread::spawn(move || call(i, &c))
            })
            .collect();
        // 편승자 n-1이 **게이트에 들어온 것을 확인한 뒤** 취득자를 놓아준다. sleep 기반 가정
        // (=취득자의 hold 안에 전부 도착한다)에 기대지 않으므로 부하가 높아도 흔들리지 않는다.
        client.wait_for_waiters(n - 1, Duration::from_secs(10));
        spy.release();
        let mut out = vec![first.join().unwrap()];
        out.extend(rest.into_iter().map(|h| h.join().unwrap()));
        out
    }

    /// 핵심 단언 — 동시 요청 N개가 취득을 요구해도 **실제 취득은 정확히 1회**이고
    /// N개 전부 같은 값을 받는다. (`>= 1` 같은 느슨한 단언은 단일 비행이 깨져도 통과한다.)
    #[test]
    fn 동시_취득요구가_여러건이어도_실제_취득은_한_번이다() {
        const N: usize = 8;
        let spy = Acquirer::new();
        let client = std::sync::Arc::new(
            GwClient::with_acquirer(None, spy.make_gated(Ok(creds_of("fresh"))))
                .with_wait_cap(TEST_WAIT_CAP),
        );

        let got = hammer(client.clone(), &spy, N, |_, c| c.acquire_creds());

        assert_eq!(spy.calls(), 1, "취득이 {}번 일어났다 — 단일 비행이 깨졌다", spy.calls());
        assert_eq!(got.len(), N);
        for r in &got {
            assert_eq!(r.as_ref().unwrap().auth_token, "AT-fresh", "편승자가 다른 값을 받았다");
        }
        // 취득자가 캐시까지 채운다(편승자는 캐시를 건드리지 않는다).
        assert_eq!(client.cached_creds().unwrap().sign_key, "HK-fresh");
    }

    /// 실패도 공유한다 — 한쪽만 성공하고 다른 쪽은 실패하는 갈림이 없어야 한다.
    #[test]
    fn 취득이_실패하면_동시_요구_전부가_실패를_받는다() {
        const N: usize = 6;
        let spy = Acquirer::new();
        let client = std::sync::Arc::new(
            GwClient::with_acquirer(None, spy.make_gated(Err("브라우저 미로그인")))
                .with_wait_cap(TEST_WAIT_CAP),
        );

        let got = hammer(client.clone(), &spy, N, |_, c| c.acquire_creds());

        assert_eq!(spy.calls(), 1);
        assert_eq!(got.iter().filter(|r| r.is_err()).count(), N, "일부만 실패했다");
        for r in &got {
            // `Creds`에는 일부러 `Debug`가 없다(토큰이 로그·패닉 메시지로 새지 않게).
            // 그래서 `unwrap_err()` 대신 패턴으로 꺼낸다.
            let Err(e) = r else { panic!("실패해야 할 요구가 성공했다") };
            let msg = format!("{e:#}");
            assert!(msg.contains("브라우저 미로그인"), "원인 문구가 유실됐다: {msg}");
        }
        assert!(client.cached_creds().is_none(), "실패는 캐시하지 않는다");
    }

    /// `creds()`와 `reacquire_creds()`가 **같은 게이트**를 지나는지.
    /// 한쪽만 막으면 재취득이 캐시를 비운 창으로 `creds()`가 새어 들어가 각자 취득한다.
    #[test]
    fn creds와_reacquire가_같은_게이트를_공유한다() {
        const N: usize = 8;
        let spy = Acquirer::new();
        let client = std::sync::Arc::new(
            GwClient::with_acquirer(None, spy.make_gated(Ok(creds_of("shared"))))
                .with_wait_cap(TEST_WAIT_CAP),
        );

        // 짝수는 재취득 경로, 홀수는 캐시 미스 경로로 동시에 진입시킨다.
        let got = hammer(client.clone(), &spy, N, |i, c| {
            if i % 2 == 0 {
                c.reacquire_creds().then(|| creds_of("shared")).ok_or_else(|| anyhow!("실패"))
            } else {
                c.creds()
            }
        });

        assert_eq!(spy.calls(), 1, "두 경로가 각자 취득했다 — 게이트가 공유되지 않았다");
        assert!(got.iter().all(|r| r.is_ok()));
    }

    /// 편승자는 **통지로** 깨야 한다 — 상한 만료로 풀리면 기능은 같아 보여도 동시 401 N건 중
    /// N-1건이 각각 상한만큼(기본 30초) 얼어붙는다. `notify_one`이면 정확히 그렇게 된다.
    #[test]
    fn 편승자는_상한이_아니라_통지로_깬다() {
        const N: usize = 8;
        let spy = Acquirer::new();
        let client = std::sync::Arc::new(
            GwClient::with_acquirer(None, spy.make_gated(Ok(creds_of("fresh"))))
                .with_wait_cap(TEST_WAIT_CAP),
        );

        let t0 = Instant::now();
        let got = hammer(client.clone(), &spy, N, |_, c| c.acquire_creds());
        let elapsed = t0.elapsed();

        for r in &got {
            let Ok(c) = r else {
                panic!("편승자가 상한 만료로 실패했다 — 통지가 일부에게만 갔다");
            };
            assert_eq!(c.auth_token, "AT-fresh");
        }
        assert!(
            elapsed < Duration::from_millis(1500),
            "편승자가 통지가 아니라 상한(3초)까지 기다렸다: {elapsed:?}"
        );
    }

    /// 편승 대기에는 **상한이 실제로 걸린다.** 취득이 끝나지 않아도 기다리는 쪽은 포기한다.
    #[test]
    fn 편승_대기는_상한에서_포기한다() {
        let spy = Acquirer::new();
        let client = std::sync::Arc::new(
            // 취득자는 600ms 붙잡혀 있고 상한은 100ms — 편승자는 반드시 만료를 본다.
            GwClient::with_acquirer(
                None,
                spy.make(Ok(creds_of("slow")), Duration::from_millis(600)),
            )
            .with_wait_cap(Duration::from_millis(100)),
        );

        let acq = {
            let c = client.clone();
            std::thread::spawn(move || c.acquire_creds())
        };
        spy.wait_started();

        let t0 = Instant::now();
        let r = client.acquire_creds();
        let waited = t0.elapsed();

        let Err(e) = r else { panic!("상한이 걸리지 않았다") };
        assert!(format!("{e:#}").contains("대기 시간 초과"), "{e:#}");
        assert!(waited < Duration::from_secs(1), "상한보다 오래 기다렸다: {waited:?}");
        assert!(acq.join().unwrap().is_ok(), "취득자 자신은 상한과 무관하다");
    }

    /// 기본 상한이 "사람이 기다릴 수 있는" 범위인지. 상한을 필드로 내리면서 기본값이 조용히
    /// 커지면(예: 시간 단위) 상한이 있으나 마나가 되므로 값 자체를 못박는다.
    #[test]
    fn 기본_대기상한은_사람이_기다릴_범위다() {
        assert!(
            ACQUIRE_WAIT_CAP >= Duration::from_secs(5),
            "키체인 프롬프트를 기다리는 경로가 있어 몇 초로는 짧다"
        );
        assert!(
            ACQUIRE_WAIT_CAP <= Duration::from_secs(60),
            "이보다 길면 도구 호출이 사실상 멈춘 것으로 보인다"
        );
        assert_eq!(
            GwClient::new(None).wait_cap,
            ACQUIRE_WAIT_CAP,
            "프로덕션 생성자가 기본 상한을 쓰지 않는다"
        );
    }

    /// **캐시를 쓰는 것은 취득자뿐이다.** 편승자도 쓰게 하면 늦게 깬 쪽이 그 사이 갱신된 새 값을
    /// 자기가 받은 옛 값으로 덮는다. 값이 같아 눈에 안 보이는 결함이라 쓰기 횟수로 못박는다.
    #[test]
    fn 캐시_쓰기는_취득자만_한다() {
        const N: usize = 8;
        let spy = Acquirer::new();
        let client = std::sync::Arc::new(
            GwClient::with_acquirer(None, spy.make_gated(Ok(creds_of("fresh"))))
                .with_wait_cap(TEST_WAIT_CAP),
        );

        let got = hammer(client.clone(), &spy, N, |_, c| c.acquire_creds());

        assert!(got.iter().all(|r| r.is_ok()));
        assert_eq!(spy.calls(), 1);
        assert_eq!(
            client.creds_writes(),
            1,
            "취득 1회에 캐시 쓰기가 {}번 — 편승자까지 캐시를 썼다",
            client.creds_writes()
        );
    }

    /// 성질 ② — **새 비행의 편승자가 옛 비행의 결과를 집지 않는다.**
    ///
    /// 폭풍 테스트로는 이걸 못 본다(모든 비행이 같은 값을 내므로 옛 값을 받아도 똑같아 보인다).
    /// 라운드마다 **다른 값**을 내게 해서 결과가 자기 비행의 것인지 못박는다. 슬롯을 비행마다
    /// 새로 만들지 않고 재사용하면(=`OnceLock`이 이미 차 있어 두 번째 `set`이 조용히 무시되면)
    /// 2라운드 편승자가 1라운드 값을 받는다.
    #[test]
    fn 새_비행의_편승자는_옛_비행의_결과를_받지_않는다() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let calls = std::sync::Arc::new(AtomicUsize::new(0));
        let release = std::sync::Arc::new(AtomicUsize::new(0)); // 여기 적힌 라운드까지 놓아준다
        let (c1, r1) = (calls.clone(), release.clone());
        let client = std::sync::Arc::new(
            GwClient::with_acquirer(
                None,
                Box::new(move || {
                    let me = c1.fetch_add(1, SeqCst) + 1; // 1-base 라운드 번호
                    let deadline = Instant::now() + Duration::from_secs(10);
                    while r1.load(SeqCst) < me {
                        assert!(Instant::now() < deadline, "라운드 {me} 해제 신호가 오지 않았다");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Ok(creds_of(&format!("gen{me}")))
                }),
            )
            .with_wait_cap(TEST_WAIT_CAP),
        );

        for round in 1..=2usize {
            let acq = {
                let c = client.clone();
                std::thread::spawn(move || c.acquire_creds())
            };
            while calls.load(SeqCst) < round {
                std::thread::sleep(Duration::from_millis(1));
            }
            let joiner = {
                let c = client.clone();
                std::thread::spawn(move || c.acquire_creds())
            };
            client.wait_for_waiters(1, Duration::from_secs(5)); // 편승이 성립한 뒤에 놓아준다
            release.store(round, SeqCst);

            let want = format!("AT-gen{round}");
            let Ok(a) = acq.join().unwrap() else { panic!("취득자가 실패했다") };
            let Ok(b) = joiner.join().unwrap() else { panic!("편승자가 실패했다") };
            assert_eq!(a.auth_token, want, "취득자가 제 값을 못 받았다");
            assert_eq!(
                b.auth_token, want,
                "라운드 {round} 편승자가 옛 비행의 결과를 받았다"
            );
        }
        assert_eq!(calls.load(SeqCst), 2, "라운드마다 새 비행이어야 한다");
    }

    /// ⭐ **연속 401 폭풍** — 비행이 **여러 라운드** 도는 상황. 단발 비행만 도는 다른 테스트로는
    /// 보이지 않는 결함이 여기서만 드러난다: 편승자가 깬 뒤 결과를 **공유 자리에서** 다시 읽으면,
    /// 그 사이 시작된 다음 비행이 그 자리를 갈아버려 **성공한 비행에 편승하고도 실패를 받는다.**
    /// (사양이 지정한 수동 재현법 `INNO_CREED_AUTH_TOKEN=broken`이 정확히 이 상태 — 매 요청이
    /// 401이라 재취득이 연달아 일어난다. 이 기능이 겨냥한 바로 그 상황이다.)
    ///
    /// 단언은 **요구 == 성공**이다. 한 건이라도 잃으면 실패다.
    #[test]
    fn 연속_재취득_폭풍에서_편승자는_자기_비행의_결과를_받는다() {
        const THREADS: usize = 6;
        const ROUNDS: usize = 400;
        let spy = Acquirer::new();
        let client = std::sync::Arc::new(
            // hold=0 — 취득이 짧을수록 "깨어난 편승자 vs 다음 비행"의 경합이 잦아 결함이 잘 드러난다.
            GwClient::with_acquirer(None, spy.make(Ok(creds_of("storm")), Duration::ZERO))
                .with_wait_cap(TEST_WAIT_CAP),
        );

        let hs: Vec<_> = (0..THREADS)
            .map(|_| {
                let c = client.clone();
                std::thread::spawn(move || {
                    let mut ok = 0usize;
                    let mut first_err = None;
                    for _ in 0..ROUNDS {
                        match c.acquire_creds() {
                            Ok(cr) => {
                                assert_eq!(cr.auth_token, "AT-storm", "다른 비행의 값을 받았다");
                                ok += 1;
                            }
                            Err(e) if first_err.is_none() => first_err = Some(format!("{e:#}")),
                            Err(_) => {}
                        }
                    }
                    (ok, first_err)
                })
            })
            .collect();

        let mut ok = 0usize;
        let mut sample = None;
        for h in hs {
            let (n, e) = h.join().unwrap();
            ok += n;
            sample = sample.or(e);
        }
        assert_eq!(
            ok,
            THREADS * ROUNDS,
            "요구 {}건 중 {}건만 성공했다 — 편승자가 자기 비행의 결과를 잃었다. 첫 실패: {:?}",
            THREADS * ROUNDS,
            ok,
            sample
        );
        // 폭풍인데 취득이 요구 수만큼 일어났다면 단일 비행이 아예 동작하지 않은 것이다.
        assert!(
            spy.calls() < THREADS * ROUNDS,
            "편승이 한 번도 일어나지 않았다 — 이 테스트가 겨냥한 상황이 만들어지지 않았다"
        );
    }

    /// 재취득이 **비행을 시작할 때는** 옛 값을 버린다. 남겨두면 그 사이 들어온 다른 요청이
    /// 낡은 토큰으로 계속 서명해 401을 다시 만든다(게이트에 합류시키는 것이 목적이다).
    #[test]
    fn 재취득은_비행_시작과_함께_옛_값을_버린다() {
        let spy = Acquirer::new();
        let client = std::sync::Arc::new(GwClient::with_acquirer(
            Some(creds_of("old")),
            spy.make_gated(Ok(creds_of("new"))),
        ));

        let t = {
            let c = client.clone();
            std::thread::spawn(move || c.reacquire_creds())
        };
        spy.wait_started(); // 취득이 시작됐다 = 무효화 지점을 이미 지났다
        assert!(client.cached_creds().is_none(), "재취득이 옛 값을 버리지 않았다");

        spy.release();
        assert!(t.join().unwrap());
        assert_eq!(client.cached_creds().unwrap().auth_token, "AT-new");
    }

    /// 진행 중인 비행에 편승하는 **재취득은 캐시를 비우지 않는다.**
    /// 게이트 밖에서 비우면, 취득자가 새 값을 넣고 `running`을 내리기 전 창에 끼어들어 그 값을
    /// 지우고 편승할 수 있다 — 편승자는 캐시를 쓰지 않으니 캐시가 빈 채 남아 중복 취득이 된다.
    #[test]
    fn 편승하는_재취득은_캐시를_비우지_않는다() {
        let spy = Acquirer::new();
        let client = std::sync::Arc::new(GwClient::with_acquirer(
            Some(creds_of("old")),
            spy.make(Ok(creds_of("fresh")), Duration::from_millis(200)),
        ));

        let acq = {
            let c = client.clone();
            std::thread::spawn(move || c.acquire_creds())
        };
        spy.wait_started(); // 이 뒤의 재취득은 반드시 편승자다
        assert!(client.reacquire_creds());
        assert!(acq.join().unwrap().is_ok());

        assert_eq!(spy.calls(), 1);
        // 쓰기는 취득자의 성공 기록 1회뿐 — 편승한 재취득이 무효화(None 쓰기)를 했다면 2가 된다.
        assert_eq!(
            client.creds_writes(),
            1,
            "편승한 재취득이 캐시를 건드렸다 — 무효화가 게이트 밖에 있다"
        );
        assert_eq!(client.cached_creds().unwrap().auth_token, "AT-fresh");
    }

    /// **취득이 패닉해도 게이트는 풀린다.** 락 밖에서 취득하므로 패닉해도 뮤텍스는 poison되지
    /// 않는다 — `running`을 내릴 주체가 `Drop`뿐이다. 이게 없으면 이후 모든 취득이 편승자가 되어
    /// 상한까지 기다렸다 실패하고, 프로세스를 재시작해야 복구된다.
    #[test]
    fn 취득이_패닉해도_게이트가_잠기지_않는다() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (c, s) = (calls.clone(), started.clone());
        let client = std::sync::Arc::new(
            GwClient::with_acquirer(
                None,
                Box::new(move || {
                    if c.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        s.store(true, std::sync::atomic::Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(150)); // 편승자가 붙을 시간
                        panic!("취득 도중 패닉");
                    }
                    Ok(creds_of("after-panic"))
                }),
            )
            .with_wait_cap(TEST_WAIT_CAP),
        );

        // 패닉 백트레이스로 테스트 출력을 더럽히지 않는다(패닉 자체는 그대로 일어난다).
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));

        let acq = {
            let c = client.clone();
            std::thread::spawn(move || c.acquire_creds())
        };
        while !started.load(std::sync::atomic::Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(1));
        }
        // ① 편승자는 빈손으로 매달리지 않고 사유를 받는다.
        let t0 = Instant::now();
        let joined = client.acquire_creds();
        let waited = t0.elapsed();
        assert!(acq.join().is_err(), "취득자 스레드가 패닉하지 않았다 — 전제가 깨졌다");
        std::panic::set_hook(hook);

        let Err(e) = joined else { panic!("패닉한 비행이 성공을 돌려줬다") };
        assert!(format!("{e:#}").contains("중단"), "패닉 사유가 아니다: {e:#}");
        assert!(waited < Duration::from_secs(1), "편승자가 상한까지 매달렸다: {waited:?}");

        // ② 그 다음 취득은 새 비행으로 정상 성공한다(게이트가 잠기지 않았다).
        let t1 = Instant::now();
        let got = client.acquire_creds();
        assert_eq!(got.unwrap().auth_token, "AT-after-panic", "패닉 뒤 게이트가 잠겼다");
        assert!(t1.elapsed() < Duration::from_secs(1), "상한 만료로 풀렸다: {:?}", t1.elapsed());
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    /// 캐시가 차 있으면 취득하지 않는다(게이트가 정상 경로를 느리게 만들지 않는지).
    #[test]
    fn 캐시가_있으면_취득하지_않는다() {
        let spy = Acquirer::new();
        let client = GwClient::with_acquirer(
            Some(creds_of("seed")),
            spy.make(Ok(creds_of("fresh")), Duration::ZERO),
        );
        for _ in 0..5 {
            assert_eq!(client.creds().unwrap().auth_token, "AT-seed");
        }
        assert_eq!(spy.calls(), 0);
    }

    /// **과잉 병합 방지** — 시간차를 둔 재취득은 각각 새 비행이어야 한다.
    /// (편승은 "지금 돌고 있는 비행"에만 한다. 지난 결과를 재사용하면 만료된 토큰으로 재시도하게 된다.)
    #[test]
    fn 연달아_일어난_재취득은_각각_새로_취득한다() {
        let spy = Acquirer::new();
        let client = GwClient::with_acquirer(
            None,
            spy.make(Ok(creds_of("x")), Duration::ZERO),
        );
        assert!(client.reacquire_creds());
        assert!(client.reacquire_creds());
        assert_eq!(spy.calls(), 2, "동시가 아닌 재취득까지 합쳐버리면 만료 복구가 죽는다");
    }

    /// 캐시 락이 poison돼도 값을 잃지 않는다(예전 `if let Ok(..)`은 조용히 건너뛰었다).
    #[test]
    fn 캐시_락이_poison되어도_읽고_쓴다() {
        let client = std::sync::Arc::new(GwClient::new(Some(creds_of("before"))));
        let c2 = client.clone();
        let _ = std::thread::spawn(move || {
            let _g = c2.creds.write().unwrap();
            panic!("락을 쥔 채 패닉 — poison 유발");
        })
        .join();
        assert!(client.creds.is_poisoned());
        assert_eq!(client.cached_creds().unwrap().auth_token, "AT-before");
        client.set_cached_creds(Some(creds_of("after")));
        assert_eq!(client.cached_creds().unwrap().auth_token, "AT-after");
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
