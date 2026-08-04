# inno-creed

아마란스(gw.innogrid.com, 더존 WEHAGO/BIZCUBE 그룹웨어) 내부 API를 직접 호출하는 **Rust MCP 서버**.

브라우저 없이 순수 HTTP + HMAC 서명으로 그룹웨어를 조회·조작한다(회의실 예약, 메일 등).

## 문서

| 문서 | 내용 |
|---|---|
| [architecture.md](architecture.md) | 아키텍처, 크레덴셜 취득, 서명 규격, 공통 규약 |
| [api-reference.md](api-reference.md) | 모듈별 확정 API(자원/메일) 스키마 |

> 조사 과정·미확정 내용은 `.claude-workspace/analyze/`(git 미포함)에 있다. 이 `docs/`는 **실증으로 확정된 사실**만 담는다.

## 현재 도구 (MCP)

**자원(회의실)**
| 도구 | 기능 |
|---|---|
| `list_resources` | 회의실(자원) 목록 |
| `list_reservations` | 기간·자원별 예약 현황(자원 미지정 시 전체) |
| `reserve_resource` | 회의실 예약(등록 + read-back 검증) |
| `update_reservation` | 예약 수정(본인 소유, 재발급 seqNum 추적, read-back) |
| `cancel_reservation` | 예약 취소(본인 소유, read-back) |

**일정(캘린더)**
| 도구 | 기능 |
|---|---|
| `list_calendars` / `list_events` | 캘린더 목록 / 기간 일정 조회 |
| `create_event` | 개인 캘린더 일정 등록(read-back) |
| `update_event` | 일정 수정 — 제목/내용/시간(본인 작성, in-place, read-back) |
| `delete_event` | 일정 삭제(본인 작성, 소프트 삭제, read-back) |

**메일**
| 도구 | 기능 |
|---|---|
| `list_mailboxes` / `list_inbox` | 메일함 목록 / 받은메일 |
| `send_mail` | 메일 발송(2단계, 본인/타인, 실증) |
| `delete_mail` | 메일 삭제(휴지통 이동) |

**게시판**
| 도구 | 기능 |
|---|---|
| `list_notices` | 최근 공지/게시글 목록(본문 프리뷰 포함) |
| `read_notice` | 게시글 1건 본문(평문)·댓글 — ⚠️ 조회수 증가 |

## 빌드 · 설치

```sh
cargo build --release
claude mcp add inno-creed -- $(pwd)/target/release/inno-creed
# 등록 후 Claude Code 재시작 → 도구 노출
```

첫 실행 시 macOS 키체인(`Chrome Safe Storage`) 접근 허용 프롬프트가 한 번 뜬다.

## 핵심 원칙

- **응답 성공 ≠ 실제 반영**: 서버는 `successTf:true`를 주면서 실제로는 무시(no-op)할 수 있다(권한 밖 대상 등). 모든 mutation은 **read-back(재조회)로 검증**한다.
- **소유권 가드**: 쓰기 도구는 대상 `empSeq == 본인`일 때만 실행하고, 아니면 명시적 에러. (서버도 남의 예약 수정을 무시하지만, MCP에서 먼저 걸러 명확한 오류를 준다.)
