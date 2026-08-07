//! 일정(캘린더) 모듈 — `/schres/sc111*`
//!
//! ⭐ **이 모듈의 mutation 함수(`*_and_verify`)는 소유권 가드와 read-back 검증을 포함한다**
//! (`docs/architecture.md` §7). 자원 모듈과 같은 형태다. 검증 없는 raw 래퍼
//! (`upsert_event`/`update_event_items`/`delete_event`)도 남아 있지만 **새 호출부는 `*_and_verify`를 쓸 것.**
//!
//! ⚠️ 자원 예약과 반대로 **수정은 in-place**다 — `schSeq`가 유지되고 재발급되지 않는다
//! (자원은 `rs121A12`가 시간 변경 시 `seqNum`을 새로 발급한다).
//! 소유자 필드도 다르다: 일정은 `createSeq`("작성자"), 자원은 `empSeq`("소유자").

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::error::{InvalidInput, NotOwner};

/// 내가 볼 수 있는 캘린더 1건(`sc111A02` 결과에서 쓰는 필드만 추림).
#[derive(Clone, Debug)]
pub struct Calendar {
    pub mcal_seq: String,
    /// 표시 이름 (예: "개인캘린더.홍길동", "R&D 본부")
    pub title: String,
    /// `E`=개인, `M`=부서/공용
    pub cal_type: String,
    /// 캘린더 소유자 empSeq
    pub owner_emp_seq: String,
    /// 일정 등록 가능 여부(`insertRwGbn == "w"`). 개인 캘린더 외 공용 캘린더도 등록 가능한
    /// 경우가 있다. ⚠️ 서버가 무시(no-op)할 수 있으므로 등록 후 read-back으로 최종 확인한다.
    pub can_insert: bool,
    pub color: String,
}

/// 내가 볼 수 있는 캘린더 목록. 클라이언트 캐시(10분 TTL)를 경유하므로 반복 호출해도
/// `sc111A02`는 TTL당 1회만 나간다. 조회(list_events)와 등록(create_calendar_event)이 같은 목록을 쓴다.
pub async fn calendars(c: &GwClient) -> Result<Vec<Calendar>> {
    let raw = match c.cached_calendars() {
        Some(list) => list,
        None => {
            let data = list_calendars(c).await?;
            let list = data
                .get("resultList")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            c.set_calendars(list.clone());
            list
        }
    };
    let s = |v: &Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    Ok(raw
        .iter()
        .map(|v| Calendar {
            mcal_seq: s(v, "mcalSeq"),
            title: s(v, "calTitle"),
            cal_type: s(v, "calType"),
            owner_emp_seq: s(v, "empSeq"),
            can_insert: s(v, "insertRwGbn") == "w",
            color: s(v, "calColor"),
        })
        .collect())
}

/// `Calendar` 목록 → `sc111A03`의 `calList` 형식.
/// ⚠️ **빈 `calType`은 `"E"`로 보정한다**(실측 — 서버가 빈 값을 주는 캘린더가 있고, 그대로
/// 넘기면 그 캘린더의 일정이 조회 결과에서 빠진다). `adminYn`은 조회용이라 항상 `"Y"`.
fn to_cal_list(cals: &[Calendar]) -> Vec<Value> {
    cals.iter()
        .map(|c| {
            json!({
                "mcalSeq": c.mcal_seq,
                "calType": if c.cal_type.is_empty() { "E" } else { &c.cal_type },
                "adminYn": "Y",
                "color": c.color
            })
        })
        .collect()
}

/// 내가 볼 수 있는 전체 캘린더를 `sc111A03` `calList` 형식으로 구성(조회 대상 전체).
pub async fn all_cal_list(c: &GwClient) -> Result<Vec<Value>> {
    Ok(to_cal_list(&calendars(c).await?))
}

/// 본인 개인 캘린더(`calType == "E"` + 소유자 == 본인). 등록 기본 대상.
pub fn personal<'a>(cals: &'a [Calendar], my_emp_seq: &str) -> Option<&'a Calendar> {
    cals.iter()
        .find(|c| c.cal_type == "E" && c.owner_emp_seq == my_emp_seq)
}

/// mcalSeq 또는 캘린더 이름(부분 일치)으로 캘린더를 찾는다.
pub fn find<'a>(cals: &'a [Calendar], key: &str) -> Option<&'a Calendar> {
    cals.iter()
        .find(|c| c.mcal_seq == key)
        .or_else(|| cals.iter().find(|c| c.title.contains(key)))
}

/// 등록 대상 캘린더 결정: 지정 없으면 본인 개인 캘린더, 지정하면 mcalSeq/이름으로 해석.
/// 등록 권한(`insertRwGbn`)이 없는 캘린더는 선택 가능한 목록과 함께 명시적 에러.
///
/// ⚠️ 에러 종류를 구분해서 올린다 — **지정한 키가 틀렸거나 권한 없는 캘린더를 골랐으면
/// 호출자 잘못(`InvalidInput` → `invalid_params`), 본인 개인 캘린더를 못 찾는 건 서버 상태
/// 이상(`anyhow` → `internal_error`)**이다. 이동 전 `mcp.rs`의 분류를 그대로 보존한 것.
pub fn resolve_target<'a>(
    cals: &'a [Calendar],
    key: Option<&str>,
    my_emp_seq: &str,
) -> Result<&'a Calendar> {
    let writable = || {
        cals.iter()
            .filter(|c| c.can_insert)
            .map(|c| format!("{}({})", c.title, c.mcal_seq))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let cal = match key {
        Some(k) => find(cals, k).ok_or_else(|| {
            InvalidInput::new(format!("'{k}' 에 해당하는 캘린더 없음. 등록 가능: {}", writable()))
        })?,
        None => personal(cals, my_emp_seq).ok_or_else(|| {
            anyhow!("본인 개인 캘린더를 찾지 못했습니다. 등록 가능: {}", writable())
        })?,
    };
    if !cal.can_insert {
        return Err(InvalidInput::new(format!(
            "'{}' 캘린더는 일정 등록 권한이 없습니다. 등록 가능: {}",
            cal.title,
            writable()
        ))
        .into());
    }
    Ok(cal)
}

