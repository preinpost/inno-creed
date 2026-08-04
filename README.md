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

**회의실(자원)**
| 도구 | 기능 |
|---|---|
| `list_resources` | 회의실(자원) 목록 |
| `list_reservations` | 기간·자원별 예약 현황(자원 미지정 시 전체) |
| `reserve_resource` | 회의실 예약 |
| `update_reservation` | 예약 수정(본인 소유만) |
| `cancel_reservation` | 예약 취소(본인 소유만) |

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
| `list_approvals` | 함별 문서 목록(미결/기결/수신참조/시행/상신) |
| `read_approval` | 문서 1건 본문(평문)·헤더·결재선 (열람 부작용 없음) |
| `approval_counts` | 함별 미처리 건수 |
| `submit_approval` | 문서 상신 — ⚠️ 실제 결재요청 통지 발송 |
| `cancel_approval` | 상신취소(임시보관 복귀) |
| `list_approval_lines` / `read_approval_line` | 개인결재라인 목록 / 결재자 구성 조회 |
| `save_approval_line` / `delete_approval_line` | 개인결재라인 생성·수정 / 삭제 (상신 아님, 재사용 config) |
| `get_approval_line_schema` / `list_approval_line_schemas` | 문서 종류별 결재라인 스키마(직책 기반) |
| `get_submission_guide` / `list_submission_guides` | 양식별 신청 가이드(필수항목·절차·주의) |

**근태 · 조직**
| 도구 | 기능 |
|---|---|
| `get_attendance_today` | 출퇴근 현황 조회(부작용 없음) |
| `clock_in` / `clock_out` | 출근·퇴근 기록 — ⚠️ 실제 근태 punch, 기존 기록은 덮어쓰지 않음 |
| `org_chart` | 부서 트리 / 부서별 사원·직책 (결재선 담당자 해석용) |

## 요구 사항

- **macOS** — 크레덴셜을 macOS 키체인 + 브라우저 쿠키 DB에서 가져옵니다. (다른 OS는 미지원)
- **Rust 1.85+** (edition 2024)
- **Chrome 또는 Firefox로 `https://gw.innogrid.com` 에 로그인된 상태** — 로그인 세션이 없으면 도구 호출 시 로그인 안내를 반환합니다.

## 빌드 · 설치

```sh
git clone <이 저장소> && cd inno-creed
cargo build --release
```

Claude Code에 MCP 서버로 등록:

```sh
claude mcp add inno-creed -- $(pwd)/target/release/inno-creed
```

등록 후 Claude Code를 재시작하면 도구가 노출됩니다. 다른 MCP 클라이언트라면 stdio 전송으로 바이너리를 직접 실행하도록 설정하면 됩니다.

```json
{
  "mcpServers": {
    "inno-creed": {
      "command": "/절대경로/inno-creed/target/release/inno-creed"
    }
  }
}
```

첫 실행 시 macOS 키체인(`Chrome Safe Storage`) 접근 허용 프롬프트가 한 번 뜹니다. 허용해야 쿠키를 복호화할 수 있습니다.

> 프리빌트 릴리즈 바이너리는 아직 제공하지 않습니다. 기능이 더 안정되면 OS별로 빌드해 릴리즈에 올릴 예정입니다.

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
