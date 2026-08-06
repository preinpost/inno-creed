# 아키텍처 · 인증 · 공통 규약

> 실증(브라우저 밖 순수 HTTP)으로 확정된 사실만 정리. gw.innogrid.com = 더존 WEHAGO/BIZCUBE 그룹웨어(React CSR SPA, 해시 라우팅).

## 1. 왜 브라우저가 필요 없는가 (확정)

서버는 요청을 **HTTP 헤더 4종(+Content-Type)** 만으로 인증한다. 실증 결과:

- `Origin` / `Referer` **검사 안 함** — 없거나 임의 값이어도 200.
- 쿠키 **필수 아님** — 헤더로 인증 정보를 다 넘기면 쿠키 없이 200.
- 즉 Python/Rust에서 헤더만 맞춰 POST하면 브라우저 없이 그룹웨어 조작이 가능하다. (playwright는 **크레덴셜 최초 취득/신규 API 캡처**에만 쓰고, 운영에는 불필요.)

## 2. 아키텍처

```
inno-creed (Rust MCP 서버, 헤드리스)
 ├─ creds    크레덴셜 취득: Chrome 쿠키 복호화 → authToken / signKey
 ├─ sign     wehago-sign(HMAC-SHA256) · transaction-id 생성
 ├─ util     도메인 무관 순수 함수(날짜 days_to_ymd/fmt_ymd · digits_only · JSON 필드 추출 json_str/s)
 ├─ client   GwClient: ensure_session(gw050A02 lazy 취득+10분 TTL 캐시) · 캘린더 목록 캐시(10분 TTL) · 사원 명부 캐시(30분 TTL) · 본인 표시정보 캐시(30분 TTL) · signed()로 헤더 4종 주입 · 전송 · 응답봉투 파싱 · companyInfo 조립
 ├─ error    도메인 공통 에러 타입 — NotOwner(소유권 위반) · InvalidInput(호출자 인자 오류)
 ├─ modules  resource(자원) · calendar(일정) · mail(메일) · board(게시판) · approval*(전자결재)
 │           org(조직) · attendance(근태) · search(통합검색) · submission_guide
 │           API 래퍼 + 파생 조회 + **소유권 가드 · read-back 검증**(`*_and_verify`)
 └─ mcp/     rmcp 서버(stdio)
    ├─ mod.rs   서버 골격: Amaranth · 라우터 합성(all_tools) · ensure_session · 에러 변환 · instructions
    ├─ tools/   도구 46개 — 도메인 11개(resource·calendar·mail·board·approval{,_line,_submit,_meta}·org·attendance·search)
    └─ args/    도구 인자 스키마 — 도메인 8개. ⚠️ doc comment가 그대로 LLM 프롬프트가 된다
```

