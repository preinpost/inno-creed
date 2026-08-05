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
/// `sc111A02`는 TTL당 1회만 나간다. 조회(list_events)와 등록(create_event)이 같은 목록을 쓴다.
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

/// 일정 등록/수정 — `sc111A05`. `sch_seq` 빈문자열이면 신규(insert), 채우면 수정(update).
/// 반환 `resultData.schSeq`(=schmSeq)가 생성/수정된 일정 ID.
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
) -> Result<Value> {
    let emp = c.emp_seq();
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
            "myMemo": "",
            "alldayYn": allday,
            "lunarYn": "N",
            "inviterPartType": "M",
            "schPartEmpList": [{
                "compSeq": c.comp_seq(),
                "deptSeq": c.dept_seq(),
                "orgType": "E",
                "orgSeq": emp,
                "empSeq": emp,
                "empName": c.emp_name(),
                "partType": "M",
                "mcalSeq": ""
            }],
            "schUserList": [],
            "addressUserList": [],
            "resList": [],
            "reservedList": [],
            "uidList": "",
            "placeMapData": "{}",
            "otherLinkList": [],
            "groupSeq": c.group_seq(),
            "empSeq": emp,
            "videoYn": "N",
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

/// 일정 수정 — `sc111A05` + `rangeCode:"UO"` + `itemList`(변경분 diff). schSeq 유지(in-place, 재발급 없음).
/// `items`=변경 항목 배열. item 형식(실측): schTitle `{schTitle}`, schContents `{contents}`,
/// schDate `{schDate:{startDate,endDate,allDay,lunar,lunarDate}}`. videoYn/schParticipants/mailSend는
/// 폼이 항상 포함하므로 기본 주입.
pub async fn update_event_items(
    c: &GwClient,
    sch_seq: &str,
    mcal_seq: &str,
    items: Vec<Value>,
) -> Result<Value> {
    let emp = c.emp_seq();
    let mut item_list = vec![
        json!({ "item": "videoYn", "videoYn": "N" }),
        json!({
            "item": "schParticipants",
            "addSchPartEmpList": [],
            "updateSchPartEmpList": [{
                "compSeq": c.comp_seq(),
                "deptSeq": c.dept_seq(),
                "orgType": "E",
                "orgSeq": emp,
                "empSeq": emp,
                "empName": c.emp_name(),
                "partType": "M",
                "mcalSeq": mcal_seq
            }],
            "removeSchPartEmpList": []
        }),
        json!({ "item": "mailSend", "mailSend": "N" }),
    ];
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
            "videoYn": "N",
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

/// 일정 등록 + **read-back 검증**. 대상 캘린더 해석까지 포함한다.
#[allow(clippy::too_many_arguments)]
pub async fn create_event_and_verify(
    c: &GwClient,
    calendar: Option<&str>,
    title: &str,
    start: &str,
    end: &str,
    contents: &str,
    allday: &str,
) -> Result<Value> {
    let target = resolve_target_cal(c, calendar).await?;
    let reg = upsert_event(
        c, "", &target.mcal_seq, &target.cal_type, title, start, end, contents, allday,
    )
    .await
    .map_err(|e| anyhow!("일정 등록 실패: {e}"))?;
    let sch_seq = reg
        .get("schSeq")
        .and_then(|v| v.as_str())
        .map(String::from)
        .ok_or_else(|| anyhow!("등록 응답에 schSeq 없음"))?;

    // read-back: 시작일 기준 재조회로 실제 생성 확인
    let day = &start[..start.len().min(8)];
    let ev = find_event(c, &sch_seq, day).await.ok();
    let reflected = ev
        .as_ref()
        .map(|e| e.get("schTitle").and_then(|v| v.as_str()) == Some(title))
        .unwrap_or(false);
    Ok(json!({
        "ok": reflected,
        "schSeq": sch_seq,
        "title": title,
        "calendar": format!("{}({})", target.title, target.mcal_seq),
        "period": format!("{start}~{end}"),
        "verified_by_readback": reflected
    }))
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
    update_event_items(c, sch_seq, &g("mcalSeq"), items)
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
    Ok(json!({
        "ok": reflected,
        "schSeq": sch_seq,
        "title": ag("schTitle"),
        "period": format!("{}~{}", ag("startDate"), ag("endDate")),
        "verified_by_readback": reflected
    }))
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
}
