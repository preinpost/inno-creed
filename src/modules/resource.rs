//! 자원(회의실 예약) 모듈 — `/schres/rs121*`

use anyhow::Result;
use serde_json::{json, Value};

use crate::client::GwClient;

/// 자원(회의실) 목록 — `rs121A01`
pub async fn list_resources(c: &GwClient) -> Result<Value> {
    c.call(
        "/schres/rs121A01",
        &json!({
            "companyInfo": c.company_info(),
            "searchText": "",
            "attrUseYn": "",
            "attrList": ["1", "3", "ETC"],
            "propList": [],
            "langCode": "kr"
        }),
    )
    .await
}

/// 본인 사용자(예약자) subscriber 객체
fn subscriber_self(c: &GwClient) -> Value {
    json!({
        "groupSeq": c.group_seq(),
        "compSeq": c.comp_seq(),
        "deptSeq": c.dept_seq(),
        "empSeq": c.emp_seq()
    })
}

/// 예약 등록 — `rs121A06`. 날짜는 `YYYYMMDDHHmm`. 반환에 `seqNum`/`resIdx`.
pub async fn create_reservation(
    c: &GwClient,
    res_seq: &str,
    req_text: &str,
    start: &str,
    end: &str,
    desc: &str,
) -> Result<Value> {
    c.call(
        "/schres/rs121A06",
        &json!({
            "companyInfo": c.company_info(),
            "resSeq": res_seq,
            "reqText": req_text,
            "apprYn": "N",
            "alldayYn": "N",
            "startDate": start,
            "endDate": end,
            "descText": desc,
            "resSubscriberList": [subscriber_self(c)],
            "uidList": "",
            "repeatType": "10",
            "repeatEndDay": "",
            "langCode": "kr"
        }),
    )
    .await
}

/// 예약 수정 — `rs121A12`. `start_date_pk`/`create_date_pk`는 원본 식별키(변경 전 값).
#[allow(clippy::too_many_arguments)]
pub async fn update_reservation(
    c: &GwClient,
    res_seq: &str,
    seq_num: i64,
    res_idx: &str,
    req_text: &str,
    start: &str,
    end: &str,
    desc: &str,
    start_date_pk: &str,
    create_date_pk: &str,
    res_name: &str,
) -> Result<Value> {
    c.call(
        "/schres/rs121A12",
        &json!({
            "companyInfo": c.company_info(),
            "resSeq": res_seq,
            "seqNum": seq_num,
            "resIdx": res_idx,
            "reqText": req_text,
            "apprYn": "N",
            "alldayYn": "N",
            "startDatePk": start_date_pk,
            "createDatePk": create_date_pk,
            "startDate": start,
            "endDate": end,
            "descText": desc,
            "resSubscriberList": [subscriber_self(c)],
            "uidList": "",
            "repeatType": "10",
            "repeatEndDay": "",
            "repeatByDay": "",
            "resName": res_name,
            "langCode": "kr"
        }),
    )
    .await
}

/// 예약 상세 조회 — `rs121A10`. (소유권 확인·삭제 전 스냅샷 확보용)
pub async fn get_reservation(
    c: &GwClient,
    res_seq: &str,
    seq_num: i64,
    res_idx: &str,
) -> Result<Value> {
    c.call(
        "/schres/rs121A10",
        &json!({
            "companyInfo": c.company_info(),
            "resSeq": res_seq,
            "seqNum": seq_num,
            "resIdx": res_idx,
            "langCode": "kr"
        }),
    )
    .await
}

/// 예약 삭제(휴지통) — `rs121A11`. 상세(get_reservation) 스냅샷 필드가 필요.
#[allow(clippy::too_many_arguments)]
pub async fn delete_reservation(
    c: &GwClient,
    res_seq: &str,
    seq_num: i64,
    res_idx: &str,
    req_text: &str,
    start: &str,
    end: &str,
    create_date: &str,
    res_name: &str,
) -> Result<Value> {
    c.call(
        "/schres/rs121A11",
        &json!({
            "companyInfo": c.company_info(),
            "statusCode": "CA",
            "deleteRangeCode": "UO",
            "resSeqList": [{
                "resSeq": res_seq,
                "seqNum": seq_num,
                "resIdx": res_idx,
                "reqText": req_text,
                "startDate": start,
                "endDate": end,
                "createDate": create_date,
                "schmSeq": "",
                "schSeq": "",
                "resName": res_name,
                "alldayYn": "N"
            }],
            "langCode": "kr"
        }),
    )
    .await
}

// ─────────────────────────────────────────────────────────────────────────────
// 파생 조회: 자원 그룹 필터 · 슬림 정규화 · 빈 시간 계산 · 내 예약
// ─────────────────────────────────────────────────────────────────────────────