- 구조 근거: MCP는 **실행층**, 크레덴셜만 외부(브라우저)에서 취득. 그래서 헤드리스로 돌아간다.
- 서버 시작 순서: `creds::from_browser()`(크레덴셜 — Chrome → Firefox 폴백) → stdio MCP 서브. [세션 정보](#4-authtoken-구조--세션-정보-lazy-취득--ttl-캐시)는 첫 도구 호출 시 `ensure_session()`이 lazy 취득(선취득 없음).
- 소스: `src/{creds,sign,util,client,error}.rs`, `src/modules/*.rs`, `src/mcp/{mod.rs,tools/,args/}`. 빌드 타깃은 `src/main.rs`(MCP 서버)와 `src/bin/probe.rs`(디버그 REPL — 임의 엔드포인트를 서명 호출) 둘.
- **도구 라우터 합성**: 도메인마다 `#[tool_router(router = <도메인>_router, vis = "pub(crate)")]`로 라우터를 만들고 `Amaranth::all_tools()`가 `ToolRouter`의 `Add`로 합친다. `#[tool_handler(router = Self::all_tools())]`로 경로를 명시한다 — 핸들러는 라우터를 **필드로 갖지 않는다**(매크로가 호출 때마다 표현식을 평가하므로 필드에 담아도 읽히지 않는다).
- **모듈 함수 시그니처 규약**: 첫 인자는 `c: &GwClient`. 예외는 `org::roster`/`org::find_person` 둘뿐이며 `&Arc<GwClient>`를 받는다 — 부서를 `JoinSet`으로 병렬 순회하는데 `spawn`이 `'static`을 요구하고 `GwClient`는 `RwLock` 보유로 `Clone`이 아니기 때문이다. 대안(신규 의존성/역할 분담 붕괴/직렬화)이 전부 대가가 커서 **의도적으로 예외를 유지**한다. 새 함수는 `&GwClient`를 쓸 것(상세: `src/modules/org.rs` 헤더 주석).
- **파생 조회**: 일부 도구는 단일 API 래퍼가 아니라 여러 호출을 조합해 서버측에서 계산을 끝낸다 — `find_free_rooms`(자원 목록+예약을 인터벌 연산), `find_person`(부서 전수 순회 후 캐시), `my_reservations`·`pending_approvals`(필터+요약). LLM이 매 호출마다 같은 다단 조합을 반복하지 않게 하려는 것.

## 3. 크레덴셜 취득 (Chrome / Firefox · macOS·Linux·Windows)

Chrome 또는 Firefox가 `gw.innogrid.com`에 저장한 쿠키에서 두 값을 뽑는다(`src/creds.rs`):

| 쿠키 | 용도 | 후처리 |
|---|---|---|
| `BIZCUBE_AT` | `authToken` | URL 디코드(`%7C`→`|`) |
| `BIZCUBE_HK` | `signKey` | 그대로 |

**Chrome** — 쿠키 DB(SQLite, `WHERE host_key='gw.innogrid.com'`)의 `encrypted_value`를 OS별로 복호화:

| OS | 복호화 키 | 알고리즘 |
|---|---|---|
| macOS | 키체인 `security find-generic-password -s "Chrome Safe Storage"` → PBKDF2-HMAC-SHA1(1003, 16B) | AES-128-CBC(iv=0x20×16, Pkcs7) |
| Linux | 키링(`v11`): `secret-tool`로 `Chrome Safe Storage` 비밀 조회 → PBKDF2-HMAC-SHA1(1, 16B). 키링 미사용(`v10`): 고정 비번 `"peanuts"` | AES-128-CBC(iv=0x20×16, Pkcs7) |
| Windows | `v10`: `os_crypt.encrypted_key`(base64, `DPAPI` 접두) → `CryptUnprotectData`로 32B 키. `v20`(app-bound): `os_crypt.app_bound_encrypted_key`(`APPB` 접두) → Chrome Elevator COM `IElevator::DecryptData` → 끝 32B 키 | AES-256-GCM(nonce 12B + tag 16B) |

- 공통: `encrypted_value` 앞 **3바이트 버전 프리픽스(`v10`/`v20`) 제거**(Windows는 접두로 `v10`↔`v20` 키 선택). 최신 Chrome은 평문 앞에 **32B 도메인 SHA256**을 붙이므로 UTF-8 파싱 실패 시 앞 32B 제거.
- 쿠키 DB 경로: 신버전 `Default/Network/Cookies` → 구버전 `Default/Cookies` 폴백. User Data 루트는 OS별(mac `~/Library/…`, linux `~/.config/google-chrome`, win `%LOCALAPPDATA%\Google\Chrome\User Data`).

**Firefox** — `cookies.sqlite`(`moz_cookies`)가 **평문**이라 복호화 없이 읽는다. 프로필 루트만 OS별(mac `~/Library/…/Firefox/Profiles`, linux `~/.mozilla/firefox`, win `%APPDATA%\Mozilla\Firefox\Profiles`)로 분기, `*.default*` 프로필 우선. Chrome 실패 시 폴백.

- **취약 경로**: Windows 최신 Chrome은 실행 중 쿠키 파일을 **배타적으로 잠가** 복사 불가(Chrome 종료 필요). `v20` app-bound는 Elevator COM으로 시도하나 Chrome이 호출자를 거부할 수 있음(best-effort). 실패 시 Firefox 폴백.
- **수동 우회**: `INNO_CREED_AUTH_TOKEN`(=`BIZCUBE_AT`) + `INNO_CREED_SIGN_KEY`(=`BIZCUBE_HK`) 환경변수를 모두 지정하면 브라우저 읽기를 건너뛰고 그 값을 사용(모든 경로보다 우선). 모든 OS·브라우저 우회.
- **만료**: 401 감지 시 쿠키 재복호화로 재취득(만료 주기 미관측 — 열린 질문).
- 임시 파일(복사한 쿠키 DB)은 사용 후 삭제.

## 4. authToken 구조 & 세션 정보 (lazy 취득 + TTL 캐시)

```
authToken = "{groupSeq}|{empSeq}|{secret}"
          = "gcms<테넌트>|<본인 empSeq>|..."
```

- `split('|')`: `[0]`=groupSeq, `[1]`=empSeq(UC 본인 식별, 소유권 가드 기준).
- **나머지 세션 정보는 하드코딩하지 않고 `gw050A02`(SSO 세션정보 조회)로 취득** — 배포용(사용자마다 값이 다름). agent용 tool이 아니라 **값이 필요할 때 내부적으로 lazy 호출**하고 **인메모리 10분 TTL로 캐시**한다(`ensure_session()`).

  | 값 | 출처 |
  |---|---|
  | groupSeq, empSeq | authToken split |
  | compSeq, deptSeq, empName, emailAddr, emailDomain | `gw050A02` → `resultData.sessionInfo.ucUserInfo` (UC 계열) |
  | empCd, deptCd, coCd | 같은 `ucUserInfo`의 `erpEmpSeq`/`erpDeptSeq`/`erpCompSeq` (근태/ERP 계열 — UC seq와 별개 코드 체계) |

  - **`gw050A02` 호출**: `POST /gw/gw050A02`, `Content-Type: x-www-form-urlencoded`, body `a10Domain=https://gw.innogrid.com`. Bearer 인증 헤더만으로 "이미 로그인된 사용자"의 sessionInfo 반환(별도 CSRF 토큰 불필요). 브라우저는 SSO 진입 시 이 응답을 `sessionStorage.userInfo`에 캐시한다 — MCP는 sessionStorage 대신 동일 API를 직접 호출.
  - **lazy + TTL**: 첫 도구 호출 시 취득 → 10분간 캐시 재사용 → 만료 시 재조회. 시작 시 선취득하지 않음. 저장은 `RwLock<SessionCache>`(info + `Instant`), fetch 중 락 미보유(await 동안). 이전 방식(`mail000A01` + `sc111A02` 2회, 서버 시작 시 1회)을 대체 — `ucUserInfo` 하나로 UC + 근태 코드를 한 번에 확보.
- `companyInfo` 객체(compSeq/groupSeq/deptSeq/emailAddr/emailDomain)는 이 세션 정보로 조립해 요청 body에 공통 주입.

## 5. 요청 서명 (wehago-sign)

```
wehago-sign = Base64( HMAC_SHA256( authToken ‖ transactionId ‖ timestamp ‖ urlPathname , signKey ) )
```

- 4개 입력을 **구분자 없이** 순서대로 이어 HMAC(키=signKey). `src/sign.rs` 참조.
- `urlPathname` = 요청 경로(`/schres/rs121A06` 등, 쿼리 제외).
- `transaction-id` = 요청마다 새로 뽑는 32 hex(16바이트 랜덤).
- `timestamp` = unix epoch 초.

### 요청 헤더

| 헤더 | 값 |
|---|---|
| `Authorization` | `Bearer {authToken}` |
| `timestamp` | unix epoch 초 |
| `transaction-id` | 32 hex |
| `wehago-sign` | 위 서명 |
| `Content-Type` | `application/json`(대부분) / `multipart/form-data`(메일 발송) |

## 6. 응답 봉투

```json
{ "resultCode": 0, "resultMsg": "SUCCESS", "resultData": { ... } }
```

- 성공 판정: `resultCode ∈ {0, 200}`. (모듈별 혼용 주의)
- 도구는 `resultData`만 반환.

## 7. 필수 안전 규약

### 7.1 응답 성공 ≠ 실제 반영 → read-back 검증

서버는 **권한 밖 대상을 수정 요청받으면 `successTf:true`를 주면서 실제로는 무시(silent no-op)** 한다. 실증: 남이 만든 예약의 "내용"을 수정 요청 → 응답 성공 → **재조회하니 그대로**. 따라서:

> 모든 mutation(등록/수정/삭제)은 직후 **재조회(read-back)로 실제 상태를 확인**하고, 반영이 안 됐으면 실패로 처리한다.

**구현 위치**: 도구 층이 아니라 **각 도메인 모듈의 `*_and_verify` 함수 안**이다(`resource::reserve/update/cancel_and_verify`, `calendar::create/update/delete_event_and_verify`, `attendance::punch_and_verify`). 검증 없는 raw 래퍼도 남아 있으나 새 호출부는 `*_and_verify`를 쓴다 — 규칙이 모듈에 있어야 MCP를 거치지 않는 호출자도 우회할 수 없다.

### 7.2 소유권 가드

- 자원 예약의 쓰기는 서버가 **소유자(생성자) 본인일 때만** 실제 반영한다(IDOR 아님 — 정보 조회는 열려 있으나 쓰기는 막힘).
- 서버에 맡기지 않고, 쓰기 전에 대상의 소유자 == 본인 empSeq(authToken에서 추출)를 확인하고 아니면 **명시적 에러**를 반환한다. (조회는 제한 없음.)
- **소유자 필드는 도메인마다 다르다** — 자원 예약은 `empSeq`("소유자"), 일정은 `createSeq`("작성자"). 그래서 가드 함수는 도메인별로 각자 둔다(`resource::check_owner` / `calendar::check_author`). 다만 **에러는 `error::NotOwner` 타입 하나를 공유**하고, `mcp::map_domain_err`가 `downcast_ref`로 판별해 `invalid_params`로 매핑한다 — 문자열 매칭이 아니라 타입으로 분류하는 것이 핵심이다.
