# API 레퍼런스 (확정)

> 모든 호출: `POST https://gw.innogrid.com{path}`, 헤더 4종 + Content-Type, body JSON(달리 표기 없으면). 응답은 [응답 봉투](architecture.md#6-응답-봉투). `companyInfo`는 공통 주입.

## 자원(회의실 예약) — `/schres/rs121*`

| API | 용도 | 메서드 래퍼 |
|---|---|---|
| `rs121A01` | 자원(회의실) 목록 | `list_resources` |
| `rs121A05` | 예약 현황 조회(기간·자원) | `list_reservations` |
| `rs121A06` | 예약 등록 | `create_reservation` |
| `rs121A10` | 예약 상세 조회 | `get_reservation` |
| `rs121A11` | 예약 삭제(휴지통) | `delete_reservation` |
| `rs121A12` | 예약 수정 | `update_reservation` |

### 부가 조회 API (읽기 확정 — 자원 HOME 진입 시 호출)

| API | 내용 |
|---|---|
| `rs121A24` | 자원종류 목록(`resAttrList`) — 회의실(attrSeq 1) / 인재INC 구로 오피스 회의실(3) |
| `rs121A28` | 자원속성 정의(`resPropList`) — 회의실명 / 법인차량 / 인재INC 구로 오피스 회의실 |
| `rs121A29` | 자원속성 값(`resDpropList`) |
| `rs121A49` | 자원 모듈 설정 — 운영시간(`confStartTime` `0000` / `confEndTime` `2400`), `resApprYn` 등 |

### 파생 도구 (새 API 없이 조합)

| 도구 | 기반 | 내용 |
|---|---|---|
| `find_free_rooms` | A01+A05 | 자원별 예약을 빼고 `duration_min` 이상 빈 구간만 반환. 종일·다일 예약은 해당일 전체 점유 처리 |
| `my_reservations` | A01+A05 | `empSeq`==본인 필터 + 슬림화. **수정/취소에 필요한 `seqNum`/`resIdx`를 얻는 정규 경로** |

- **건물 그룹**(`group`): `""`(전체) / `"본사"`→`attrSeq=1` / `"구로"`→`attrSeq=3`. 한 회사의 자원종류 구분이지 별도 시스템이 아니다.
- **`list_reservations`는 기본 슬림 응답**(`resSeq/resName/seqNum/resIdx/start/end/title/owner/ownerEmpSeq/attendees/allDay`). 원본 74필드는 회의 안건 전문(`descText`)까지 실려 오므로 `verbose:true`일 때만 그대로 준다.
- 시각은 도구 경계에서 **ISO(`2026-08-04T10:00`)로 정규화**. 서버 원본은 `resStartDate`(`202608041000`)와 `startDate`(`"2026-08-04 10:00:00"`)가 혼재하고 `endDate`는 아예 없다.

### 자원 ID (resSeq) — 실측

| 회의실 | resSeq |
|---|---|
| 회의실 A-1 | 45 |
| 회의실 A-2 | 46 |
| 회의실 B | 47 |
| 회의실 C | 48 |
| Meeting RM.3 | 57 |

### 날짜 포맷

- 등록/수정 `startDate`/`endDate`: `YYYYMMDDHHmm` (예: `202608071100`)
- 조회 기간 `startDate`/`endDate`(rs121A05): `YYYYMMDD`

### 예약 식별 키

한 예약은 `(resSeq, seqNum, resIdx)` 3-튜플로 식별된다.

- `resSeq`: 자원 ID(회의실).
- `seqNum`: 예약 시퀀스(number).
- `resIdx`: 반복/인스턴스 인덱스.

### ⚠️ 시간 변경 시 재발급 (핵심 함정)

`rs121A12`로 **시간(startDate/endDate)을 바꾸면 예약이 재발급**되어 `seqNum`과 `resIdx`가 **둘 다 새 값으로 바뀐다.** (이름/내용만 바꾸면 `seqNum` 유지.)

- 응답 `resultData`에 **새 `seqNum`(number)** 과 **새 `resIdx`** 가 담겨 온다:
  ```json
  {"successTf":true,"resIdx":3,"resSeq":"57","seqNum":71044}
  ```
- **`resIdx`는 JSON NUMBER 로 온다**(문자열 `"3"` 아님). 파싱 시 String/Number 양쪽을 처리해야 한다 — `.as_str()`만 쓰면 None → 옛 값 fallback → 다음 read-back이 `(새 seqNum, 옛 resIdx)` 불일치로 **에러 9208**. (`src/modules/resource.rs`의 `json_idx` 헬퍼가 이를 흡수.)
- 재발급마다 `resIdx`가 1→2→3…으로 증가.

> 따라서 수정 후에는 **응답에서 받은 새 seqNum/resIdx로** read-back 해야 한다.
> **자원 수정은 재발급(seqNum/resIdx 변경), 일정 수정은 in-place(schSeq 유지)** — 대비해서 기억할 것.

### 예약명 vs 화면 표시명 (`reqText` / `resTitleDisplay`) — 혼동 주의

**아마란스 자원 HOME 타임라인은 예약명을 표시하지 않는다.** 화면에 찍히는 것은 서버가 조립한
`resTitleDisplay` = `[예약자명] 자원명` 이고, 모든 사용자 예약이 같은 형식이다(2026-08-06 웹 실측).

| 필드 | 예 | 정체 | 어느 API에 있나 |
|---|---|---|---|
| `reqText` | `회의` | **실제 예약명**(사용자 입력값) | 목록(`rs121A05`)·상세(`rs121A10`) 둘 다 |
| `resTitleDisplay` | `[홍길동] 회의실 B` | 서버 조립 **표시용** 문자열 | **목록에만** — 상세에는 없다 |

- 그래서 "예약명을 `회의`로 넣었는데 화면엔 `[홍길동] 회의실 B`로 나온다"는 오해가 생긴다. 예약은 정상이다.
- 웹 예약 폼은 예약명 칸에 `[자원명] 사용자명` 류 기본값을 채워주며, 사용자가 그대로 두면 `reqText` 자체가
  그 형태가 된다(예: `[회의실 B] 홍길동`). MCP는 이름을 지어주지 않으므로 형식이 달라 보일 수 있다.
- MCP는 두 값을 `title`(목록)/`reqText`(등록·수정)와 `displayTitle`로 **함께** 반환한다. 상세 API에 없는
  등록·수정 경로에서는 같은 규칙으로 조립한다(`modules::resource::display_title`).

### 삭제 파라미터 (rs121A11)

- `statusCode: "CA"`, `deleteRangeCode: "UO"`.
- `resSeqList[]`에 상세조회(rs121A10) 스냅샷 필드(reqText/startDate/endDate/createDate/resName 등) 포함.
- 삭제=휴지통 이동.

## 일정(캘린더) — `/schres/sc111*`

| API | 용도 | 메서드 래퍼 |
|---|---|---|
| `sc111A02` | 캘린더 목록 | `list_calendars` |
| `sc111A03` | 일정 이벤트 조회(기간·캘린더) | `list_events` |
| `sc111A05` | 일정 등록 **및 수정**(공용) | `create_calendar_event` / `update_calendar_event` |
| `sc111A06` | 일정 삭제(소프트, 30일 휴지통) | `delete_calendar_event` |

- 개인 캘린더(등록 기본 대상)는 `sc111A02` 결과에서 `calType:"E"` + `empSeq==본인` 으로 판별. 날짜 `YYYYMMDDHHmm`.
- **`calRwGbn` ≠ `insertRwGbn`**(실증): 공용 캘린더는 `calRwGbn:"r"`(캘린더 자체는 읽기전용)이지만 `insertRwGbn:"w"` 라 **일정 등록은 된다**. 등록 가능 판정은 `insertRwGbn` 기준.
- **공용 캘린더 등록 실증 완료**: `mcalSeq:1147`(부서 공용, `calType:"M"`)에 등록 → 재조회 시 해당 공용 캘린더에 그대로 반영(개인 캘린더로 흘러가지 않음), `createSeq`=본인이라 본인이 삭제 가능. no-op 아님.
- 조회 `sc111A03`: `calList[]`(대상 캘린더)로 서버사이드 필터. 이벤트 식별 = `schSeq`(=`schmSeq`). 소유자 = `createSeq`.

### 등록 (sc111A05, 신규)

- `schSeq`/`schmSeq` **빈 문자열이면 신규(insert)**. 전체 필드(schTitle/startDate/endDate/mcalSeq/schPartEmpList 등) 포함.
- 응답: `{"schSeq":"89328","schmSeq":"89328"}`.

### ⚠️ 수정 (sc111A05, itemList diff) — 등록과 payload 구조가 완전히 다름

**같은 API(sc111A05)지만 수정은 전혀 다른 형태**다. `schSeq` 채우고 전체 필드를 보내면 **신규 INSERT로 처리됨**(함정). 수정은 반드시:

```json
{
  "companyInfo": {...},
  "schSeq": "89515", "schmSeq": "89515",
  "schGbnCode": "10",
  "rangeCode": "UO",                          // ← 수정 표식(필수)
  "itemList": [ ...변경 항목만 diff... ],
  "groupSeq": "...", "empSeq": "<본인 empSeq>",
  "alarmOnModify": false,
  "repeatType": "10", "alarm_yn": "N", "videoYn": "N",
  "videoTimeZone": "Asia/Seoul", "mailSend": "N", "langCode": "kr"
}
```

- **schSeq 유지(in-place)** — 자원과 달리 재발급 없음. 응답 `{"schSeq":"89515","schmSeq":"89515"}` 그대로.
- 수정 시 서버가 알림 발송 여부를 물음(UI). MCP는 `mailSend:"N"`으로 미발송.

#### itemList item 형식 (실측 확정)

각 항목은 `{"item":"<key>", ...값}`. **item명과 값 필드명이 다를 수 있음**(주의):

| item | 값 필드 | 형태 |
|---|---|---|
| `schTitle` | `schTitle` | `{"item":"schTitle","schTitle":"새제목"}` |
| `schContents` | **`contents`** | `{"item":"schContents","contents":"새내용"}` ← item≠값키 |
| `schDate` | `schDate`(객체) | `{"item":"schDate","schDate":{"startDate":"..","endDate":"..","allDay":"N","lunar":"N","lunarDate":""}}` |

- 시간 변경 객체 필드명은 **`allDay`/`lunar`**(등록 body의 `alldayYn`/`lunarYn`과 다름).
- 폼이 변경 여부와 무관하게 **항상 포함**하는 item: `videoYn`, `schParticipants`(add/update/removeSchPartEmpList), `mailSend`.
- **전체 item key 목록**(프론트 번들 chunk 20 실측 — 향후 확장 근거): `schCalendar`(캘린더 이동) · `schTitle` · `schDate` · `schAlarm` · `schContents` · `schMyMemo`(비밀메모) · `schParticipants` · `schDisclosure`(공개범위) · `schAddress` · `schPlace`(장소) · `schReservation`(자원예약) · `schAttachFile`(첨부) · `schRelatedWork`(유관업무) · `videoYn` · `mailSend`.

### 삭제 (sc111A06)

- ⚠️ **`companyInfo` 없이** 호출: `{"mcalSeq","schmSeq","schSeq","rangeCode":"","langCode":"kr"}`.
- 소프트 삭제(30일 휴지통 보관 후 영구삭제). 반복일정은 `rangeCode`로 범위 지정.

### 소유권 (수정/삭제)

- 이벤트 `createSeq == 본인 empSeq`일 때만 MCP가 실행. 아니면 명시적 에러.

## 메일 — `/mail/mail0*`

| API | 용도 | 래퍼 |
|---|---|---|
| `mail000A01` | 메일함(폴더) 목록 | `list_mailboxes` |
| `mail003A01` | 메일 목록 조회 | `list_mails` / `list_mail_inbox` / `list_mail_drafts` |
| `mail014A01` → `mail014A04` | 메일 발송(2단계, 첨부 지원) | `send_mail` |
| `mail014A01` → `mail014A14` | 메일 임시저장(2단계, 첨부 지원 — **발송 아님**) | `save_mail_draft` |
| `mail014A01`(draft) → (첨부 있으면 `mail014A08`) → `mail014A04` → `mail002A07` | **임시보관 메일 발송**(발송 후 원본 삭제. 첨부는 재업로드 없이 승계) | `send_mail_from_draft` |
| `mail014A06` | 발송·임시저장 첨부 업로드(multipart `file[]`) | `send_mail`/`save_mail_draft`의 `attachments` |
| `mail002A01` | 메일 상세 읽기(본문·헤더·첨부목록) | `read_mail` |
| `mail014A08` → `/ecm/ecm001A03` | 첨부 다운로드(2단계) | `download_mail_attachment` |
| `mail014A08` | 초안 첨부 승계(`fileSn` → 발송용 `fileId`) | `send_mail_from_draft` |
| `mail002A05` | 메일 삭제(휴지통) | `delete_mails` |

- 특이점: 메일 API는 body에 **`mainApiCode`**(예 `"mail003A01"`)를 명시해 라우팅(다른 모듈은 URL만). 메일 식별자 = `muid`.
- 메일함 mboxSeq 실측: INBOX `26986` / SENT `26989` / DRAFTS `26992` / TRASH `26995` / SPAM `26998`.

### 메일 목록 (mail003A01)

- body에 `mainApiCode:"mail003A01"` 필요(누락 시 실패).
- `pageSize`를 `TotalRecordCount` 이상으로 주면 **1회 호출로 전량 조회**(상한 없음).
- 정렬 `sort:"rfc822date"`, `sortType:"desc"`.
- ⚠️ **`boxName`은 서버가 무시한다. 조회 대상을 정하는 것은 `mboxSeq` 하나뿐이다**(실측:
  `mboxSeq=26989`(SENT)에 `boxName:"INBOX"`를 실어 호출하니 보낸메일 3건이 나왔다).
  그래서 임시보관함 조회도 **추가 API 없이 seq만 바꿔** 이 API를 그대로 쓴다.

#### 임시보관함 조회 → `list_mail_drafts`

`mboxSeq`를 DRAFTS로 바꾼 `mail003A01` 호출이다. 응답 형태는 받은메일함과 같다(배열 키 `Records`,
항목의 `muid`가 `read_mail`/`delete_mail`의 키).

⚠️ **메일함 seq를 상수로 박지 않는다** — 위 실측값은 이 계정의 값이고 계정마다 다르다.
`mail000A01`(`list_mailboxes`) 응답에서 `fullname`/`name`이 `"DRAFTS"`인 노드를 찾아 그
`mboxSeq`를 쓴다. 응답이 중첩으로 올 수 있어 트리를 훑고, `mboxSeq`가 숫자/문자열 어느 쪽으로
와도 흡수한다.

⚠️ **미확인 — `muid`가 재저장·발송 후에도 유지되는지는 확인하지 않았다.** 신규 저장 직후의
read-back(같은 muid로 목록에서 찾기)까지만 실증됐다. 이 저장소에는 **휴지통 이동 시 muid가
재부여된다**는 기록이 있어(`delete_mail` 설명), 임시저장 재저장이나 초안 발송에서도 같은 일이
일어날 가능성이 있다. 초안을 발송 대상으로 지목하는 경로를 만들 때는 **저장 시점의 muid를
그대로 발송 키로 신뢰하지 말고**, 발송 직전에 `list_mail_drafts`로 다시 확인할 것.
→ `send_mail_from_draft`가 그렇게 한다(아래 "임시보관 메일 발송"의 안전장치 절).

### 메일 발송 (mail014A01 → A04) — 실측·실증

**2단계**: `mail014A01`(작성폼 초기화, 컨텍스트 확보) → `mail014A04`(multipart 발송).

- `mail014A04`는 `multipart/form-data`. 헤더 서명 + **body 내 `authToken`**(형식 `loginId|groupSeq|empSeq|secret` — 헤더 authToken 앞에 loginId가 추가된 형태)을 함께 요구.
- 발송에 필요한 값은 **전부 A01 응답에서 동적 취득**(하드코딩 없음):

  | A04 FormData 필드 | 출처 |
  |---|---|
  | `from`, `email` | A01 `email` (순수 이메일, **표시형 아님**) |
  | `fromName` | 사용자명(세션) |
  | `to` | 수신자(표시형 `"이름 <email>"` 또는 이메일) |
  | `fileDir` | A01 **`filedir`**(소문자) |
  | `sessionKey`, `externalSendLimit`, `bigFileDay` | A01 동명 |
  | `insideDomainArray` | A01 동명(null이면 `"[]"`) |
  | `neobizaddr`/`neobizIntedAddr`/`neobizOrg` | A01 `groupMailOption.groupMail{Addr,IntedAddr,Org}` |
  | `mail_kind` | `"me"`(내게쓰기) |
  | `muid` | `"0"`(신규) |
  | 그 외 | `immediately:"false"`, `toBeDeleted:"false"`, `bigFileCnt:"0"` 등 실측 고정 |

- **응답은 표준 봉투로 감싸짐**: `{"resultCode":0,"resultData":{"result":true,"muid":..,"resultMessage":"SUCCESS"}}` — 성공 판정은 `resultData.result`(최상위 `result` 아님).
- 본인 앞 발송 실증(받은메일함 도착 확인).

### 메일 임시저장 (mail014A01 → A14) → `save_mail_draft`

**2단계**: `mail014A01`(작성폼 초기화) → `mail014A14`(multipart 임시저장). 발송은 일어나지 않는다.

- **폼은 A04(발송)와 동일**하다 — 위 표의 필드를 그대로 쓰고(코드에서도 같은 빌더를 공유), 아래 7개만 더 붙인다.

  | 추가 필드 | 신규 저장 시 값 | 의미 |
  |---|---|---|
  | `autoMUID` | `""` | 재저장 시 직전 draft muid |
  | `beforeMailType` | `"me"` | A01을 연 `mailKind` |
  | `beforeMUID` | `""` | 재저장 시 직전 muid |
  | `mailKey` | `""` | 재저장 시 A01 응답의 `mailkey` |
  | `isFirst` | `"0"` | ⚠️ **첫 저장이 "0"이다**(프론트가 `isFirst===true ? "0" : "1"`로 보낸다) |
  | `draftType` | `"true"` | 수동 임시저장(첨부 유무와 무관) |
  | `autoDraftType` | `"false"` | 에디터 자동저장(`mail014A13`)이 아님 |

- 제목이 비면 `"(제목없음)"`으로 저장한다(프론트 동작 재현).
- 응답: `resultData.autoMUID` = 저장된 임시보관 메일의 muid → 도구는 `draft_muid`로 반환.
  `resultCode 999` = "로그인 사용자가 변경되어 임시저장을 할 수 없습니다" → 전송 실패와 구분해 안내한다.
- **read-back 검증**: 저장 직후 임시보관함(위 `list_mail_drafts` 경로)을 재조회해 그 `muid`가
  목록에 실제로 있는지 보고 `verified_by_readback`으로 반환한다. **통수(+1) 비교가 아니라 muid
  대조**인 이유는 조회 사이에 메일이 들어오거나 다른 세션이 초안을 지우면 통수가 오탐이 나기 때문이다.
  조회가 실패하면 "확인 못 함"이지 저장 실패가 아니므로 에러 대신 `false`로 표시한다.
- 첨부는 발송과 같은 경로(`mail014A06` 업로드 → `uidAuthList`/`bigFileCnt`).
- **자동 임시저장(`mail014A13`, `draftType=false`/`autoDraftType=true`)은 에디터 전용**이라 MCP에는 구현하지 않았다.

### 임시보관 메일 발송 (mail014A01(draft) → A04 → mail002A07) → `send_mail_from_draft`

**초안 발송 전용 API는 없다.** 신규 발송과 **같은 `mail014A04`**를 쓴다(2026-08-06 브라우저 캡처).

#### ① 초안 열기 — `mail014A01`에 draft 좌표

신규 작성용 `{"mainApiCode":"mail014A01","mailKind":"me"}`와 달리 body가 이렇다:

```json
{"mailKind":"draft","mailTo":"(Unknown)","uid":"<draftMuid>","mbox":"DRAFTS",
 "viewFlag":"noRead","fromFlag":true,"readType":"","domainSeq":""}
```

이 한 콜이 발송에 필요한 것을 **전부** 준다 — 그래서 MCP가 호출마다 독립이어도 `mailkey`를
인자로 들고 다닐 필요가 없다:

| 응답 경로 | 쓰임 |
|---|---|
| `mailkey` | ③의 원본 삭제 키. ⚠️ **`.eml` 접미가 붙은 값**으로, 신규 A01의 `mailkey`와 형식이 다르다 |
| `sessionKey` / `filedir` | A04 폼의 `sessionKey`/`fileDir`(신규 A01 값과 다른 새 값) |
| `mailInfo.mime.header` | A04 폼의 `mimeHeader`(객체를 JSON 문자열로) |
| `mailInfo.mime.body.html` | 본문(`htmlContents`) |
| `mailInfo.mime.fileList` | 첨부 목록. 첨부 없는 초안은 `[]` |
| `decodeMime.subject` | 제목(디코드된 값 — mime 헤더의 `subject`는 `=?UTF-8?B?...?=` 상태다) |
| `decodeMime.to` | 수신자(디코드된 표시형. ⚠️ 아래 "수신자" 참조) |

⚠️ **경로를 줄여 적지 말 것** — 본문·첨부는 `mailInfo` 바로 밑이 아니라 `mailInfo.mime` 밑이다.
도구가 `mailInfo.body.html`/`mailInfo.fileList`도 보는 것은 응답 모양이 다를 때를 위한 fallback이지
정본이 아니다. 이 자리를 잘못 적으면 다음 사람이 fallback을 정본으로 승격시켜 첨부 가드가 항상 0을 읽는다.

#### ② 첨부 승계 — `mail014A08` (⚠️ `mail014A07`이 **아니다**)

초안이 이미 서버에 들고 있는 첨부는 **다시 업로드하지 않는다.** 초안 `fileSn`을 발송용 `fileId`로
바꾸는 콜 하나가 전부다 — 첨부를 붙인 채 초안을 만들어 발송해 본 실측에서 `mail014A07`은
**한 번도 호출되지 않았다**(A06 업로드 → A08 → A04 → A002A07 순).

```
POST /mail/mail014A08   (form-urlencoded)
  moduleGbn=MAIL
  authKeyMap={"email":<본인>,"empSeq":<empSeq>,"muid":<draftMuid>}
  fileSn=<초안 fileList의 fileSn들을 **콤마로 이어붙임**>
  condition=99
→ list[] : { fileId(발송용 새 토큰), filePath, fileName, originalFileName, fileExtsn,
             fileSize, encoding:"base64", offset, muid, authKeyMap, fileSn:"" }
```

⚠️ 세 가지 함정:
- 초안 `fileList[].fileSn`은 **업로드 때의 `fileId`와 다른 새 토큰**이다(저장을 거치며 재발급).
- 응답의 `fileSn`은 **빈 문자열**로 온다 — 폼에는 요청에 쓴 초안 토큰을 되살려 싣는다.
- `fileSize`는 원본이 아니라 **MIME 본문 크기**다(48B→"66"). 사용자에게 보여줄 크기가 아니다.

폼의 `uidAuthList` 원소 = **A08 응답 객체를 그대로 두고** 아래를 덧붙인 것:
`fileSize`→`"<n> Bytes"`, `authKeyMap`→**JSON 문자열로 한 번 더 감쌈**, `fileSn`→초안 토큰,
`link:"N"`·`fileDeleteYN:"Y"`·`serverFile:"Y"`·`useDownView:"N"`·`fileClass:"icon_<ext>"`·
`noConvertFileSize:<정수>`·`id:<인덱스>`.
⛔ **신규 업로드(`mail014A06`) 경로의 `uidAuthList`와 키 구성이 다르다** — 하나로 합치면 한쪽이 조용히 깨진다.

#### ③ 발송 — `mail014A04`

**필드 이름 집합은 신규 발송과 완전히 같다(37개).** 값만 5개 다르다:

| 필드 | 신규 발송 | 초안 발송 |
|---|---|---|
| `muid` | `"0"` | **그 초안의 muid** |
| `mail_kind` | `"me"` | **`"draft"`** |
| `mimeHeader` | `""` | **초안의 mime 헤더 JSON** |
| `fileDir`/`sessionKey` | 신규 A01 응답 | **draft 모드 A01 응답** |
| `uidAuthList`/`bigFileCnt`/`fwFile` | 업로드분 또는 `""`/`"0"`/`""` | **②의 승계분.** `fwFile`은 첨부 `originalFileName`을 콤마로 이은 목록 |

⚠️ `fwFile`은 신규 발송에서 늘 빈 값이라 의미를 몰랐던 필드다 — **초안 발송에서만 채워진다.**

응답 `resultData.result:true`로 성공 판정. ⚠️ 응답의 `muid`는 새 메일 id가 아니라 **보낸 초안의
muid를 그대로 되돌려준다**(신규 발송과 의미가 다르다).

⚠️ `mimeHeader`·`mail_kind:"draft"`가 **필수인지는 실측하지 않았다.** 브라우저가 보내므로 그대로
보낸다 — 생략 시험을 하지 않았으니 줄이지 말 것.

⚠️ **브라우저와 다른 값이 하나 있다** — `bigFilePeriod`. 브라우저는 기간 문자열을 싣지만 우리는
빈 값으로 보낸다. 위험은 낮다: `bigFileCnt=0`이면 쓸 데가 없고, 기존 `send_mail`이 빈 값으로
성공해 왔다. 다만 **대용량 첨부(`bigFile`) 경로는 실측하지 않았으므로 도구가 거부한다**(아래).

#### ④ 원본 삭제 — `mail002A07`

```json
{"mailKey":"<①의 mailkey(.eml)>","beforeMUID":<draftMuid>}
```

**서버는 자동으로 정리하지 않는다** — 프론트가 발송 직후 이 콜을 친다. 빼먹으면 보낸 메일이
임시보관함에 남아 다음에 또 보내는 사고가 난다. 발송·정리 직후 임시보관함은 줄었는데 **휴지통
건수는 그대로**여서 **휴지통을 거치지 않는 삭제로 보인다**(`mail002A05`의 휴지통 이동과 다르다).

#### 도구가 거는 안전장치

- **발송 전 muid 실재 확인** — 임시보관함을 조회해 그 muid가 있을 때만 보낸다. 위 "미확인 —
  `muid`가 재저장·발송 후에도 유지되는지" 항목이 요구한 확인이다. 조회 자체가 실패하면
  "없다"가 아니라 "확인 못 했다"이므로 발송하지 않는다.
  ⚠️ **이 조회는 최근 20건(`DRAFT_READBACK_PAGE`)만 훑는다** — 초안이 21건 이상 쌓인 계정에서는
  오래된 초안을 이 도구로 보낼 수 없다(막히는 방향이라 안전하지만, 웹에서 발송하거나 초안을
  정리해야 한다). 에러 문구가 "최근 N건에 없다"고 그대로 밝힌다.
- **판정 불가는 전부 거부** — 첨부 목록(`mailInfo.mime.fileList`)·본문(`…mime.body.html`)·
  제목(`decodeMime.subject`) 중 **하나라도 응답에서 읽어내지 못하면 보내지 않는다.**
  키를 못 찾은 것을 "첨부 없음"·"빈 본문"으로 취급하면 첨부가 빠지거나 **내용이 텅 빈 메일**이
  나가는데, 둘 다 회수할 수 없다. 재저장(2회차 이상) 초안의 응답 모양은 실측하지 않았다.
- **첨부는 승계하되 미실측 경로는 거부** — ②의 `mail014A08` 경로로 승계한다. 다음은 막는다:
  **파일명에 콤마**(폼의 `fwFile`이 콤마 구분이라 이름이 쪼개진다) · **동명 파일 둘 이상**
  (A08 응답 순서 보장이 미실측이라 이름으로 짝짓는데 짝을 확정할 수 없다) · **대용량 첨부**
  (`bigFile`/`bigFilePeriod`를 우리는 빈 값으로 보내므로 그 부분이 빠진 채 나간다) ·
  **A08 응답 개수 불일치**(적으면 빠뜨리고, 많으면 요청하지 않은 파일이 섞인 것이다).
  ⚠️ **첫 실사용에서 확인할 지점** — 대용량 첨부 감지는 응답에 `bigFile`/`bigFilePeriod`가
  값을 가진 채 실려 있는지로 판정하는데, **정상 초안의 응답에도 `bigFilePeriod`가 늘 실려 오는지는
  확인하지 못했다**(참조한 캡처가 발췌라 전문을 못 봤다). 실려 온다면 첨부 있는 초안이 전부
  거부된다 — fail-closed 방향이라 위험은 아니지만, 거부가 잦으면 이 키 목록부터 볼 것.
- **참조(cc/bcc) 걸린 초안은 거부** — 발송 폼은 `cc`/`bcc`를 **항상 빈 값으로** 보낸다.
  초안에 참조가 있어도 읽지 않으므로 그대로 보내면 **참조 수신자가 조용히 빠진 채 나가고 회수할 수
  없다.** 참조가 실려 오는 자리(`mailInfo.mime.header.cc`/`bcc` · `decodeMime.cc`/`bcc` ·
  최상위 `paramCc`/`paramBcc`) 중 하나라도 값이 있으면 거부하고, **어느 자리도 읽지 못하면
  "참조 없음"이 아니라 판정 불가로 보고 중단**한다(실측: 참조 없는 초안도 `mime.header.cc`를
  빈 문자열로 들고 온다).
  ⚠️ **`bcc`는 관측된 응답 어디에도 키가 없었다**(원래 메시지 헤더에 남지 않는 필드다) — 실려 오면
  막지만, "bcc가 있는데 응답에 실리지 않는" 경우는 이 검사로 잡히지 않는다(잔여 위험).
- **발송 성공 ≠ 정리 성공** — ④가 실패해도 발송은 성공으로 보고하되 `draft_deleted:false`와
  안내를 함께 실어 사람이 중복 발송을 막게 한다.
- **수신자** — 인자로 주면 그 값, 없으면 **`decodeMime.to`를 HTML 언이스케이프**한 값(비면
  `paramTo`). ⚠️ **`mime.header.to`를 쓰면 안 된다** — 실측 결과 그 값은 표시명이
  `=?UTF-8?B?...?=`로 MIME 인코딩된 데다 `<`/`>`가 `&lt;`/`&gt;`로 HTML 이스케이프돼 온다.
  브라우저가 실제로 A04에 싣는 값은 `decodeMime.to`를 언이스케이프한 표시형이다.
  세 자리의 형태 차이:

  | 경로 | 형태 |
  |---|---|
  | `mailInfo.mime.header.to` | `=?UTF-8?B?…?= &lt;주소&gt;` (MIME 인코딩 + HTML 엔티티) |
  | `decodeMime.to` | `표시명 &lt;주소&gt;` (디코드됨, 엔티티는 남음) ← **이것을 언이스케이프해 쓴다** |
  | `paramTo` | `주소` (순수 이메일) ← 위가 비었을 때의 대안 |

  ⛔ **주소 값에 본문 렌더러(`html_to_text`)를 쓰지 말 것.** 그쪽은 ① 태그 제거 → ② 엔티티 디코드
  순서라, 서버가 **이미 언이스케이프된** `홍길동 <a@b.c>`를 주면 `<a@b.c>`를 태그로 보고 통째로
  버린다(남는 `"홍길동 "`은 공백이라 빈 값 검사도 통과한다). 로컬파트가 `p`/`br`/`div`/`td` 같은
  블록태그로 시작하면(`<p.kim@x.co>`) 개행 하나로 치환되기까지 한다. `decodeMime.to`가 늘
  이스케이프돼 온다는 근거는 **관측 1건뿐**이므로, 주소에는 **엔티티 언이스케이프만** 적용하고
  결과에 `@`가 없으면 발송을 거부한다.

  ⚠️ 수신자 **없는** 초안은 `paramTo`가 `(Unknown)`으로 올 수 있다(그런 초안의 URL이
  `mailTo=(Unknown)`이었다) — 주소가 아니므로 "수신자 없음"으로 취급해 발송을 거절한다.

  ✅ **수신자가 여럿이어도 안전하다** — 도구도 브라우저도 `decodeMime.to` **문자열을 통째로**
  싣는다. 주소를 쪼개거나 다시 잇지 않으므로 구분자가 무엇이든 브라우저와 같은 값이 나간다
  (구분자 자체는 여전히 미실측이지만, 그 값을 해석할 일이 없어 위험이 아니다).

#### 아직 실측하지 않은 것

지어내지 말고 **먼저 재보라**. 도구는 아래 대부분을 명시적으로 거부한다(조용히 잘못 보내지 않는다).

- **참조(cc)의 형태·구분자** — 여러 명일 때 어떤 구분자로 오는지, `decodeMime.cc`가 실제로 존재하는지는
  확인하지 않았다. **다만 "참조가 걸린 초안을 보내면 참조가 빠진다"는 미실측이 아니라 확정된 사실이라
  도구가 거부한다**(위 안전장치). 승계를 구현하려면 cc가 저장된 초안을 실측해야 한다.
- **`bcc`가 응답에 실리는지** — 관측된 응답 어디에도 키가 없었다. 실려 오면 막지만, 실리지 않는
  형태로 저장돼 있다면 감지할 수 없다.
- **수신자 구분자** — 여러 명이 어떤 구분자로 들어가는지 확인하지 않았다(1명만 봤다).
  다만 도구가 그 값을 **해석하지 않고 통째로 옮기므로** 위험이 아니다(위 "수신자" 참조).
- **`mail014A08` 응답 순서가 요청 순서와 같은지** — 그래서 도구는 인덱스가 아니라 **이름으로 짝짓고**,
  이름이 겹치면(동명 파일) 거부한다.
- **동명 파일 2개 · 파일명에 콤마가 든 파일** — `fwFile`이 콤마 구분이라 깨질 수 있다. 거부한다.
- **대용량 첨부(`bigFile`/`bigFilePeriod`)** — 폼 값을 실측하지 않았다. 거부한다. ⚠️ 다만 대용량
  첨부가 `mailInfo.mime.fileList`에 실리지 않는 형태라면 그 검사에 걸리지 않는다(그것도 미실측).
- **재저장(2회차 이상) 초안** — `autoMUID`/`beforeMUID`/`isFirst=1` 경로를 타는 초안은 만들어보지 않았다.
  응답 모양이 다를 수 있어, 본문·제목·첨부목록을 못 읽으면 발송하지 않는 가드가 이 경우의 방어선이다.
- **`mimeHeader`·`mail_kind:"draft"`의 필수 여부** — 생략 시험은 실패하면 메일이 잘못 나가므로 하지 않았다.

### 메일 상세 읽기 (mail002A01) → `read_mail`

```
POST /mail/mail002A01   body(JSON): { uid: <muid> }
→ resultData.mime.body.{html,plain}   본문
  resultData.mime.fileList[] : { originalFileName, fileExtsn, fileSize, fileSn }  첨부
  resultData.decodeMime : { subject, from, to, date, email }  헤더(엔티티 인코딩됨 &lt;&gt;)
```

- 도구는 본문을 **평문화**해서 반환(HTML은 `html_to_text`, plain 우선). ⚠️ **렌더링하지 않으므로 외부 이미지(추적 픽셀)를 자동 fetch하지 않음** — 외부 리소스가 있으면 `remoteResourceCount`로 개수만 경고. (보안: 열람 유출 방지)
- 첨부는 메타데이터(name/ext/`fileSizeApprox`/fileSn/isImage)만 반환. `fileSn`은 **호출마다 바뀌는 세션 토큰** → read 직후 다운로드에 사용.
- ⚠️ `fileList[].fileSize`는 원본 바이트가 아니라 **MIME 본문(base64+줄바꿈) 크기**라 실제보다 ~33% 큼 → 도구는 `fileSizeApprox`(≈ ×3/4)로 근사해 반환. **정확한 크기는 `download_mail_attachment`의 `bytes`**. (게시판 `ecm001A04`의 fileSize는 원본 그대로라 이 보정 불필요)
- ⚠️ 조회수/읽음처리: UI는 별도 `mail002A15`(seen)를 호출한다. `mail002A01` 단독은 읽음 부작용이 없는 것으로 관측(미확정).

### 메일 첨부 다운로드 (mail014A08 → ecm001A03) → `download_mail_attachment`

**2단계**: `mail014A08`(fileSn→다운로드 fileId 변환) → `/ecm/ecm001A03`(바이트).

```
1) POST /mail/mail014A08  (x-www-form-urlencoded)
   moduleGbn=MAIL & authKeyMap={"email","muid","empSeq"} & fileSn=<read_mail의 fileSn>
   → resultData.list[0].fileId  (256자 다운로드 토큰)
2) POST /ecm/ecm001A03  (x-www-form-urlencoded, 응답=바이너리)
   moduleGbn=MAIL & authKeyMap={"muid","email","empSeq"} & fileSn=<fileId> & condition=99
   → 파일 바이트(Content-Disposition에 원본 파일명)
```

- **게시판 첨부와 동일한 ECM 다운로드 엔드포인트**(`/ecm/ecm001A03`) — `moduleGbn`만 `MAIL`, authKeyMap이 mail용(muid/email/empSeq)으로 다름. (번들의 `ecmapi/*.do`는 여전히 무관·404)
- `email` = 수신함 소유자(본인) = `ensure_session`의 `emailAdd@emailDomain`.
- ⚠️ 바이트를 파일로 **저장만** 한다(열거나 실행하지 않음) → 악성 첨부도 격리 후 정적분석에 안전. 이미지 첨부는 저장 후 그대로 이미지로 판독 가능(실측: PNG 왕복 무손상).

### 메일 발송 시 첨부 (mail014A06 업로드) → `send_mail(attachments)`

`send_mail`에 로컬 파일 경로(`attachments`)를 주면 **2단계**로 첨부 발송:

```
1) POST /mail/mail014A06  (multipart/form-data)
   파일마다 field 이름 file[] 로 append (번들 실측: forEach(f => form.append("file[]", f)))
   → resultData.list[] : { fileId(발송용 토큰), originalFileName, fileExtsn, fileSize, filePath, moduleGbn:"MAIL" }
2) POST /mail/mail014A04  (기존 발송 + 아래 필드 채움)
   uidAuthList = JSON배열[ { fileName, fileSize:"N Bytes", fileExtsn, title, fileClass:"icon_<ext>",
                            fileId, filePath, noConvertFileSize:<int>, moduleGbn:"MAIL", id:<idx>, ... } ]
   bigFileCnt  = 첨부 개수
```

- 실증(MCP 왕복): 임시 PNG/TXT를 `attachments`로 자기 앞 발송 → `read_mail`에 첨부 2개 도착 → `download_mail_attachment`로 되받아 **원본과 바이트 동일**.

### 메일 삭제 (mail002A05)

- `{"uids":"muid,muid","mailKey":"","boxName":""}` — `uids`=콤마구분 muid(다건).
- 삭제=휴지통 이동. ⚠️ **이동 시 `muid` 재부여** → 이후 추적은 재조회 필요.

## 게시판 — `/board/APIHandler/*` (읽기 전용)

헤더 서명만으로 완결(`companyInfo` 불필요). 표준 응답봉투.

### 목록 (ViewBoardNewAndNoticeArtList) → `list_notices`

```
POST /board/APIHandler/ViewBoardNewAndNoticeArtList
body(JSON): { page, pageSize, sort:"write_date", sortType:"desc",
              menuCode:"UFA", pageCode:"UFA1000", moduleCode:"UF",
              noticeYn:"Y", apiName:"ViewBoardNewAndNoticeArtList",
              use_list_art_content:"Y",   # 본문 프리뷰 포함
              searchTitle/searchNick/searchDesc/... : "" }  # 검색 필터(빈값=전체)
→ resultData.articleList[] : { art_seq_no, art_title, cat_title(게시판명), cat_seq_no(게시판id),
                               mbr_nick(작성자), dept_name, write_date, read_cnt, file_cnt,
                               art_read_yn, is_new_yn, uid(첨부 fileIds), art_content(프리뷰) }, totalCnt
```

**필터(실측):**
- **검색** `search`+`field`: field="title"→searchTitle, "content"→searchDesc, "author"→searchNick, 그 외/빈값→searchTotal(통합). ✅ 동작.
- **날짜** `searchStartDate`/`searchEndDate`: **반드시 `YYYYMMDD`(구분자 없음)**. 대시(`2026-07-31`)를 넣으면 `resultCode:500 시스템 오류` → `modules/board.rs`가 입력을 숫자만 남겨 정규화. ✅ 동작(범위 밖 글 제외 확인).
- **게시판별** `searchBoard`: ❌ **이 엔드포인트(전 게시판 공지 집계)에선 무시됨** — cat_seq_no(492/502)를 넣어도 필터 안 됨(0건도 아니고 전체 반환). 특정 게시판 목록은 `ViewBoardArtList`가 별도로 있으나 cat_seq_no가 아닌 미상의 "게시판 코드"를 요구("게시판 코드가 없습니다") → 라이브 캡처 필요, 미구현. 현재는 출력 `boardId`로 클라이언트단 구분만.

### 상세 (ViewPost) → `read_notice`

```
POST /board/APIHandler/ViewPost
body(JSON): { art_seq_no, menuCode:"UFA", pageCode:"UFA1000", moduleCode:"UF",
              adminPage:"N", externalYn:"N", presentPassword:"", isPrint:"N" }
→ resultData.art       : 게시글(art_content = 본문 HTML), read_cnt는 number
  resultData.board     : 게시판 메타(cat_title = 게시판명; art.cat_title은 null)
  resultData.remarkList: 댓글
```

- ⚠️ **ViewPost 호출 시 조회수(read_cnt) 증가** — 순수 조회가 아니라 실제 "열람" 처리.
- `art_content`는 인라인 스타일 포함 HTML(수십 KB) → `modules/board.rs`가 태그 제거·엔티티 디코드로 평문화해서 반환.
- 필드 타입 혼용 주의: `read_cnt`가 목록에선 문자열, 상세에선 정수 → `json_str`로 흡수.

### 첨부 목록 (ecm001A04) → `list_notice_attachments`

```
POST /ecm/ecm001A04           # ⚠️ .do 없음, /ecmapi 아님 (번들 상수 ecmapi/*.do는 별개 컴포넌트용)
Content-Type: application/x-www-form-urlencoded
body: moduleGbn=BOARD
      authKeyMap={"empSeq":<본인>,"cat_seq_no":"U","art_seq_no":<글번호>,
                  "survey_no":"","reply_seq_no":"","fileIds":<attachmentUid>}
      fileSn=0  condition=99
→ resultData.list[] : { fileId, originalFileName, fileExtsn, fileSize, linkedFilePath(저장경로) }
```

### 첨부 다운로드 (ecm001A03) → `download_notice_attachment`

```
POST /ecm/ecm001A03           # ecm001A04와 동일 패턴, 응답은 바이너리
Content-Type: application/x-www-form-urlencoded
body: 위와 동일 + fileSn=<파일 순번(0-base, 목록 배열 인덱스)>
→ (성공) 파일 바이트. Content-Disposition에 원본 파일명.
  (실패) content-type=json 봉투 → 에러 처리.
```

- **엔드포인트 주의**: 게시판 첨부는 `/ecm/ecm001AXX`(`.do` 없음, `/ecmapi` 없음) 계열. 번들 상수의 `ecmFileDownUrl=/ecm/ecmapi/ecm001A03.do` 등 `.do` 경로는 **다른 파일 컴포넌트용이라 서명 호출 시 404** — 혼동 금지. `ecm001A05`=**삭제**이므로 다운로드로 호출 금지.
- 다중 첨부: `list_notice_attachments`가 `fileIds`(콤마 구분 uid 전체)로 목록을 받고, `download_notice_attachment`는 `file_sn`(순번)으로 단건 지정.

## 전자결재 — `/eap/*` (읽기 + 쓰기)

헤더 서명만으로 완결(목록/상세는 companyInfo 불필요, 카운트만 필요). 표준 봉투. 엔드포인트·필드는 실제 트래픽 캡처로 확정했다.

**미구현은 승인/반려뿐**이다 — 조직 의사결정 행위라 의도적으로 제외했다. 상신·상신취소·임시보관삭제·개인결재라인 CRUD는 구현·e2e 실증 완료.

### 함별 목록 → `list_approvals`

수신계열은 `eap105A04`(응답 `resultData.map.{list,totalCount}`), 상신함은 `eap107A04`(응답 `resultData.list.{list,totalCount}`).

```
POST /eap/eap105A04  (수신계열) | /eap/eap107A04 (상신)
body(JSON): { eaBoxId, nMenuID=menuNo, menuNo, upperMenuNo=eaBoxId,
              page, pageSize, sfrDt, stoDt,   # 기간 YYYYMMDD(빈값이면 도구가 최근 ~3개월로 채움)
              periodPicker, sortField, sortType:"DESC", fDocSts:[], sFormId:["0"],
              useElasticSearch:true, useElasticSearch_new:true }
→ list[] item: { DOC_ID, DOC_NO, DOC_TITLE, FORM_NM, FORM_ID, USER_NM(기안자), DEPT_NM,
                 DOC_STSNM(종결/반려/진행), lineUserName(현재결재자), READYN, REP_DT/ARRIVED_DT/END_DT,
                 COMMENT_COUNT, FILE_CNT }
```

**함 → (eaBoxId, menuNo, periodPicker, API)** (실측):

| box_name | 함 | eaBoxId | menuNo | period | API |
|---|---|---|---|---|---|
| pending | 미결문서 | 1000900 | 1001000 | ARRIVED_DT | eap105A04 |
| approved | 기결문서 | 1000900 | 1001100 | ACTION_TIME | eap105A04 |
| approved_ongoing | 기결(진행) | 1000900 | 1001110 | ACTION_TIME | eap105A04 |
| approved_done | 기결(종결) | 1000900 | 1001120 | ACTION_TIME | eap105A04 |
| reference | 수신참조 | 1000900 | 1001200 | REP_DT | eap105A04 |
| enforcement | 시행문서 | 1000900 | 1001400 | REP_DT | eap105A04 |
| sent | 상신문서 | 1000300 | 1000400 | REP_DT | eap107A04 |

- ⚠️ **빈 기간이면 서버 기본이 좁아 문서를 놓침** → 도구가 최근 ~3개월(오늘−92일~오늘)로 자동 보정.

### 문서 상세 → `read_approval`

```
POST /eap/eap111A04
body(JSON): { doc_id, form_id, bindType:"V", setReadYn:"N",   # N=열람 부작용 없음(도구 기본)
              p_doc_id:0, doc_auth:"0", spDocId:"", commentReqYn:"N", pageCode:"UBA1100", docToken:"" }
→ resultData: { docTitle, docNo, docStsName, empName, deptName, repDt, attachCnt, lineName(현재결재자),
                contentsWord(본문 평문), docContents/contents(HTML), user_info[](결재선 처리내역) }
```

- 도구는 `contentsWord`(평문) 우선, 없으면 `docContents` 태그제거. `user_info[]`는 처리시각/여부(이름 미노출→코드).

### 미처리 카운트 → `approval_counts`

```
POST /eap/api/getMenuCountInfo   (companyInfo 필요 → ensure_session)
body: { deptSeq, userSe:"USER|AT", compSeq, bizSeq(=compSeq), empSeq, groupSeq, menuType:"", pageCode:"EapSide" }
→ resultData: { "<menuNo>": "<count>", ... }  # 도구가 menuNo를 box 라벨로 변환
```

### 개인결재라인 CRUD

| API | 용도 | 래퍼 |
|---|---|---|
| `eap102A02` | 라인 목록 | `list_approval_lines` |
| `eap102A05` | 라인 상세(결재자 구성) | `read_approval_line` |
| `eap102A10` | 라인 생성·수정 | `save_approval_line` |
| `eap102A09` | 라인 삭제 — body `{"lineIdList":[<행 객체>]}` | `delete_approval_line` |

- ⚠️ **삭제는 `lineId` 숫자가 아니라 `eap102A02`가 준 행 객체를 통째로** 넘겨야 한다. 그래서 `list_approval_lines`가 각 항목에 원본 행을 `_row`로 실어 준다.
- ⚠️ **결재자 객체는 부분 지정이 안 된다.** `[{"user_id":"<empSeq>"}]`만 주면 저장은 성공하고 `lineId`도 돌아오지만, **`eap110A03`이 그 라인을 결재자 0명으로 해석한다**(2026-08-06 실측). `read_approval_line`이 주는 멤버 객체(27필드 — `co_id`/`dept_id`/`duty_cd`/`grade_cd`/`act_id`/`org_div` 등)를 그대로 재사용할 것. 배열 순서 = 결재 순서이고 순서 필드(`doc_line_seq` 등)만 서버가 자동 주입한다.

### 상신 → `submit_approval`

```
POST /eap/eap110A03   결재선 병합 + form_info.form_d_tp 취득 (읽기, docID:0)
POST /eap/eap110A06   상신 → resultData.result = 신규 docId
```

- **`eap110A03`은 개인 라인을 그대로 쓰지 않는다.** 양식필수 합의자·수신참조(`m_Refer`)·시행자(`m_Oper`)를 병합해 돌려주고, 그 결과가 그대로 결재선이 된다. 즉 **개인 라인에는 결재(act_id 3000)만 담으면 된다.** 병합 결과 실측:

  | 양식 | 결재 | 수신참조 | 시행 |
  |---|---|---|---|
  | 외근신청서(41) | 개인 라인 그대로 | 인사총무팀·재무회계팀·자금팀 | — |
  | 연차휴가신청서(36) | 개인 라인 그대로 | 시행자 1명 + 인사총무팀·인사지원실 | 1명 |
  | 휴직신청서(67) | 개인 라인 그대로 | 없음 | 없음 |

  > `eap110A03`은 **부작용 없는 읽기 콜**이다. 상신 전에 "이 라인으로 올리면 누구에게 가는가"를 미리 확인하는 용도로 쓸 수 있다(`tests/live`의 상신 시나리오가 이 방식으로 사전 가드를 건다).

- **근태 양식(`form_d_tp`가 `HP_HPD0110_*`)은 상신 전에 HP 연동 5콜이 선행돼야 한다.** 빠뜨리면 `eap110A06`이 `resultCode 2099`로 실패한다:

  ```
  /human/attendapplication/0hr00011            HP 신청 검증·스테이징
  /human/attendapplication/create              HP 신청 커밋 → appSq, appDt
  /system/apiUtilEap/GetLinkKey                → linkKey        (approKey — 대문자 K)
  /personal/hpd0110/saveAttendApplicationLinkKey   linkKey ↔ appSq 명시 바인딩
  /system/apiUtilEap/SetEnageGroup             approKey에 linkKey·formDTp·콜백 API 등록
  ```

- ⚠️ `form_d_tp`는 **양식마다 다르다**(연차36 `_00011` / 출장40 `_00021` / 외근41 `_00031` / 휴일43 `_00051` / 휴직67 `_00015` / 교육42 `_00041`). 하드코딩 금지 — `eap110A03` 응답에서 동적 취득한다. 그래서 a03를 interlock 등록보다 먼저 호출한다.
- casing 함정: `/system/`·`/personal/` 계열은 **`approKey`**(대문자 K), `eap110A03`/`A06`은 **`approkey`**(소문자).
- `bindData`는 **이중 인코딩**(`JSON.stringify` 두 번)해 전송한다. 본문 HTML은 `encodeURIComponent`.
- 페이로드의 신원 필드(`coCd`/`deptCd`/`empCd`/이름)는 도구가 **로그인 사용자 값으로 덮어쓴다** — 가이드 예시에 박힌 타인 신원이 그대로 상신되는 것을 막기 위해서다.

### 상신취소 → `cancel_approval`

상태(`doc_sts`)에 따라 단계가 달라진다. 사전조회는 `docId`(소문자), 실행 3콜은 `docID`(대문자) — 실측 확정.

| 단계 | 대상 doc_sts | API | 전이 |
|---|---|---|---|
| 사전조회 | — | `eap110A98 {docId, pageCode:"UBAP002"}` | → `doc_sts` |
| 결재취소 | 30(진행중) | `eap110A54 {docID, formID, actID:"", pageCode:"UBAP002"}` | 30→20 |
| 상신취소 | 20(상신) | `eap110A18 {docID, pageCode:"UBAP002"}` | 20→10 (**문서채번 반납**) |
| 임시보관삭제 | 10(보관) | `eap110A19 {docID, pageCode:"UBAP001"}` | 소멸 |

- **상신 직후 문서는 `doc_sts` 30**이라 `eap110A18`만으로는 `2116`("결재자가 결재하여 상신 취소할수 없습니다") — 반드시 `eap110A54`가 선행돼야 한다.
- `eap110A54`는 `formID`를 요구하는데 `eap110A98` 응답에는 없다 → 호출자가 `list_approvals`의 `formId`를 넘겨야 한다.
- ⭐ **이 3단계는 HP 근태 레코드(`appSq`)까지 회수한다** (2026-08-06 실측, docId 141760: 상신으로 `appSq 33147` 생성 → `purge` 취소 후 HP 목록에서 소멸, 총건수 59 → 59). 되돌리기는 완전하다.
- 검증은 `read_approval`이 `2156`("삭제된 문서는 열 수 없습니다")를 주는지로 한다. `purge:false`면 임시보관(10)에 남고 `read_approval`은 `2385`(임시저장).

### 임시보관 문서 삭제 → `delete_temp_approval`

```
GET /eap/sse/eap107A25?docIdList=<csv>     # ⚠️ SSE 스트림(다른 API와 형태가 다름)
```

### ⚠️ 되돌릴 수 없는 것 — HP 근태 레코드 고아

**상신이 실패**하면(`eap110A06` 에러) `create`가 만든 HP 신청 레코드만 남고 취소할 eap 문서가 없어 **지울 방법이 없다.** soft delete(`/human/hrd0220/updateAttendApprovalDeleteReq`)는 `deleteReqYn=Y` 플래그만 세우고 `approState 2`로 잔존하며, 배포 번들 전수 조사에도 **하드삭제 API가 없다**.

- 현황 조회(도구 미노출, `probe`로만): `POST /human/attendapplication/at00001` `{approStateList:["0".."5"], linkAtCdList:[], startDate, endDate, calendarViewType:"DEFAULT"}`
- 상신을 자동으로 반복하는 테스트는 **전후로 이 목록을 찍어 고아 발생을 감지**해야 한다(`tests/live`가 그렇게 한다).

## 통합검색 — `/gw/APIHandler/gw018A02`

| API | 용도 | 메서드 래퍼 |
|---|---|---|
| `gw018A02` | 메일·결재·게시판·일정·자원·파일 통합검색 | `search` |

```json
{"header":{},
 "body":{"tsearchKeyword":"연차","tsearchSubKeyword":"","boardType":"6",
         "fromDate":"2026-08-01","toDate":"2026-08-31","dateDiv":"",
         "detailSearchYn":"N","selectDiv":"S","orderDiv":"B","syncTime":"N",
         "pageIndex":1,"hrSearchYn":"N","hrEmpSeq":"","pageSize":10,"webMobileDiv":"W"}}
```

응답: `{totalcount, resultgrid:[...]}`.

### boardType = 모듈 구분자 (실측 확정)

| boardType | 모듈 | 후속 조회용 ID |
|---|---|---|
| `0` | 메일 | `muid` → `read_mail` |
| `3` | 일정 | `schSeq` |
| `6` | 전자결재 | `docId`+`formId` → `read_approval` |
| `9` | 게시판 | `artSeqNo` → `read_notice` |
| `10` | 파일(첨부) | `fileId` |
| `13` | 자원(예약) | `resSeq` |

- **모듈별 전용 검색 API는 없다.** `mail003A01`·`eap105A04`에 검색 파라미터를 넣어도 서버가 버린다. 이 API가 유일한 검색 경로.
- ⚠️ 검색어 필드는 **`tsearchKeyword`**. `searchText`/`keyword` 등은 조용히 무시된다.
- ⚠️ **`dateDiv`는 반드시 빈 문자열**. `"A"`/`"R"`/`"W"` 등 값을 넣으면 날짜 필터가 **통째로 무효화**된다(에러 없음). 날짜 형식은 `2026-08-01`·`20260801` 둘 다 가능.
- ⚠️ 결재의 `deptNm`/`userNm`/`formNm`은 **다국어 객체**(`{kr,en,jp,cn}`)로 온다. 문자열로 단정하면 빈 값.
- 검증(결재/"연차"): 무필터 6 → 2020년 0 → 2026-08 2 → 2026년 6.

## 근태 — `/human/*`

| API | 용도 | 메서드 래퍼 |
|---|---|---|
| `common/judgeTimeManagement/getTodayComeLeaveInfo` | 오늘 출퇴근 조회 | `get_attendance_today` |
| `common/judgeTimeManagement/getJudgeTimeManagement` | **출퇴근 punch(쓰기)** | `attendance_clock_in`/`attendance_clock_out` |
| `openapi/worktime/status/getWorkTimeStatusList` | **기간(월) 근태 현황** | `attendance_month` |

### 기간 근태 현황 (getWorkTimeStatusList)

```json
{"coCd":"<coCd>","startDate":"20260701","endDate":"20260804","empCdList":["<본인 empCd>"]}
```

- 응답 = 배열, **1행 = 1일**(74필드). 핵심: `atDt` / `attresultCd`·`attresultNm` / `comeTm`·`leaveTm` / `basicworkTm`·`overworkTm`·`standardworkTm`(분) / `holiFg`·`holiNm` / `atNm`(연차 등 사유).
- `attresultCd`: `9101`=지각 `9102`=조퇴 `9103`=결근 `9104`=정상근무 `9301`=휴일.
- ⚠️ **`comeTm`/`leaveTm`이 `HHmm` 4자리**다 — 같은 근태 도메인인 `getTodayComeLeaveInfo`의 12자리(`YYYYMMDDHHmm`)와 형식이 다르다.
- ⚠️ **누락일이 있다**(마감 전인 당일 등). 요청 35일 → 29행 실측. 날짜는 행 순서가 아니라 `atDt`로 인덱싱할 것.
- ⚠️ 연차 사용일은 `comeTm`/`leaveTm`이 비었는데 `basicworkTm`=480(인정근무). **출퇴근 기록 유무로 근무일을 판정하지 말 것** — `attresultNm`/`atNm`을 같이 봐야 한다.
- `judgeTimeManagement` 계열에는 월별 API가 **없다**. 이 openapi 경로가 담당(2026-08-04 실측).

## 조직 — `/gw/APIHandler/gw102*`

| API | 용도 | 메서드 래퍼 |
|---|---|---|
| `gw102A01` | 부서 트리 | `org_chart` |
| `gw102A02` | 부서별 사원 목록 | (내부) |
| — | 이름/ID/이메일로 사람 찾기 | `find_person` |

- `find_person`은 **전사 명부를 조립해 클라이언트에서 거른다**(30분 캐시). 실측 75개 부서 중 인원 있는 곳만 동시 8개로 조회, 첫 호출 ~1.1초 / 이후 ~2ms, 명부 326명.
- ⚠️ **서버측 인물 검색은 존재하지 않는다**(2026-08-04 실측):
  - `gw102A02`의 `searchText`는 **서버가 조용히 무시** — 검색어와 무관하게 부서 인원 전원(17명)을 그대로 반환.
  - `/ab/ab099A23`(주소록 검색)은 JSON이 아닌 응답을 반환해 사용 불가.
- ⚠️ 전사 일괄 조회 불가 — `gw102A02`에 회사/사업장 노드(`orgGubun` `c`/`b`)를 주면 **0명**. 부서(`d`) 단위 순회가 유일.
- 명부 326명 vs 루트 노드 `childUserCnt` 353 — 차이는 미해소. 부서에 소속되지 않은 인원이 있을 수 있으므로 **"명부에 없음"을 "재직하지 않음"으로 단정하지 말 것**.

## 세션 정보 (내부) — `gw050A02`

도구가 아니라 **내부 lazy 부트스트랩**. 세션 값이 필요할 때 `ensure_session()`이 호출하고 **10분 TTL 캐시**.

```
POST /gw/gw050A02
Content-Type: application/x-www-form-urlencoded
body: a10Domain=https://gw.innogrid.com        # 유일 파라미터
→ resultData.sessionInfo.ucUserInfo = { compSeq, deptSeq, empName, emailAdd, emailDomain,
                                        erpEmpSeq, erpDeptSeq, erpCompSeq, ... }
```

- Bearer 인증 헤더(4종)만으로 "이미 로그인된 사용자"의 sessionInfo 반환 — 별도 CSRF 토큰 불필요.
- **한 응답으로 UC 계열(compSeq/deptSeq/email)과 근태/ERP 계열(erpEmpSeq=empCd, erpDeptSeq=deptCd, erpCompSeq=coCd)을 동시 확보** → 이전 `mail000A01`+`sc111A02` 2회 부트스트랩을 대체.
- 브라우저는 SSO 진입 시 이 응답을 `sessionStorage.userInfo`에 캐시. MCP는 동일 API를 직접 호출.

## 공통 안전 규약

- **소유권 가드**: 자원/일정 mutation은 대상 소유자(`empSeq`/`createSeq`)==본인일 때만 실행, 아니면 명시적 에러.
- **read-back 검증**: 모든 mutation 직후 재조회로 실제 반영 확인. 서버가 `successTf:true`를 주며 무시(no-op)하는 경우 방어. 자원 시간수정은 새 seqNum/resIdx로, 일정은 유지된 schSeq로 재조회.

## 미조사 (다음 단계)

- **전자결재(`/eap/*`)**: 읽기 3종 + 개인결재라인 CRUD + **상신·상신취소·임시보관삭제** 구현 완료(근태 4양식 순수 API e2e 실증). **미구현은 승인/반려뿐** — 조직 의사결정 행위라 의도적 제외.
- 전자결재 첨부(문서에 파일 붙여 상신)는 미조사 — 상신 payload의 `fileGroup`/`attachCnt` 자리만 확인.
- **메신저(대화방)**: gw API 미노출 — 별도 제품(웹 통합알림 `event02A01`도 MAIL/BOARD/HPD만, 메신저 이벤트 없음). 자동화하려면 메신저 서비스 별도 리버싱 필요.
- 메일 상세 본문·첨부는 구현 완료(read_mail/download_mail_attachment).
- **메일·결재 검색 구현 완료** — 통합검색 `gw018A02`(위 섹션). 모듈별 전용 검색 API는 존재하지 않는다.
- 게시판: 읽기(목록/상세/검색/날짜필터/첨부 목록·다운로드) 구현 완료. **미구현** — 쓰기(글/댓글 등록), 특정 게시판별 목록(`ViewBoardArtList`의 "게시판 코드" 라이브 캡처 필요).
- 근태: punch(`attendance_clock_in`/`attendance_clock_out`)·오늘 조회·**기간 조회(`attendance_month`)** 구현 완료. **근태 신청은 전자결재 상신 경로에 포함**(`submit_approval`의 HP 연동 5콜 — 위 전자결재 절). 미조사 — 연차 잔여(`/human/hrd0620/0hr00001` 등 경로만 확보), 근태 승인.
- ⚠️ **HP 근태 신청의 하드삭제 API는 존재하지 않는다**(번들 전수 조사). 상신이 실패해 생긴 고아 레코드는 회수 불가 — 위 전자결재 절의 "되돌릴 수 없는 것" 참조.
- **회의실 정원(capacity) — 아마란스에 개념 자체가 없음(확정, 재조사 불필요)**. 4중 확인: `rs121A01` 응답에 필드 없음 / langPack 10만 문자열에 "수용인원" 0건 / 자원 HOME·예약 다이얼로그 화면에 정원 표시·입력 없음 / **자원 속성 체계(`rs121A28`·`rs121A29`)가 "회의실명"·"법인차량" 같은 분류 태그일 뿐 숫자 속성이 아님**. "N명 들어가는 회의실" 류 질의는 이 시스템으로 답할 수 없다.
- ⚠️ `rs121A02`~`A04`는 자원 등록·수정(**쓰기**)일 가능성이 있어 미확인으로 둔다. 공용 자원에 부작용이 가므로 맹목 호출 금지.
- 일정 확장 item(schCalendar/schAlarm/schMyMemo/schDisclosure/schPlace/schReservation 등)은 key만 확보, 값 구조 미실측.

## 호출자 함정과 그 처리 (MCP 인자·응답 설계 규칙)

실제 회귀 점검에서 **호출자가 앞 도구의 출력을 그대로 다음 도구에 물렸다가 실패한** 사례들이다.
전부 "조심해서 피하라"가 아니라 **도구 쪽에서 원인을 없애는 방향**으로 처리했다. 새 도구도 같은 규칙을 따를 것.

### 1. ID 타입 불일치 → 인자에서 문자열/숫자 양쪽 수용

조회 도구는 ID를 문자열로 주는데(`list_approval_lines` → `lineId:"2047"`) 쓰기 도구는 정수를 요구했고
(`save_approval_line.line_id: i64`), 같은 값을 받는 `read_approval_line.line_id`는 **문자열**이었다.
즉 어느 쪽을 찍어도 절반은 틀린다. rmcp 에러(`invalid type: string "2047", expected i64`)에는
**어느 인자인지도 안 나온다.**

→ `src/mcp/args/mod.rs`의 `flex` 모듈로 **ID·순번·개수·날짜코드 인자 62개 전부** 양쪽을 받게 했다.
스키마도 `["integer","string"]`으로 넓혀 **둘 다 된다는 사실을 모델에게 알린다**(조용히 받아주기만 하면
모델은 계속 한쪽을 찍고 실패한다). 숫자로 해석 불가한 문자열은 여전히 거절한다 — 조용히 0으로 만들지 않는다.

### 2. 같은 이름, 다른 의미 → 이름이 아니라 **출처**를 설명

| 인자 | 값 | 출처 |
|---|---|---|
| `download_notice_attachment.file_sn` | **0-base 인덱스**(정수) | `list_notice_attachments` → `files[].fileSn` |
| `download_mail_attachment.file_sn` | **서버 토큰 문자열**(긴 base64류) | `read_mail` → `attachments[].fileSn` |

옛 이름 `download_attachment` 가 `download_mail_attachment` 와 같은 뿌리를 써서 순번을 넣었다가 **HTTP 422**를 맞았다.
→ 도구 이름에 도메인을 박아(`download_notice_attachment`) 뿌리 충돌 자체를 없앴다. 다만 **인자 이름 `file_sn`은
여전히 두 도구가 공유**하므로, 두 도구 설명에 서로를 대조해 적고 `list_notice_attachments` 응답에
**`fileSn`(인덱스)을 실어** 호출자가 배열을 직접 세지 않게 했다.

### 3. 숫자를 문자열로 흘리지 말 것

`list_notices`의 `fileCnt`가 문자열 `"0"`이라 **첨부 없는 글이 "첨부 있음"으로 판정**됐다(`"0"`은 truthy).
→ 정규화 단계에서 **정수로 변환**(`board::file_cnt`). 서버가 문자열/정수를 혼용해도 응답 타입은 하나로 고정한다.

### 4. 응답 배열 키가 도구마다 다르다 → 도구 설명에 명시

`articles`(게시판) / `Records`(받은메일) / `documents`(결재함) / `lines`(결재라인) / `events`(일정) /
`reservations`(예약) / `rooms`(빈 회의실) / `files`(첨부) / `branches[].steps`(결재선 제안).

특히 **`list_mail_inbox`만 서버 원본 봉투를 그대로 반환**한다(다른 목록 도구는 정규화돼 있다). 도구 설명에
그 사실과 키를 적어 뒀다. 새 목록 도구는 **정규화하고, 배열 키를 도구 설명에 적는다**.