/// 건물(자원종류) 그룹 → `attrSeq`. 실측: `1`=회의실(본사), `3`=인재INC 구로 오피스 회의실.
/// 빈 문자열/"전체"면 필터 없음. 숫자를 직접 줘도 된다.
fn attr_filter(group: &str) -> Option<&str> {
    match group.trim() {
        "" | "전체" | "all" => None,
        "본사" | "hq" => Some("1"),
        "구로" | "guro" => Some("3"),
        other => Some(other),
    }
}

/// 그룹에 해당하는 자원 목록(resSeq, resName, attrSeq).
pub async fn resources_in_group(c: &GwClient, group: &str) -> Result<Vec<Value>> {
    let data = list_resources(c).await?;
    let want = attr_filter(group);
    Ok(data
        .get("resultList")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|r| match want {
                    None => true,
                    Some(a) => r.get("attrSeq").and_then(|v| v.as_str()) == Some(a),
                })
                .cloned()
                .collect()
        })
        .unwrap_or_default())
}

/// `YYYYMMDDHHmm` → `YYYY-MM-DDTHH:mm`. 형식이 다르면 원문 그대로.
fn iso(ts: &str) -> String {
    if ts.len() == 12 && ts.chars().all(|c| c.is_ascii_digit()) {
        format!(
            "{}-{}-{}T{}:{}",
            &ts[0..4],
            &ts[4..6],
            &ts[6..8],
            &ts[8..10],
            &ts[10..12]
        )
    } else {
        ts.to_string()
    }
}

/// `YYYYMMDDHHmm` → 자정 기준 분. 다른 날짜면 `day_ymd` 대비로 ±1440씩 보정.
fn minutes_of(ts: &str, day_ymd: &str) -> Option<i64> {
    if ts.len() != 12 {
        return None;
    }
    let hh: i64 = ts[8..10].parse().ok()?;
    let mm: i64 = ts[10..12].parse().ok()?;
    let same_day = &ts[0..8] == day_ymd;
    let base = hh * 60 + mm;
    Some(if same_day {
        base
    } else if &ts[0..8] < day_ymd {
        i64::MIN / 4 // 전날 이전 시작 → 사실상 -무한(하루 전체 점유)
    } else {
        i64::MAX / 4 // 다음날 이후 종료 → +무한
    })
}

/// `"0900-1200"` / `"09:00-12:00"` → (시작분, 종료분).
fn parse_window(w: &str) -> Result<(i64, i64)> {
    let cleaned: String = w.chars().filter(|c| c.is_ascii_digit() || *c == '-').collect();
    let (a, b) = cleaned
        .split_once('-')
        .ok_or_else(|| anyhow::anyhow!("window 형식 오류: '{w}' (예: \"0900-1200\")"))?;
    let to_min = |s: &str| -> Result<i64> {
        if s.len() != 4 {
            anyhow::bail!("window 시각은 HHmm 4자리여야 함: '{s}'");
        }
        let h: i64 = s[0..2].parse()?;
        let m: i64 = s[2..4].parse()?;
        Ok(h * 60 + m)
    };
    Ok((to_min(a)?, to_min(b)?))
}

fn hhmm(min: i64) -> String {
    format!("{:02}:{:02}", min / 60, min % 60)
}

/// 예약 1건을 조회용 슬림 형태로. 원본은 74필드에 회의 안건 전문(`descText`)까지 실려 온다.
pub fn slim_reservation(r: &Value) -> Value {
    let s = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    json!({
        "resSeq": s("resSeq"),
        "resName": s("resName"),
        "seqNum": r.get("seqNum").cloned().unwrap_or(Value::Null),
        "resIdx": r.get("resIdx").cloned().unwrap_or(Value::Null),
        "start": iso(&s("resStartDate")),
        "end": iso(&s("resEndDate")),
        "title": s("reqText"),
        "owner": s("empName"),
        "ownerEmpSeq": s("empSeq"),
        "attendees": s("resUserName"),
        "allDay": s("alldayYn") == "Y"
    })
}