/// `resolve_target`의 네트워크 포함판(캘린더 목록을 캐시 경유로 가져온다).
pub async fn resolve_target_cal(c: &GwClient, key: Option<&str>) -> Result<Calendar> {
    let cals = calendars(c).await?;
    Ok(resolve_target(&cals, key, &c.emp_seq())?.clone())
}

/// 특정 날짜(`YYYYMMDD`)의 일정 목록에서 `schSeq` 매칭 원본을 찾는다.
/// read-back·소유권 확인의 공통 재료 — 일정 상세 단건 조회 API가 없어서 목록에서 골라낸다.
pub async fn find_event(c: &GwClient, sch_seq: &str, date: &str) -> Result<Value> {
    let cal_list = all_cal_list(c).await?;
    let events = list_events(c, date, date, cal_list)
        .await
        .map_err(|e| anyhow!("일정 조회 실패: {e}"))?;
    events
        .get("resultList")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter()
                .find(|e| e.get("schSeq").and_then(|v| v.as_str()) == Some(sch_seq))
                .cloned()
        })
        .ok_or_else(|| {
            InvalidInput::new(format!("일정을 찾을 수 없음 (schSeq={sch_seq}, date={date})")).into()
        })
}

/// 소유권 가드 — 일정 원본의 `createSeq`가 로그인 사용자와 같은지.
/// ⚠️ 자원(`empSeq`)과 **필드명도 호칭도 다르다**. 필드가 없으면 `""`와 비교해 거부한다.
fn check_author(orig: &Value, me: &str, action: &'static str) -> Result<()> {
    let owner = orig.get("createSeq").and_then(|v| v.as_str()).unwrap_or("");
    if owner != me {
        return Err(NotOwner {
            relation: "작성",
            kind: "일정",
            action,
            owner: owner.to_string(),
            me: me.to_string(),
        }
        .into());
    }
    Ok(())
}

/// 캘린더 목록 — `sc111A02` (원본 응답. 캐시를 거치려면 `calendars()` 사용)
pub async fn list_calendars(c: &GwClient) -> Result<Value> {
    c.call(
        "/schres/sc111A02",
        &json!({
            "companyInfo": c.company_info(),
            "calType": "",
            "langCode": "kr"
        }),
    )
    .await
}

/// 일정 이벤트 조회 — `sc111A03`. `cal_list`=조회 대상 캘린더 배열(`sc111A02`에서 구성).
pub async fn list_events(c: &GwClient, start: &str, end: &str, cal_list: Vec<Value>) -> Result<Value> {
    c.call(
        "/schres/sc111A03",
        &json!({
            "companyInfo": c.company_info(),
            "startDate": start,
            "endDate": end,
            "mySchYn": "N",
            "calList": cal_list,
            "tcalList": [],
            "acalList": [],
            "searchEmpSeq": "",
            "sortDate": "Y",
            "langCode": "kr"
        }),
    )
    .await
}

/// `YYYYMMDDHHMM` → `YYYY-MM-DDTHH:MM`. 형식이 어긋나면 원본을 그대로 돌려준다.
fn to_iso(s: &str) -> String {
    if s.len() != 12 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return s.to_string();
    }
    format!("{}-{}-{}T{}:{}", &s[0..4], &s[4..6], &s[6..8], &s[8..10], &s[10..12])
}

/// `sc111A03` 원본 응답 → 도구가 반환할 정제본.
///
/// ⚠️ **`delYn`을 `mine`으로 바꿔 내보내는 이유** — 원본 이름은 "삭제 여부"로 읽히지만
/// 실제로는 정반대다. 아마란스 프론트(`ScheduleApi.js:1912` `editable = r.delYn === 'Y'`)는
/// 이 값으로 **드래그 수정과 삭제 버튼**을 함께 열어주는 쓰기권한 플래그로 쓴다. 삭제된 일정은
/// 애초에 목록 응답에서 빠진다(실측). 실측 상관도 명확하다 — 5~8월 3,219건 중 `Y` 19건은
/// **전부** 본인이 참석자/작성자인 일정이고, `N` 3,200건은 **한 건도** 아니다.
/// `editable`로 이름 붙이지 않은 건 `check_author`가 작성자 본인에게만 수정·삭제를 허용해서,
/// 참석자일 뿐인 `Y` 일정과 어긋나기 때문이다.
pub fn shape_events(raw: &Value) -> Value {
    let list = raw.get("resultList").and_then(|v| v.as_array());
    let events: Vec<Value> = list
        .map(|arr| arr.iter().map(shape_event).collect())
        .unwrap_or_default();
    json!({ "count": events.len(), "events": events })
}

