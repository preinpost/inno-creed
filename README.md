<p align="center">
  <img src="assets/inno-creed.jpg" width="400" alt="Inno Creed — 이노그리드 아마란스 MCP">
</p>

<h1 align="center">inno-creed</h1>

<p align="center">
  아마란스(<code>gw.innogrid.com</code>, 더존 WEHAGO/BIZCUBE 그룹웨어) 내부 API를 직접 호출하는 <b>Rust MCP 서버</b>
</p>

---

## 무엇인가

그룹웨어를 **브라우저 없이** 다루는 MCP 서버입니다. 회의실 예약, 일정, 메일, 게시판, 전자결재, 근태, 조직도를 Claude 같은 MCP 클라이언트에서 대화로 처리할 수 있습니다.

- **헤드리스** — Playwright 같은 브라우저 자동화 없이 순수 HTTP + HMAC 서명(`wehago-sign`)으로 호출합니다. 서버가 `Origin`/`Referer`를 검사하지 않고 쿠키도 필수가 아니라, 헤더 4종만 맞추면 인증이 통과합니다.
- **로그인 불필요** — 아이디/비밀번호를 요구하지 않습니다. 이미 브라우저에 로그인돼 있으면 그 쿠키를 복호화해 크레덴셜(`authToken` / `signKey`)만 가져옵니다.
- **안전 규약 내장** — 모든 쓰기 작업은 read-back으로 실제 반영을 검증하고, 남의 데이터 수정은 사전에 차단합니다(아래 [안전 규약](#안전-규약)).

## 할 수 있는 것

**통합검색**
| 도구 | 기능 |
|---|---|
| `search` | 메일·전자결재·게시판·일정·자원·파일을 한 번에 검색(기간 필터, 모듈별 범위 지정) |

> 결과에 후속 조회용 ID가 함께 옵니다 — 메일 `muid` → `read_mail`, 결재 `docId`+`formId` → `read_approval`, 게시판 `artSeqNo` → `read_notice`. "지난달 그 메일 찾아서 본문 보여줘" 같은 요청이 한 흐름으로 이어집니다.

**회의실(자원)**
| 도구 | 기능 |
|---|---|
| `find_free_rooms` | **빈 시간 탐색** — 날짜·필요시간·구간·건물을 주면 예약을 뺀 가용 구간만 반환 |
| `list_resources` | 회의실(자원) 목록 |
| `list_reservations` | 기간·자원별 예약 현황(자원 미지정 시 전체). 기본 슬림 응답, 원본은 `verbose:true` |
| `my_reservations` | 본인 예약만 — 수정·취소에 필요한 `seqNum`/`resIdx`를 얻는 경로 |
| `reserve_resource` | 회의실 예약 |
| `update_reservation` | 예약 수정(본인 소유만) |
| `cancel_reservation` | 예약 취소(본인 소유만) |

> `find_free_rooms`는 종일·다일 예약(예: 반년짜리 공용좌석)도 해당일 전체 점유로 처리합니다. `group="본사"|"구로"`로 건물을 나눠 볼 수 있습니다.
> ⚠️ 회의실 **정원(수용인원) 정보는 아마란스에 존재하지 않습니다** — "10명 회의실" 류 조건은 지원할 수 없습니다.

**일정(캘린더)**
| 도구 | 기능 |
|---|---|
| `list_calendars` / `list_events` | 캘린더 목록 / 기간 일정 조회 |
| `create_event` | 개인 캘린더 일정 등록 |
| `update_event` | 일정 제목·내용·시간 수정(본인 작성만) |
| `delete_event` | 일정 삭제(본인 작성만, 소프트 삭제) |

**메일**
| 도구 | 기능 |
|---|---|
| `list_mailboxes` / `list_inbox` | 메일함 목록 / 받은메일 |
| `read_mail` | 메일 1건 본문(평문)·헤더·첨부목록 — 외부 이미지 자동로드 안 함 |
| `send_mail` | 메일 발송(첨부 지원, 받는사람 미지정 시 본인에게) |
| `delete_mail` | 메일 삭제(휴지통 이동) |
| `download_mail_attachment` | 첨부파일 저장(실행 없이 저장만) |

**게시판**
| 도구 | 기능 |
|---|---|
| `list_notices` | 공지/게시글 목록(본문 프리뷰, 검색어·기간 필터) |
| `read_notice` | 게시글 1건 본문(평문)·댓글 — ⚠️ 조회수 증가 |
| `list_attachments` / `download_attachment` | 게시글 첨부 목록 / 다운로드 |

**전자결재**
| 도구 | 기능 |
|---|---|
| `pending_approvals` | **미결 요약** — 제목·기안자·대기일수(오래 기다린 순) |
| `list_approvals` | 함별 문서 목록(미결/기결/수신참조/시행/상신) |
| `read_approval` | 문서 1건 본문(평문)·헤더·결재선 (열람 부작용 없음) |
| `approval_counts` | 함별 미처리 건수(숫자만 — 내용까지 보려면 `pending_approvals`) |
| `submit_approval` | 문서 상신 — ⚠️ 실제 결재요청 통지 발송 |
| `cancel_approval` | 상신 취소 — 상태별 3단계(결재취소→상신취소→`purge` 시 삭제) |
| `delete_temp_approval` | 임시보관 문서 삭제 — 상신취소로 되돌아온 문서·시험 잔여물 정리용 |
| `list_approval_lines` / `read_approval_line` | 개인결재라인 목록 / 결재자 구성 조회 |
| `save_approval_line` / `delete_approval_line` | 개인결재라인 생성·수정 / 삭제 (상신 아님, 재사용 config) |
| `suggest_approval_line` | **결재선 후보 제안** — 본인 직책으로 구간 판정 + 직책→사람 해석 (⛔ 확정 아님, 사용자 확인 필수) |
| `get_approval_line_schema` / `list_approval_line_schemas` | 문서 종류별 결재라인 스키마(직책 기반) 원본 |
| `get_submission_guide` / `list_submission_guides` | 양식별 신청 가이드(필수항목·절차·주의) |

**근태 · 조직 · 나**
| 도구 | 기능 |
|---|---|
| `get_attendance_today` | 오늘 출퇴근 현황(부작용 없음) |
| `attendance_month` | **기간(월) 근태** — 일자별 출퇴근·근무시간·지각/연차 + 기간 합계 |
| `clock_in` / `clock_out` | 출근·퇴근 기록 — ⚠️ 실제 근태 punch, 기존 기록은 덮어쓰지 않음 |
| `find_person` | **사람 찾기** — 이름·ID·이메일 → `empSeq`/부서/직책/연락처 |
| `org_chart` | 부서 트리 / 부서별 사원·직책 |
| `whoami` | 로그인한 본인 정보(`empSeq`·부서·이메일 + 근태용 `empCd` + 부서명·직책·직급) |

> 결재선 구성·회의 참석자·메일 수신자는 전부 `empSeq`를 요구합니다. `find_person`이 그 진입점이고, 본인 값은 `whoami`로 얻습니다.
> `find_person`의 첫 호출은 전사 명부를 조립하느라 1초 남짓 걸리고, 이후 30분간 캐시됩니다.

## 요구 사항

- **macOS · Linux · Windows** — Chrome/Firefox 쿠키에서 크레덴셜(`authToken`/`signKey`)을 가져옵니다. 쿠키 복호화 방식이 OS마다 달라 각각에 맞게 처리합니다.
- **Chrome 또는 Firefox로 `https://gw.innogrid.com` 에 로그인된 상태** — 세션이 없으면 도구 호출 시 로그인 안내를 반환합니다.

### 플랫폼별 크레덴셜 지원

| | macOS | Linux | Windows |
|---|---|---|---|
| **Chrome** | 키체인 `Chrome Safe Storage` → AES-128-CBC | 키링(`v11`, `secret-tool`) / `"peanuts"`(`v10`) → AES-128-CBC | `v10` DPAPI 키 / `v20` app-bound(Chrome Elevator COM) → AES-256-GCM |
| **Firefox** | ✅ (쿠키 평문) | ✅ | ✅ |

- **Firefox가 가장 확실한 크로스플랫폼 경로**입니다(쿠키가 평문이라 OS 무관). Chrome이 안 잡히면 Firefox로 `gw.innogrid.com`에 로그인하면 됩니다.
- **Windows Chrome 주의**: 최신 Chrome은 실행 중 쿠키 파일을 **배타적으로 잠급니다**. Chrome을 **완전히 종료**한 뒤 실행해야 쿠키를 읽을 수 있습니다. `v20` app-bound 쿠키는 Chrome Elevator COM으로 복호화를 시도하지만(best-effort) Chrome 버전/보안설정에 따라 거부될 수 있습니다.
- **어떤 브라우저에서도 못 가져오면** 값을 직접 지정할 수 있습니다(아래 [크레덴셜 직접 지정](#크레덴셜-직접-지정-수동)).

### 브라우저 경로 오버라이드 (snap/flatpak/커스텀 프로필)

브라우저가 표준 위치에 없으면(예: Ubuntu의 **snap Firefox** → `~/snap/firefox/common/.mozilla/firefox`) 환경변수로 직접 지정합니다:

| 환경변수 | 용도 |
|---|---|
| `INNO_CREED_FIREFOX_COOKIES` | Firefox `cookies.sqlite` 파일 경로(직접) |
| `INNO_CREED_FIREFOX_DIR` | Firefox 프로필 **디렉토리**(스캔) |
| `INNO_CREED_CHROME_COOKIES` | Chrome `Cookies` DB 파일 경로(직접) |
| `INNO_CREED_CHROME_USER_DATA` | Chrome `User Data` 루트 |

크레덴셜 취득에 실패하면 에러 메시지에 **Chrome/Firefox 각각 어떤 경로를 확인했는지**가 표시되니, 그 경로를 보고 위 환경변수로 실제 위치를 지정하면 됩니다. (쿠키는 있는데 복호화만 실패하면 키링/app-bound 안내가 함께 나옵니다.)

### 크레덴셜 직접 지정 (수동)

브라우저 복호화가 불가한 환경(예: **Windows 최신 Chrome app-bound**, Chrome을 못 닫는 상황)에서는 쿠키 값을 **직접 지정**하면 브라우저 읽기를 완전히 건너뜁니다. 다른 모든 경로보다 우선합니다.

| 환경변수 | 값 (브라우저 DevTools → Application → Cookies → `gw.innogrid.com`) |
|---|---|
| `INNO_CREED_AUTH_TOKEN` | `BIZCUBE_AT` 쿠키 값 (URL 인코딩된 `%7C`도 그대로 붙여넣기 가능) |
| `INNO_CREED_SIGN_KEY` | `BIZCUBE_HK` 쿠키 값 |

두 값이 **모두** 설정돼 있어야 사용됩니다. MCP 클라이언트로 실행할 땐 등록 설정의 `env` 블록에 넣으세요(셸 `export`는 전달되지 않음).

## 설치

> 📖 **처음이거나 남에게 공유한다면 → [단계별 설치 가이드 `docs/INSTALL.md`](docs/INSTALL.md)** (OS별 절차 · Gatekeeper/SmartScreen 우회 · 문제 해결 포함). 아래는 요약입니다.

### 프리빌트 바이너리 (권장)

[**릴리즈**](https://github.com/zilhak/inno-creed/releases/latest)에서 OS에 맞는 바이너리를 내려받으세요.

| OS / arch | 파일 |
|---|---|
| macOS (Apple Silicon) | `inno-creed-macos-arm64` |
| Linux x86_64 | `inno-creed-linux-x86_64` |
| Linux aarch64 | `inno-creed-linux-aarch64` |
| Windows x86_64 | `inno-creed-windows-x86_64.exe` |

macOS·Linux는 내려받은 뒤 실행 권한을 부여하세요: `chmod +x inno-creed-*`. (macOS에서 Gatekeeper가 막으면 `xattr -d com.apple.quarantine <파일>`.)

### 소스 빌드

```sh
git clone https://github.com/zilhak/inno-creed && cd inno-creed
cargo build --release   # → target/release/inno-creed (Windows는 inno-creed.exe)
```

**Rust 1.96+** (edition 2024, 번들 `libsqlite3-sys`가 최신 toolchain 요구)와 **C 컴파일러**(rusqlite 번들 SQLite 컴파일용)가 필요합니다.

### MCP 등록

Claude Code:

```sh
claude mcp add inno-creed -- /절대경로/inno-creed        # Windows는 ...\inno-creed.exe
```

다른 MCP 클라이언트는 stdio 전송으로 바이너리를 직접 실행하도록 설정하면 됩니다.

```json
{
  "mcpServers": {
    "inno-creed": {
      "command": "/절대경로/inno-creed"
    }
  }
}
```

등록 후 클라이언트를 재시작하면 도구가 노출됩니다. macOS에서 Chrome 크레덴셜을 쓸 경우 첫 실행 시 키체인(`Chrome Safe Storage`) 접근 허용 프롬프트가 한 번 뜹니다.

## 동작 방식

```
inno-creed (Rust MCP 서버, 헤드리스)
 ├─ creds    Chrome 쿠키 복호화(→ Firefox 폴백) → authToken / signKey
 ├─ sign     wehago-sign(HMAC-SHA256) · transaction-id 생성
 ├─ client   세션 lazy 취득(10분 TTL 캐시) · 헤더 주입 · POST · 응답 파싱
 ├─ modules  자원 · 일정 · 메일 · 게시판 · 전자결재 · 근태 · 조직 API 래퍼
 └─ mcp      rmcp stdio 서버: 도구 정의 · 소유권 가드 · read-back 검증
```

크레덴셜만 브라우저에서 빌려오고, 실행은 전부 순수 HTTP입니다. 서명·세션 규격은 [architecture.md](docs/architecture.md)에 정리돼 있습니다.

## 안전 규약

- **응답 성공 ≠ 실제 반영** — 서버는 권한 밖 대상에 대해 `successTf:true`를 주면서 실제로는 무시(silent no-op)합니다. 그래서 모든 mutation은 직후 **재조회(read-back)로 실제 상태를 확인**하고, 반영되지 않았으면 실패로 처리합니다.
- **소유권 가드** — 쓰기 도구는 대상의 `empSeq`가 본인일 때만 실행하고, 아니면 명시적 에러를 냅니다. 서버도 남의 데이터 수정을 무시하지만, MCP에서 먼저 걸러 원인을 분명히 알려줍니다.
- **부작용 있는 도구는 명시** — 근태 punch(`clock_in`/`clock_out`), 상신(`submit_approval`), 게시글 열람(`read_notice`, 조회수 증가)은 실제 기록이 남습니다. 사용자가 명시적으로 지시할 때만 호출하세요.
- **서버 자동 결재선 불신** — 서버가 채워주는 기본 결재선은 위임전결 규정과 일치하지 않습니다. `get_approval_line_schema`로 직책 기반 스키마를 받고, `org_chart`로 담당자를 해석한 뒤 사람이 확인하고 상신하세요.

## 문서

| 문서 | 내용 |
|---|---|
| [docs/architecture.md](docs/architecture.md) | 아키텍처, 크레덴셜 취득, 서명 규격, 공통 규약 |
| [docs/api-reference.md](docs/api-reference.md) | 모듈별 확정 API 스키마 |

`docs/`에는 **실증으로 확정된 사실만** 담습니다. 조사 과정·미확정 내용은 `.claude-workspace/analyze/`(git 미포함)에 있습니다.