/// 빈 시간 찾기. `date`=YYYYMMDD, `duration_min`=필요 시간(분), `window`=탐색 구간(HHmm-HHmm),
/// `group`=""|"본사"|"구로". 자원별로 예약을 빼고 `duration_min` 이상인 구간만 남긴다.
/// 종일·다일 예약(예: 반년짜리 공용좌석)은 해당일 전체 점유로 처리한다.
pub async fn find_free_slots(
    c: &GwClient,
    date: &str,
    duration_min: i64,
    window: &str,
    group: &str,
) -> Result<Value> {
    let (win_start, win_end) = parse_window(if window.trim().is_empty() {
        "0900-1800"
    } else {
        window
    })?;
    if duration_min <= 0 {
        anyhow::bail!("duration_min은 1 이상이어야 합니다");
    }

    let rooms = resources_in_group(c, group).await?;
    if rooms.is_empty() {
        anyhow::bail!("group '{group}'에 해당하는 자원이 없습니다 (전체/본사/구로 또는 attrSeq)");
    }
    let seqs: Vec<String> = rooms
        .iter()
        .filter_map(|r| r.get("resSeq").and_then(|v| v.as_str()).map(String::from))
        .collect();
    let refs: Vec<&str> = seqs.iter().map(|s| s.as_str()).collect();
    let bookings = list_reservations(c, date, date, &refs).await?;
    let rows = bookings
        .get("resultList")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut out: Vec<Value> = Vec::new();
    for room in &rooms {
        let seq = room.get("resSeq").and_then(|v| v.as_str()).unwrap_or("");
        // 이 방의 점유 구간(창 안쪽으로 클립)
        let mut busy: Vec<(i64, i64)> = rows
            .iter()
            .filter(|b| b.get("resSeq").and_then(|v| v.as_str()) == Some(seq))
            .filter_map(|b| {
                let st = b.get("resStartDate").and_then(|v| v.as_str())?;
                let en = b.get("resEndDate").and_then(|v| v.as_str())?;
                let a = minutes_of(st, date)?.max(win_start);
                let z = minutes_of(en, date)?.min(win_end);
                (z > a).then_some((a, z))
            })
            .collect();
        busy.sort();

        // 겹침 병합 후 빈 구간 산출
        let mut merged: Vec<(i64, i64)> = Vec::new();
        for (a, z) in busy {
            match merged.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(z),
                _ => merged.push((a, z)),
            }
        }
        let mut free: Vec<Value> = Vec::new();
        let mut cursor = win_start;
        for (a, z) in merged.iter().chain(std::iter::once(&(win_end, win_end))) {
            if a - cursor >= duration_min {
                free.push(json!({
                    "from": hhmm(cursor), "to": hhmm(*a), "minutes": a - cursor
                }));
            }
            cursor = cursor.max(*z);
        }
        if !free.is_empty() {
            out.push(json!({
                "resSeq": seq,
                "resName": room.get("resName").cloned().unwrap_or(Value::Null),
                "attrName": room.get("attrName").cloned().unwrap_or(Value::Null),
                "freeSlots": free
            }));
        }
    }

    // 가장 빠른 시작 시각 기준 정렬 → 첫 항목이 곧 추천
    out.sort_by_key(|r| {
        r.get("freeSlots")
            .and_then(|f| f.as_array())
            .and_then(|a| a.first())
            .and_then(|s| s.get("from"))
            .and_then(|v| v.as_str())
            .unwrap_or("99:99")
            .to_string()
    });

    Ok(json!({
        "kind": "freeSlots",
        "date": date,
        "window": format!("{}-{}", hhmm(win_start), hhmm(win_end)),
        "durationMin": duration_min,
        "group": if group.trim().is_empty() { "전체" } else { group },
        "roomsChecked": rooms.len(),
        "roomsWithSlot": out.len(),
        "rooms": out
    }))
}

/// 내 예약만. 수정·취소에 필요한 `seqNum`/`resIdx`를 얻는 정규 경로.
/// 기간 `start`~`end`는 YYYYMMDD.
pub async fn my_reservations(c: &GwClient, start: &str, end: &str) -> Result<Value> {
    let rooms = resources_in_group(c, "").await?;
    let seqs: Vec<String> = rooms
        .iter()
        .filter_map(|r| r.get("resSeq").and_then(|v| v.as_str()).map(String::from))
        .collect();
    let refs: Vec<&str> = seqs.iter().map(|s| s.as_str()).collect();
    let data = list_reservations(c, start, end, &refs).await?;
    let me = c.emp_seq();
    let mine: Vec<Value> = data
        .get("resultList")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter(|b| b.get("empSeq").and_then(|v| v.as_str()) == Some(me.as_str()))
                .map(slim_reservation)
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "kind": "myReservations",
        "period": format!("{start}~{end}"),
        "empSeq": me,
        "count": mine.len(),
        "reservations": mine
    }))
}