fn shape_event(e: &Value) -> Value {
    let s = |k: &str| e.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let mut o = serde_json::Map::new();
    let mut put = |k: &str, v: Value| {
        o.insert(k.to_string(), v);
    };
    put("schSeq", json!(s("schSeq")));
    put("title", json!(s("schTitle")));
    put("start", json!(to_iso(s("startDate"))));
    put("end", json!(to_iso(s("endDate"))));
    put("allday", json!(s("alldayYn") == "Y"));
    put("calendar", json!(format!("{}({})", s("calTitle"), s("mcalSeq"))));
    put("mine", json!(s("delYn") == "Y"));
    put("createName", json!(s("createName")));
    put("createSeq", json!(s("createSeq")));
    // 값이 있을 때만 — 빈 필드로 응답을 불리지 않는다.
    for (out, src) in [("place", "schPlace"), ("contents", "contents"), ("attendees", "schUserList")] {
        if !s(src).is_empty() {
            put(out, json!(s(src)));
        }
    }
    if let Some(n) = e.get("partCount").and_then(|v| v.as_i64())
        && n > 0
    {
        put("partCount", json!(n));
    }
    // 화상회의는 켜져 있을 때만 싣는다. 등록에서 지정할 수 있는 값이라 조회에서도 보여야
    // 사용자가 반영을 확인할 수 있다(수정이 이 값을 건드리는지 판별하는 근거이기도 하다).
    if s("videoYn") == "Y" {
        put("video", json!(true));
    }
    Value::Object(o)
}

/// 일정 참여자 1명. `schPartEmpList` 항목 하나로 나간다.
///
/// ⚠️ `dept_seq`는 **그 사람의 부서**를 넣는다. 실측 캡처에서는 참여자 3명의 `deptSeq`가 모두 같았는데
/// (셋 다 같은 팀이었다) 그것이 "각자의 부서"인지 "등록자의 부서"인지 **가르지 못했다**.
/// 명부가 각자의 부서를 주므로 그쪽을 택했다 — 서버가 이 값을 어디에 쓰는지는 미확인.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Participant {
    pub emp_seq: String,
    pub dept_seq: String,
    pub name: String,
}

/// 등록 시 확장 필드. 전부 선택이고 **기본값은 확장 이전 동작과 같다**(빈 메모 · 본인만 · 화상회의 없음).
#[derive(Default, Debug)]
pub struct EventExtras {
    /// 비밀메모(`myMemo`). 작성자 본인만 보며, 일정이 삭제되거나 참여자에서 빠지면 사라진다.
    pub my_memo: String,
    /// 본인 외 참여자. 본인은 항상 주최자로 들어가므로 여기 넣지 않아도 된다.
    pub participants: Vec<Participant>,
    /// 화상회의 사용 여부.
    pub video: bool,
}

/// 주최자(본인) + 참석자 → `schPartEmpList`.
///
/// `partType`은 실측으로 갈렸다 — **`M`=주최자(등록자) · `W`=참석자**.
/// 본인이 참석자 목록에 또 들어와도 **중복되지 않는다**(주최자 자리를 유지한다).
fn build_part_emp_list(comp_seq: &str, host: &Participant, guests: &[Participant]) -> Vec<Value> {
    let item = |p: &Participant, part_type: &str| {
        json!({
            "compSeq": comp_seq,
            "deptSeq": p.dept_seq,
            "orgType": "E",
            "orgSeq": p.emp_seq,
            "empSeq": p.emp_seq,
            "empName": p.name,
            "partType": part_type,
            "mcalSeq": ""
        })
    };
    let mut out = vec![item(host, "M")];
    let mut seen = vec![host.emp_seq.clone()];
    for g in guests {
        if seen.contains(&g.emp_seq) {
            continue;
        }
        seen.push(g.emp_seq.clone());
        out.push(item(g, "W"));
    }
    out
}

/// 일정 등록/수정 — `sc111A05`. `sch_seq` 빈문자열이면 신규(insert), 채우면 수정(update).
/// 반환 `resultData.schSeq`(=schmSeq)가 생성/수정된 일정 ID.
///
/// ⚠️ **`mailSend`는 `"N"` 고정이다.** 폼 실측값은 `"Y"`지만, 그러면 참여자에게 실제 메일이 나간다
/// — 참여자를 지정할 수 있게 된 이상 이 값은 설정이 아니라 **외부로 나가는 행위의 스위치**다.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_event(
    c: &GwClient,
    sch_seq: &str,
    mcal_seq: &str,
    cal_type: &str,
    title: &str,
    start: &str,
    end: &str,
    contents: &str,
    allday: &str,
    extras: &EventExtras,
) -> Result<Value> {
    let emp = c.emp_seq();
    let host = Participant {
        emp_seq: emp.clone(),
        dept_seq: c.dept_seq(),
        name: c.emp_name(),
    };
    c.call(
        "/schres/sc111A05",
        &json!({
            "companyInfo": c.company_info(),
            "schSeq": sch_seq,
            "schmSeq": sch_seq,
            "schGbnCode": "10",
            "schTitle": title,
            "mcalSeq": mcal_seq,
            "calType": cal_type,
            "startDate": start,
            "endDate": end,
            "gbnCode": "E",
            "repeatType": "10",
            "repeatByDay": "",
            "repeatEndDay": "",
            "rangeCode": "N",
            "alarm_yn": "Y",
            "schAlarmList": [],
            "contents": contents,
            "myMemo": extras.my_memo,
            "alldayYn": allday,
            "lunarYn": "N",
            "inviterPartType": "M",
            "schPartEmpList": build_part_emp_list(&c.comp_seq(), &host, &extras.participants),
            "schUserList": [],
            "addressUserList": [],
            "resList": [],
            "reservedList": [],
            "uidList": "",
            "placeMapData": "{}",
            "otherLinkList": [],
            "groupSeq": c.group_seq(),
            "empSeq": emp,
            "videoYn": if extras.video { "Y" } else { "N" },
            "videoTimeZone": "Asia/Seoul",
            "mailSend": "N",
            "langCode": "kr"
        }),
    )
    .await
}

