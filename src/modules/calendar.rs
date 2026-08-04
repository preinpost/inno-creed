//! 일정(캘린더) 모듈 — `/schres/sc111*`

use anyhow::Result;
use serde_json::{json, Value};

use crate::client::GwClient;

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