/// 예약 조회 — `rs121A05`. `res_seqs`=조회할 자원 ID들, 기간 YYYYMMDD.
pub async fn list_reservations(
    c: &GwClient,
    start: &str,
    end: &str,
    res_seqs: &[&str],
) -> Result<Value> {
    let res_list: Vec<Value> = res_seqs.iter().map(|s| json!({ "resSeq": s })).collect();
    c.call(
        "/schres/rs121A05",
        &json!({
            "companyInfo": c.company_info(),
            "startDate": start,
            "endDate": end,
            "statusType": ["10", "20"],
            "resList": res_list,
            "statusCode": "",
            "searchType": "",
            "sechType": "",
            "menuAuth": "USER",
            "langCode": "kr"
        }),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso는_12자리만_변환하고_나머지는_원문이다() {
        assert_eq!(iso("202608051030"), "2026-08-05T10:30");
        assert_eq!(iso("2026080510"), "2026080510"); // 12자리 아님 → 원문
        assert_eq!(iso(""), "");
        assert_eq!(iso("20260805103a"), "20260805103a"); // 숫자 아님 → 원문
    }

    /// 다일(多日) 예약은 센티넬로 "하루 전체 점유"를 표현한다 — 빈 시간 계산의 핵심 경계.
    #[test]
    fn minutes_of는_다른_날짜를_무한대_센티넬로_바꾼다() {
        assert_eq!(minutes_of("202608051030", "20260805"), Some(630));
        assert_eq!(minutes_of("202608050000", "20260805"), Some(0));
        assert_eq!(minutes_of("202608052359", "20260805"), Some(1439));
        // 전날 시작 → 사실상 -무한
        assert!(minutes_of("202608041800", "20260805").unwrap() < 0);
        // 다음날 종료 → +무한
        assert!(minutes_of("202608061000", "20260805").unwrap() > 24 * 60);
        assert_eq!(minutes_of("2026080510", "20260805"), None); // 12자리 아님
    }

    #[test]
    fn parse_window는_두_형식을_받는다() {
        assert_eq!(parse_window("0900-1200").unwrap(), (540, 720));
        assert_eq!(parse_window("09:00-12:00").unwrap(), (540, 720)); // 콜론은 걸러짐
        assert_eq!(parse_window("0000-2359").unwrap(), (0, 1439));
        assert!(parse_window("0900").is_err());      // 구분자 없음
        assert!(parse_window("900-1200").is_err());  // HHmm 4자리 아님
        assert!(parse_window("").is_err());
    }

    #[test]
    fn hhmm은_분을_시각으로_되돌린다() {
        assert_eq!(hhmm(0), "00:00");
        assert_eq!(hhmm(540), "09:00");
        assert_eq!(hhmm(1439), "23:59");
    }

    /// 그룹 별칭 → attrSeq. 모르는 값은 숫자로 직접 준 것으로 보고 통과시킨다.
    #[test]
    fn attr_filter는_별칭과_직접입력을_모두_받는다() {
        assert_eq!(attr_filter(""), None);
        assert_eq!(attr_filter("전체"), None);
        assert_eq!(attr_filter("all"), None);
        assert_eq!(attr_filter(" 본사 "), Some("1")); // 공백 제거
        assert_eq!(attr_filter("hq"), Some("1"));
        assert_eq!(attr_filter("구로"), Some("3"));
        assert_eq!(attr_filter("7"), Some("7"));
    }

    /// 74필드 원본 → 슬림 형태. 후속 조회에 필요한 seqNum/resIdx가 반드시 남아야 한다.
    #[test]
    fn slim_reservation은_필요한_필드만_남긴다() {
        let raw = json!({
            "resSeq": "12", "resName": "대회의실", "seqNum": 3456, "resIdx": 1,
            "resStartDate": "202608051000", "resEndDate": "202608051100",
            "reqText": "주간회의", "empName": "이재학", "empSeq": "3166",
            "resUserName": "이재학,정선미", "alldayYn": "N",
            "descText": "본문 전문(버려져야 함)"
        });
        let v = slim_reservation(&raw);
        assert_eq!(v["seqNum"], 3456);          // 숫자 원형 유지(수정·취소에 필요)
        assert_eq!(v["resIdx"], 1);
        assert_eq!(v["start"], "2026-08-05T10:00");
        assert_eq!(v["end"], "2026-08-05T11:00");
        assert_eq!(v["title"], "주간회의");
        assert_eq!(v["ownerEmpSeq"], "3166");   // 소유권 가드의 기준값
        assert_eq!(v["allDay"], false);
        assert!(v.get("descText").is_none(), "본문 전문은 슬림 형태에 남지 않아야 한다");
    }

    #[test]
    fn slim_reservation은_없는_필드를_빈값으로_채운다() {
        let v = slim_reservation(&json!({}));
        assert_eq!(v["resSeq"], "");
        assert_eq!(v["seqNum"], Value::Null);
        assert_eq!(v["allDay"], false);
    }
}