/// 변경분 → `itemList` 항목 배열. `update_event_items`의 item 형식 지식이 여기 모여 있다.
///
/// ⚠️ **item명과 값 필드명이 항상 같지는 않다**(실측):
/// 제목은 `{item:"schTitle", schTitle:…}`이지만 **내용은 `{item:"schContents", contents:…}`** 다.
/// 시간은 하나라도 바뀌면 `schDate` 항목 하나로 묶어 보내며, 지정하지 않은 쪽은 기존값을 채운다
/// (서버가 부분 갱신을 받지 않는다).
fn build_update_items(
    title: Option<&str>,
    contents: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    orig_allday: &str,
    orig_start: &str,
    orig_end: &str,
) -> Vec<Value> {
    let mut items = Vec::new();
    if let Some(t) = title {
        items.push(json!({ "item": "schTitle", "schTitle": t }));
    }
    if let Some(c) = contents {
        // ⚠️ 내용 item은 item명(schContents)과 값 필드명(contents)이 다름(실측).
        items.push(json!({ "item": "schContents", "contents": c }));
    }
    if start.is_some() || end.is_some() {
        let allday = if orig_allday.is_empty() { "N" } else { orig_allday };
        items.push(json!({
            "item": "schDate",
            "schDate": {
                "startDate": start.unwrap_or(orig_start),
                "endDate": end.unwrap_or(orig_end),
                "allDay": allday,
                "lunar": "N",
                "lunarDate": ""
            }
        }));
    }
    items
}

/// 원본 `videoYn`을 수정 payload에 넣을 값으로 정규화. 값이 없으면 `"N"`.
fn normalize_video_yn(orig: &str) -> &str {
    if orig.is_empty() { "N" } else { orig }
}

/// 수정 시 **변경 여부와 무관하게 항상 주입되는** item 3개. 폼이 그렇게 보내기 때문이다
/// (`docs/api-reference.md` — "폼이 항상 포함하는 item").
///
/// ⚠️ **`videoYn`에 상수 `"N"`을 넣으면 안 된다.** 그랬더니 제목만 고쳐도 화상회의가 꺼지는 것을
/// 실측했다(2026-08-07, `schSeq 90578`: 수정 전 `videoYn:"Y"` → 수정 후 꺼짐). 폼은 화면의
/// **현재 값**을 담아 보내는데 우리는 상수를 보내고 있었다. 원본 값을 되돌려 넣어야 한다.
///
/// 참여자(`updateSchPartEmpList`)에 본인만 담는 것은 **문제가 아니다** — 실측에서 참여자 3명짜리
/// 일정의 제목만 고쳐도 `partCount`가 3으로 유지됐다(`schSeq 90577`). 이 item은 "이 목록으로 교체"가
/// 아니라 "이 사람들의 속성만 갱신"이다(제거는 `removeSchPartEmpList`가 따로 맡는다).
fn always_present_items(
    comp_seq: &str,
    dept_seq: &str,
    emp_seq: &str,
    emp_name: &str,
    mcal_seq: &str,
    video_yn: &str,
) -> Vec<Value> {
    vec![
        json!({ "item": "videoYn", "videoYn": video_yn }),
        json!({
            "item": "schParticipants",
            "addSchPartEmpList": [],
            "updateSchPartEmpList": [{
                "compSeq": comp_seq,
                "deptSeq": dept_seq,
                "orgType": "E",
                "orgSeq": emp_seq,
                "empSeq": emp_seq,
                "empName": emp_name,
                "partType": "M",
                "mcalSeq": mcal_seq
            }],
            "removeSchPartEmpList": []
        }),
        json!({ "item": "mailSend", "mailSend": "N" }),
    ]
}

/// 일정 수정 — `sc111A05` + `rangeCode:"UO"` + `itemList`(변경분 diff). schSeq 유지(in-place, 재발급 없음).
/// `items`=변경 항목 배열. item 형식(실측): schTitle `{schTitle}`, schContents `{contents}`,
/// schDate `{schDate:{startDate,endDate,allDay,lunar,lunarDate}}`. videoYn/schParticipants/mailSend는
/// 폼이 항상 포함하므로 기본 주입.
pub async fn update_event_items(
    c: &GwClient,
    sch_seq: &str,
    mcal_seq: &str,
    items: Vec<Value>,
    orig_video_yn: &str,
) -> Result<Value> {
    let emp = c.emp_seq();
    let video_yn = normalize_video_yn(orig_video_yn);
    let mut item_list = always_present_items(
        &c.comp_seq(), &c.dept_seq(), &emp, &c.emp_name(), mcal_seq, video_yn,
    );
    item_list.extend(items);
    c.call(
        "/schres/sc111A05",
        &json!({
            "companyInfo": c.company_info(),
            "schSeq": sch_seq,
            "schmSeq": sch_seq,
            "schGbnCode": "10",
            "rangeCode": "UO",
            "itemList": item_list,
            "groupSeq": c.group_seq(),
            "empSeq": emp,
            "alarmOnModify": false,
            "repeatByDay": "",
            "repeatEndDay": "",
            "repeatType": "10",
            "alarm_yn": "N",
            // item과 같은 값으로 맞춘다 — 둘 중 어느 쪽이 서버에 먹는지 가르지 않았으므로
            // 한쪽만 고치면 결과가 갈릴 수 있다.
            "videoYn": video_yn,
            "videoTimeZone": "Asia/Seoul",
            "mailSend": "N",
            "langCode": "kr"
        }),
    )
    .await
}

/// 일정 삭제(소프트, 30일 휴지통) — `sc111A06`. ⚠️ 이 API는 `companyInfo` 없이 호출.
pub async fn delete_event(c: &GwClient, mcal_seq: &str, sch_seq: &str, range_code: &str) -> Result<Value> {
    c.call(
        "/schres/sc111A06",
        &json!({
            "mcalSeq": mcal_seq,
            "schmSeq": sch_seq,
            "schSeq": sch_seq,
            "rangeCode": range_code,
            "langCode": "kr"
        }),
    )
    .await
}

