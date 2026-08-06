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
| `resTitleDisplay` | `[이재학] 회의실 B` | 서버 조립 **표시용** 문자열 | **목록에만** — 상세에는 없다 |

- 그래서 "예약명을 `회의`로 넣었는데 화면엔 `[이재학] 회의실 B`로 나온다"는 오해가 생긴다. 예약은 정상이다.
- 웹 예약 폼은 예약명 칸에 `[자원명] 사용자명` 류 기본값을 채워주며, 사용자가 그대로 두면 `reqText` 자체가
  그 형태가 된다(예: `[회의실 B] 강승억`). MCP는 이름을 지어주지 않으므로 형식이 달라 보일 수 있다.
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
| `sc111A05` | 일정 등록 **및 수정**(공용) | `create_event` / `update_event` |
| `sc111A06` | 일정 삭제(소프트, 30일 휴지통) | `delete_event` |

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
| `mail003A01` | 메일 목록 조회 | `list_mails` / `list_inbox` |
| `mail014A01` → `mail014A04` | 메일 발송(2단계, 첨부 지원) | `send_mail` |
| `mail014A06` | 발송 첨부 업로드(multipart `file[]`) | `send_mail(attachments)` |
| `mail002A01` | 메일 상세 읽기(본문·헤더·첨부목록) | `read_mail` |
| `mail014A08` → `/ecm/ecm001A03` | 첨부 다운로드(2단계) | `download_mail_attachment` |
| `mail002A05` | 메일 삭제(휴지통) | `delete_mails` |

- 특이점: 메일 API는 body에 **`mainApiCode`**(예 `"mail003A01"`)를 명시해 라우팅(다른 모듈은 URL만). 메일 식별자 = `muid`.
- 메일함 mboxSeq 실측: INBOX `26986` / SENT `26989` / DRAFTS `26992` / TRASH `26995` / SPAM `26998`.

### 메일 목록 (mail003A01)

- body에 `mainApiCode:"mail003A01"` 필요(누락 시 실패).
- `pageSize`를 `TotalRecordCount` 이상으로 주면 **1회 호출로 전량 조회**(상한 없음).
- 정렬 `sort:"rfc822date"`, `sortType:"desc"`.

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

### 첨부 목록 (ecm001A04) → `list_attachments`

```
POST /ecm/ecm001A04           # ⚠️ .do 없음, /ecmapi 아님 (번들 상수 ecmapi/*.do는 별개 컴포넌트용)
Content-Type: application/x-www-form-urlencoded
body: moduleGbn=BOARD
      authKeyMap={"empSeq":<본인>,"cat_seq_no":"U","art_seq_no":<글번호>,
                  "survey_no":"","reply_seq_no":"","fileIds":<attachmentUid>}
      fileSn=0  condition=99
→ resultData.list[] : { fileId, originalFileName, fileExtsn, fileSize, linkedFilePath(저장경로) }
```

### 첨부 다운로드 (ecm001A03) → `download_attachment`

```
POST /ecm/ecm001A03           # ecm001A04와 동일 패턴, 응답은 바이너리
Content-Type: application/x-www-form-urlencoded
body: 위와 동일 + fileSn=<파일 순번(0-base, 목록 배열 인덱스)>
→ (성공) 파일 바이트. Content-Disposition에 원본 파일명.
  (실패) content-type=json 봉투 → 에러 처리.
```

- **엔드포인트 주의**: 게시판 첨부는 `/ecm/ecm001AXX`(`.do` 없음, `/ecmapi` 없음) 계열. 번들 상수의 `ecmFileDownUrl=/ecm/ecmapi/ecm001A03.do` 등 `.do` 경로는 **다른 파일 컴포넌트용이라 서명 호출 시 404** — 혼동 금지. `ecm001A05`=**삭제**이므로 다운로드로 호출 금지.
- 다중 첨부: `list_attachments`가 `fileIds`(콤마 구분 uid 전체)로 목록을 받고, `download_attachment`는 `file_sn`(순번)으로 단건 지정.

## 전자결재 — `/eap/*` (읽기 전용)

헤더 서명만으로 완결(목록/상세는 companyInfo 불필요, 카운트만 필요). 표준 봉투. 상세: `.claude-workspace/approval-analysis/07`.

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

- **쓰기(상신/승인/반려) 미구현**: 실 결재 발생. 상세의 `btnList`·`outProcessInfo`에 실마리만 확보.

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
| `common/judgeTimeManagement/getJudgeTimeManagement` | **출퇴근 punch(쓰기)** | `clock_in`/`clock_out` |
| `openapi/worktime/status/getWorkTimeStatusList` | **기간(월) 근태 현황** | `attendance_month` |

### 기간 근태 현황 (getWorkTimeStatusList)

```json
{"coCd":"1000","startDate":"20260701","endDate":"20260804","empCdList":["11097"]}
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

- **전자결재(`/eap/*`) 읽기 3종 구현 완료**(`list_approvals`/`read_approval`/`approval_counts`). 쓰기(상신/승인/반려)는 미조사(실 결재 부작용).
- **메신저(대화방)**: gw API 미노출 — 별도 제품(웹 통합알림 `event02A01`도 MAIL/BOARD/HPD만, 메신저 이벤트 없음). 자동화하려면 메신저 서비스 별도 리버싱 필요.
- 메일 상세 본문·첨부는 구현 완료(read_mail/download_mail_attachment).
- **메일·결재 검색 구현 완료** — 통합검색 `gw018A02`(위 섹션). 모듈별 전용 검색 API는 존재하지 않는다.
- 게시판: 읽기(목록/상세/검색/날짜필터/첨부 목록·다운로드) 구현 완료. **미구현** — 쓰기(글/댓글 등록), 특정 게시판별 목록(`ViewBoardArtList`의 "게시판 코드" 라이브 캡처 필요).
- 근태: punch(`clock_in`/`clock_out`)·오늘 조회·**기간 조회(`attendance_month`)** 구현 완료. 미조사 — 연차 잔여(`/human/hrd0620/0hr00001` 등 경로만 확보), 근태 신청/승인.
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
| `download_attachment.file_sn` | **0-base 인덱스**(정수) | `list_attachments` → `files[].fileSn` |
| `download_mail_attachment.file_sn` | **서버 토큰 문자열**(긴 base64류) | `read_mail` → `attachments[].fileSn` |

이름이 같아 순번을 넣었다가 **HTTP 422**를 맞았다. → 두 도구 설명에 서로를 명시적으로 대조해 적었고,
`list_attachments` 응답에 **`fileSn`(인덱스)을 실어** 호출자가 배열을 직접 세지 않게 했다.

### 3. 숫자를 문자열로 흘리지 말 것

`list_notices`의 `fileCnt`가 문자열 `"0"`이라 **첨부 없는 글이 "첨부 있음"으로 판정**됐다(`"0"`은 truthy).
→ 정규화 단계에서 **정수로 변환**(`board::file_cnt`). 서버가 문자열/정수를 혼용해도 응답 타입은 하나로 고정한다.

### 4. 응답 배열 키가 도구마다 다르다 → 도구 설명에 명시

`articles`(게시판) / `Records`(받은메일) / `documents`(결재함) / `lines`(결재라인) / `events`(일정) /
`reservations`(예약) / `rooms`(빈 회의실) / `files`(첨부) / `branches[].steps`(결재선 제안).

특히 **`list_inbox`만 서버 원본 봉투를 그대로 반환**한다(다른 목록 도구는 정규화돼 있다). 도구 설명에
그 사실과 키를 적어 뒀다. 새 목록 도구는 **정규화하고, 배열 키를 도구 설명에 적는다**.
