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
 ├─ client   GwClient: ensure_session(gw050A02 lazy 취득+10분 TTL 캐시) · 캘린더 목록 캐시(10분 TTL) · 헤더 4종 주입 · POST · 응답봉투 파싱 · companyInfo 조립
 ├─ modules  resource(자원) · calendar(일정) · mail(메일) · board(게시판) — API 래퍼
 └─ mcp      rmcp 서버(stdio): tool 정의 · 소유권 가드 · read-back 검증
```

- 구조 근거: MCP는 **실행층**, 크레덴셜만 외부(브라우저)에서 취득. 그래서 헤드리스로 돌아간다.
- 서버 시작 순서: `creds::from_chrome()`(크레덴셜) → stdio MCP 서브. [세션 정보](#4-authtoken-구조--세션-정보-lazy-취득--ttl-캐시)는 첫 도구 호출 시 `ensure_session()`이 lazy 취득(선취득 없음).
- 소스: `src/{creds,sign,client,mcp}.rs`, `src/modules/{resource,calendar,mail}.rs`.

## 3. 크레덴셜 취득 (Chrome, macOS)

Chrome이 `gw.innogrid.com`에 저장한 쿠키에서 두 값을 뽑는다:

| 쿠키 | 용도 | 후처리 |
|---|---|---|
| `BIZCUBE_AT` | `authToken` | URL 디코드(`%7C`→`|`) |
| `BIZCUBE_HK` | `signKey` | 그대로 |

복호화 절차 (Python으로 실증 → Rust 포팅, `src/creds.rs`):

1. macOS 키체인에서 저장소 키: `security find-generic-password -w -s "Chrome Safe Storage" -a "Chrome"`
2. 키 유도: `PBKDF2-HMAC-SHA1(password, salt="saltysalt", iterations=1003, dkLen=16)`
3. `~/Library/.../Chrome/Default/Cookies`(SQLite) 를 temp로 복사 → `WHERE host_key='gw.innogrid.com'` 조회
4. `encrypted_value`에서 **앞 3바이트(`v10` 등 버전 프리픽스) 제거** → `AES-128-CBC(key, iv=0x20×16)` 복호(Pkcs7 언패딩)

- **만료**: 401 감지 시 쿠키 재복호화로 재취득(만료 주기 미관측 — 열린 질문).
- 임시 파일(복사한 Cookies DB 등)은 사용 후 삭제.

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

### 7.2 소유권 가드

- 자원 예약의 쓰기는 서버가 **소유자(생성자) 본인일 때만** 실제 반영한다(IDOR 아님 — 정보 조회는 열려 있으나 쓰기는 막힘).
- MCP는 서버에 맡기지 않고, 쓰기 전에 대상 예약의 `empSeq == 본인 empSeq`(authToken에서 추출) 를 확인하고 아니면 **명시적 에러**를 반환한다. (조회는 제한 없음.)