/// 참여자 지정(`이름` 또는 `empSeq`) → `Participant`.
///
/// 숫자만으로 된 값은 **empSeq로 직행**하고, 그 외는 명부에서 이름을 찾는다.
/// **못 찾거나 동명이인이면 실패시킨다** — 아무나 골라 남의 일정에 넣는 것보다 낫다.
/// (`resolve_target`이 캘린더 이름에 대해 취한 태도와 같다.)
pub async fn resolve_participants(c: &std::sync::Arc<GwClient>, specs: &[String]) -> Result<Vec<Participant>> {
    let mut out = Vec::new();
    for spec in specs {
        let q = spec.trim();
        if q.is_empty() {
            continue;
        }
        // empSeq 직접 지정 — 명부를 뒤질 필요가 없다.
        if q.chars().all(|ch| ch.is_ascii_digit()) {
            let people = crate::modules::org::roster(c).await?;
            let hit = people
                .iter()
                .find(|m| m.get("empSeq").and_then(|v| v.as_str()) == Some(q));
            let g = |k: &str| {
                hit.and_then(|m| m.get(k).and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string()
            };
            out.push(Participant {
                emp_seq: q.to_string(),
                dept_seq: g("deptId"),
                name: g("name"),
            });
            continue;
        }
        let people = crate::modules::org::roster(c).await?;
        let ql = q.to_lowercase();
        let exact: Vec<&Value> = people
            .iter()
            .filter(|m| {
                m.get("name").and_then(|v| v.as_str()).map(str::to_lowercase) == Some(ql.clone())
            })
            .collect();
        let hits = if exact.is_empty() {
            people
                .iter()
                .filter(|m| {
                    m.get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_lowercase()
                        .contains(&ql)
                })
                .collect::<Vec<&Value>>()
        } else {
            exact
        };
        let g = |m: &Value, k: &str| m.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        match hits.len() {
            0 => {
                return Err(InvalidInput::new(format!(
                    "참여자 '{q}'를 찾지 못했습니다. find_person으로 확인 후 이름이나 empSeq를 지정하세요."
                ))
                .into())
            }
            1 => out.push(Participant {
                emp_seq: g(hits[0], "empSeq"),
                dept_seq: g(hits[0], "deptId"),
                name: g(hits[0], "name"),
            }),
            _ => {
                // 조용히 하나를 고르지 않는다 — 누구를 넣을지는 호출자가 정해야 한다.
                let cands: Vec<String> = hits
                    .iter()
                    .take(10)
                    .map(|m| format!("{}({})", g(m, "name"), g(m, "empSeq")))
                    .collect();
                return Err(InvalidInput::new(format!(
                    "참여자 '{q}'가 {}명입니다. empSeq로 지정하세요: {}",
                    hits.len(),
                    cands.join(", ")
                ))
                .into());
            }
        }
    }
    Ok(out)
}

/// 일정 등록 + **read-back 검증**. 대상 캘린더 해석까지 포함한다.
///
/// read-back으로 판정하는 것: 제목 · 참여자 수(`partCount`) · 화상회의(`videoYn`).
/// **비밀메모는 판정하지 못한다** — 조회 응답에 그 필드가 없다. 응답에 그 사실을 드러낸다.
#[allow(clippy::too_many_arguments)]
pub async fn create_event_and_verify(
    c: &std::sync::Arc<GwClient>,
    calendar: Option<&str>,
    title: &str,
    start: &str,
    end: &str,
    contents: &str,
    allday: &str,
    extras: &EventExtras,
) -> Result<Value> {
    let target = resolve_target_cal(c, calendar).await?;
    let reg = upsert_event(
        c, "", &target.mcal_seq, &target.cal_type, title, start, end, contents, allday, extras,
    )
    .await
    .map_err(|e| anyhow!("일정 등록 실패: {e}"))?;
    let sch_seq = reg
        .get("schSeq")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow!("등록 응답에 schSeq 없음"))?;

    // 기대 참여자 수 = 주최자(본인) + 중복 제거한 참석자. 본인이 목록에 또 있어도 늘지 않는다.
    let me = c.emp_seq();
    let mut uniq: Vec<&str> = vec![me.as_str()];
    for p in &extras.participants {
        if !uniq.contains(&p.emp_seq.as_str()) {
            uniq.push(&p.emp_seq);
        }
    }
    let expected_part = uniq.len() as i64;

    // read-back: 시작일 기준 재조회로 실제 생성 확인
    let day = &start[..start.len().min(8)];
    let ev = find_event(c, &sch_seq, day).await.ok();
    let title_ok = ev
        .as_ref()
        .map(|e| e.get("schTitle").and_then(|v| v.as_str()) == Some(title))
        .unwrap_or(false);
    // partCount는 참여자가 1명(본인)뿐일 때 응답에서 0이거나 빠질 수 있다 — 그 경우는 검사하지 않는다.
    let actual_part = ev.as_ref().and_then(|e| e.get("partCount").and_then(|v| v.as_i64()));
    let part_ok = match actual_part {
        Some(n) => n == expected_part,
        None => expected_part <= 1,
    };
    let actual_video = ev
        .as_ref()
        .and_then(|e| e.get("videoYn").and_then(|v| v.as_str()))
        .unwrap_or("");
    let video_ok = !actual_video.is_empty() && (actual_video == "Y") == extras.video;
    let reflected = title_ok && part_ok && (video_ok || actual_video.is_empty());

    let mut out = json!({
        "ok": reflected,
        "schSeq": sch_seq,
        "title": title,
        "calendar": format!("{}({})", target.title, target.mcal_seq),
        "period": format!("{start}~{end}"),
        "verified_by_readback": reflected
    });
    let o = out.as_object_mut().expect("위에서 만든 객체");
    if !extras.participants.is_empty() {
        o.insert(
            "participants".into(),
            json!(
                extras.participants.iter()
                    .map(|p| json!({ "empSeq": p.emp_seq, "name": p.name }))
                    .collect::<Vec<_>>()
            ),
        );
        o.insert(
            "partCount".into(),
            json!({ "expected": expected_part, "actual": actual_part }),
        );
    }
    if extras.video {
        o.insert("video".into(), json!({ "requested": true, "reflected": video_ok }));
    }
    if !extras.my_memo.is_empty() {
        // 조용한 성공 금지 — 보냈다는 것과 반영됐다는 것을 구분해 드러낸다.
        o.insert(
            "secretMemo".into(),
            json!({
                "sent": true,
                "verified": false,
                "note": "조회 응답에 비밀메모 필드가 없어 반영 여부를 확인할 수 없다(아마란스 웹에서 확인할 것)."
            }),
        );
    }
    Ok(out)
}

/// 일정 수정 + **소유권 가드 + read-back 검증**. 변경분만 지정한다.
/// ⚠️ 자원 예약과 달리 **`schSeq`가 유지된다**(in-place) — read-back도 같은 ID로 한다.
pub async fn update_event_and_verify(
    c: &GwClient,
    sch_seq: &str,
    date: &str,
    title: Option<&str>,
    contents: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
) -> Result<Value> {
    if title.is_none() && contents.is_none() && start.is_none() && end.is_none() {
        return Err(InvalidInput::new(
            "변경할 항목(title/contents/start/end)을 하나 이상 지정하세요.",
        )
        .into());
    }
    let orig = find_event(c, sch_seq, date).await?;
    let g = |k: &str| orig.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    check_author(&orig, &c.emp_seq(), "수정")?;

    let items = build_update_items(
        title, contents, start, end,
        &g("alldayYn"), &g("startDate"), &g("endDate"),
    );
    // 원본의 화상회의 설정을 그대로 넘긴다 — 수정이 그것을 꺼뜨리지 않게.
    update_event_items(c, sch_seq, &g("mcalSeq"), items, &g("videoYn"))
        .await
        .map_err(|e| anyhow!("일정 수정 실패: {e}"))?;

    // read-back: 지정한 필드가 반영됐는지 확인(미지정 필드는 검사 대상 아님).
    let after = find_event(c, sch_seq, date).await.ok();
    let reflected = after
        .as_ref()
        .map(|e| {
            let eq = |k: &str, v: Option<&str>| {
                v.map(|x| e.get(k).and_then(|v| v.as_str()) == Some(x)).unwrap_or(true)
            };
            eq("schTitle", title)
                && eq("contents", contents)
                && eq("startDate", start)
                && eq("endDate", end)
        })
        .unwrap_or(false);
    let ag = |k: &str| {
        after
            .as_ref()
            .and_then(|e| e.get(k).and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string()
    };
    // 손대지 않은 필드가 수정에 휩쓸리지 않았는지도 본다 — 화상회의가 실제로 꺼진 적이 있다.
    let video_kept = g("videoYn").is_empty() || ag("videoYn") == g("videoYn");
    let mut out = json!({
        "ok": reflected && video_kept,
        "schSeq": sch_seq,
        "title": ag("schTitle"),
        "period": format!("{}~{}", ag("startDate"), ag("endDate")),
        "verified_by_readback": reflected
    });
    if !video_kept {
        out.as_object_mut().expect("위에서 만든 객체").insert(
            "videoLost".into(),
            json!({
                "before": g("videoYn"),
                "after": ag("videoYn"),
                "note": "수정이 화상회의 설정을 바꿨다 — 의도한 변경이 아니면 결함이다."
            }),
        );
    }
    Ok(out)
}

/// 일정 삭제 + **소유권 가드 + read-back 검증**.
/// 삭제 성공 판정은 **재조회 실패**(목록에서 사라짐)다.
pub async fn delete_event_and_verify(c: &GwClient, sch_seq: &str, date: &str) -> Result<Value> {
    let orig = find_event(c, sch_seq, date).await?;
    check_author(&orig, &c.emp_seq(), "삭제")?;
    let mcal = orig.get("mcalSeq").and_then(|v| v.as_str()).unwrap_or("");
    delete_event(c, mcal, sch_seq, "")
        .await
        .map_err(|e| anyhow!("일정 삭제 실패: {e}"))?;

    let gone = find_event(c, sch_seq, date).await.is_err();
    Ok(json!({
        "ok": gone,
        "schSeq": sch_seq,
        "deleted": true,
        "verified_by_readback": gone
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cal(seq: &str, title: &str, ctype: &str, owner: &str, insert: bool) -> Calendar {
        Calendar {
            mcal_seq: seq.into(),
            title: title.into(),
            cal_type: ctype.into(),
            owner_emp_seq: owner.into(),
            can_insert: insert,
            color: "#fff".into(),
        }
    }

    fn sample() -> Vec<Calendar> {
        vec![
            cal("1", "개인캘린더.홍길동", "E", "100", true),
            cal("2", "R&D 본부", "M", "999", false),
            cal("3", "전사 공지", "", "999", true), // calType 빈 값 — 보정 대상
        ]
    }

    /// 빈 `calType`을 `"E"`로 채우는 것이 이 변환의 핵심(그대로 넘기면 조회에서 빠진다).
    #[test]
    #[allow(non_snake_case)] // 이름 속 `E` — 대문자를 살려야 뜻이 통하는 표기라 소문자로 풀지 않는다
    fn to_cal_list는_빈_caltype을_E로_보정한다() {
        let list = to_cal_list(&sample());
        assert_eq!(list.len(), 3);
        assert_eq!(list[0]["calType"], "E");
        assert_eq!(list[1]["calType"], "M");
        assert_eq!(list[2]["calType"], "E", "빈 calType은 E로 보정");
        assert_eq!(list[0]["adminYn"], "Y");
        assert_eq!(list[0]["mcalSeq"], "1");
        assert_eq!(list[0]["color"], "#fff");
    }

    #[test]
    fn personal과_find는_본인캘린더와_부분일치를_고른다() {
        let cals = sample();
        assert_eq!(personal(&cals, "100").unwrap().mcal_seq, "1");
        assert!(personal(&cals, "999").is_none(), "타인 개인 캘린더는 고르지 않는다");
        assert_eq!(find(&cals, "R&D").unwrap().mcal_seq, "2"); // 이름 부분일치
        assert_eq!(find(&cals, "2").unwrap().mcal_seq, "2"); // mcalSeq 우선
        assert!(find(&cals, "없는캘린더").is_none());
    }

    /// 에러 **문구**(사용자 노출)와 **분류**(invalid_params 여부)를 함께 못박는다.
    #[test]
    fn resolve_target은_기본값과_권한없음을_구분한다() {
        let cals = sample();
        assert_eq!(resolve_target(&cals, None, "100").unwrap().mcal_seq, "1");
        assert_eq!(resolve_target(&cals, Some("전사"), "100").unwrap().mcal_seq, "3");

        // 등록 권한 없는 캘린더 지정 → 호출자 잘못 + 등록 가능 목록 안내
        let err = resolve_target(&cals, Some("R&D"), "100").unwrap_err();
        assert!(err.downcast_ref::<InvalidInput>().is_some());
        let msg = err.to_string();
        assert!(msg.contains("'R&D 본부' 캘린더는 일정 등록 권한이 없습니다"));
        assert!(msg.contains("개인캘린더.홍길동(1)"));
        assert!(!msg.contains("R&D 본부(2)"), "등록 불가 캘린더는 안내 목록에서 빠진다");

        // 없는 키 → 호출자 잘못
        let err = resolve_target(&cals, Some("zzz"), "100").unwrap_err();
        assert!(err.downcast_ref::<InvalidInput>().is_some());
        assert!(err.to_string().contains("'zzz' 에 해당하는 캘린더 없음"));

        // 본인 개인 캘린더 부재 → 서버 상태 이상(InvalidInput 아님 → internal_error)
        let err = resolve_target(&cals, None, "12345").unwrap_err();
        assert!(err.downcast_ref::<InvalidInput>().is_none());
        assert!(err.to_string().contains("본인 개인 캘린더를 찾지 못했습니다"));
    }

    /// ⚠️ item명과 값 필드명이 다른 지점(실측)을 못박는다.
    #[test]
    fn build_update_items는_실측_필드명을_쓴다() {
        let items = build_update_items(Some("새 제목"), Some("새 내용"), None, None, "N", "", "");
        assert!(items.iter().any(|i| i["item"] == "schTitle" && i["schTitle"] == "새 제목"));
        // 내용: item명은 schContents, 값 필드명은 contents
        assert!(items.iter().any(|i| i["item"] == "schContents" && i["contents"] == "새 내용"));
        assert!(!items.iter().any(|i| i["item"] == "schDate"), "시간 미지정 → schDate 없음");
    }

    #[test]
    fn build_update_items는_시간변경시_미지정쪽을_기존값으로_채운다() {
        let items = build_update_items(
            None, None, Some("202608051100"), None, "", "202608050900", "202608051000",
        );
        assert_eq!(items.len(), 1, "제목/내용 미지정 → schDate 하나만");
        let d = &items[0]["schDate"];
        assert_eq!(d["startDate"], "202608051100");
        assert_eq!(d["endDate"], "202608051000", "미지정 → 기존값 유지");
        assert_eq!(d["allDay"], "N", "빈 alldayYn은 N으로 보정");
        assert_eq!(d["lunar"], "N");
    }

    /// 자원의 `check_owner`와 **필드명(createSeq)·호칭(작성자)이 다르다**는 점이 요지.
    #[test]
    #[allow(non_snake_case)] // 이름 속 `delYn` — 대문자를 살려야 뜻이 통하는 표기라 소문자로 풀지 않는다
    fn shape_events는_delYn을_mine으로_바꾸고_시각을_ISO로_돌린다() {
        let raw = json!({"resultList": [
            {"schSeq":"90270","schTitle":"시너지 미팅","startDate":"202608061400","endDate":"202608061500",
             "alldayYn":"N","calTitle":"이노그리드","mcalSeq":"230","delYn":"Y","createName":"정선미",
             "createSeq":"3081","partCount":9,"schPlace":"","contents":"","schUserList":"이연지,정선미",
             "empPicFileId":"https://…","placeMapData":"{}","stickerSeq":""},
            {"schSeq":"89599","schTitle":"석식","startDate":"202608061800","endDate":"202608061900",
             "alldayYn":"Y","calTitle":"이노그리드","mcalSeq":"230","delYn":"N","createName":"권경민",
             "createSeq":"2096","partCount":0},
        ]});
        let out = shape_events(&raw);
        assert_eq!(out["count"], 2);
        let a = &out["events"][0];
        assert_eq!(a["mine"], true);
        assert_eq!(a["start"], "2026-08-06T14:00");
        assert_eq!(a["allday"], false);
        assert_eq!(a["calendar"], "이노그리드(230)");
        assert_eq!(a["attendees"], "이연지,정선미");
        assert_eq!(a["partCount"], 9);
        // 원본 이름과 노이즈 필드는 새어나가지 않는다.
        for k in ["delYn", "schTitle", "startDate", "empPicFileId", "placeMapData", "stickerSeq"] {
            assert!(a.get(k).is_none(), "{k} 가 응답에 남아 있다");
        }
        let b = &out["events"][1];
        assert_eq!(b["mine"], false);
        assert_eq!(b["allday"], true);
        // 빈 값·0은 키 자체를 넣지 않는다.
        for k in ["place", "contents", "attendees", "partCount"] {
            assert!(b.get(k).is_none(), "{k} 가 빈 값으로 들어갔다");
        }
    }

    #[test]
    fn to_iso는_형식이_어긋나면_원본을_돌려준다() {
        assert_eq!(to_iso("202608061400"), "2026-08-06T14:00");
        assert_eq!(to_iso("20260806"), "20260806");
        assert_eq!(to_iso(""), "");
        assert_eq!(to_iso("20260806140x"), "20260806140x");
    }

    #[test]
    fn check_author는_타인과_필드누락을_거부한다() {
        let mine = json!({ "createSeq": "3166", "schTitle": "내 일정" });
        assert!(check_author(&mine, "3166", "수정").is_ok());

        let others = json!({ "createSeq": "12345" });
        let err = check_author(&others, "3166", "수정").unwrap_err();
        assert!(err.downcast_ref::<NotOwner>().is_some(), "NotOwner여야 invalid_params로 매핑된다");
        assert_eq!(
            err.to_string(),
            "본인 작성 일정이 아니라 수정할 수 없습니다 (작성자 empSeq=12345, 본인=3166)"
        );

        // createSeq 자체가 없으면 ""와 비교 → 거부
        assert!(check_author(&json!({}), "3166", "삭제").is_err());
    }

    fn p(seq: &str, name: &str) -> Participant {
        Participant { emp_seq: seq.into(), dept_seq: "2993".into(), name: name.into() }
    }

    #[test]
    fn build_part_emp_list는_참여자가_없으면_본인만_넣는다() {
        // 확장 이전 동작 보존 — 이게 깨지면 기존 사용자의 일정 등록이 달라진다.
        let list = build_part_emp_list("1000", &p("3166", "이재학"), &[]);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["empSeq"], "3166");
        assert_eq!(list[0]["partType"], "M", "본인은 주최자");
        assert_eq!(list[0]["orgSeq"], "3166", "개인은 orgSeq==empSeq");
        assert_eq!(list[0]["orgType"], "E");
    }

    #[test]
    fn build_part_emp_list는_주최자와_참석자를_구분한다() {
        let list = build_part_emp_list("1000", &p("3166", "이재학"), &[p("3131", "송학현"), p("3137", "김종명")]);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0]["partType"], "M");
        assert_eq!(list[1]["partType"], "W", "참석자는 W (실측)");
        assert_eq!(list[2]["partType"], "W");
        assert_eq!(list[1]["empName"], "송학현");
    }

    #[test]
    fn build_part_emp_list는_본인이_참석자에_또_있어도_중복시키지_않는다() {
        let list = build_part_emp_list("1000", &p("3166", "이재학"), &[p("3166", "이재학"), p("3131", "송학현")]);
        assert_eq!(list.len(), 2, "본인은 한 번만");
        assert_eq!(list[0]["partType"], "M", "중복 제거 후에도 주최자 자리를 유지한다");
        assert_eq!(list[1]["empSeq"], "3131");
    }

    #[test]
    fn build_part_emp_list는_참석자_중복도_걸러낸다() {
        let list = build_part_emp_list("1000", &p("3166", "이재학"), &[p("3131", "송학현"), p("3131", "송학현")]);
        assert_eq!(list.len(), 2);
    }

    /// ⛔ 회귀 방지 — 여기에 상수 "N"을 되돌려 넣으면 제목만 고쳐도 화상회의가 꺼진다(실측).
    #[test]
    fn always_present_items는_화상회의를_원본값으로_되돌린다() {
        let on = always_present_items("1000", "2993", "3166", "이재학", "1095", "Y");
        assert_eq!(on[0]["item"], "videoYn");
        assert_eq!(on[0]["videoYn"], "Y", "켜져 있던 일정은 켜진 채로 남아야 한다");

        let off = always_present_items("1000", "2993", "3166", "이재학", "1095", "N");
        assert_eq!(off[0]["videoYn"], "N");
    }

    #[test]
    fn normalize_video_yn은_빈값을_n으로_본다() {
        assert_eq!(normalize_video_yn(""), "N");
        assert_eq!(normalize_video_yn("Y"), "Y");
        assert_eq!(normalize_video_yn("N"), "N");
    }

    /// 참여자 item은 "교체"가 아니라 "속성 갱신"이라 본인만 담아도 남이 지워지지 않는다(실측).
    /// 그 전제가 깨지면 이 테스트가 아니라 **라이브에서** 드러난다 — 여기서는 형태만 고정한다.
    #[test]
    fn always_present_items는_참여자_제거목록을_비워둔다() {
        let items = always_present_items("1000", "2993", "3166", "이재학", "1095", "N");
        assert_eq!(items[1]["item"], "schParticipants");
        assert_eq!(items[1]["removeSchPartEmpList"], json!([]), "제거는 이 경로가 하지 않는다");
        assert_eq!(items[1]["updateSchPartEmpList"][0]["empSeq"], "3166");
        assert_eq!(items[2]["mailSend"], "N", "수정이 알림을 발송하지 않는다");
    }

    #[test]
    fn build_part_emp_list는_각자의_부서를_싣는다() {
        let mut other = p("3131", "송학현");
        other.dept_seq = "3040".into();
        let list = build_part_emp_list("1000", &p("3166", "이재학"), &[other]);
        assert_eq!(list[0]["deptSeq"], "2993");
        assert_eq!(list[1]["deptSeq"], "3040", "참여자는 자기 부서 seq를 쓴다");
    }
}
