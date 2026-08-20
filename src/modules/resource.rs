//! 자원(회의실 예약) 모듈 — `/schres/rs121*`
//!
//! ⭐ **이 모듈의 mutation 함수(`*_and_verify`)는 소유권 가드와 read-back 검증을 포함한다**
//! (`docs/architecture.md` §7). 즉 규칙이 MCP 도구 경로가 아니라 여기 있어서,
//! 모듈을 직접 쓰는 어떤 호출자도 검증을 우회할 수 없다. 근태 `punch_and_verify`와 같은 형태.
//! 검증 없는 raw 래퍼(`create_reservation`/`update_reservation`/`delete_reservation`)도 남아 있지만,
//! **새 호출부는 `*_and_verify`를 쓸 것.**

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::error::{InvalidInput, NotOwner};

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

/// 참석자(`resSubscriberList`) 목록을 만든다 — **본인이 항상 첫 항목**이고 뒤에 지정한 사람들이 붙는다.
///
/// `specs`는 **이름 또는 empSeq**(person_group 의 `empSeqs` 를 그대로 넘기면 된다).
/// `deptSeq`는 사람마다 다르므로 명부(`org::roster`, 30분 캐시)에서 각자 값을 채운다 —
/// 예약자 부서를 돌려쓰면 남의 부서가 틀리게 박힌다.
///
/// ⚠️ **타인을 넣는 것은 바깥으로 나가는 행위다**(`analyze/06` 경고). 이 함수는 값만 만들고,
/// 호출 여부 판단은 도구 설명과 사용자 지시에 맡긴다.
///
/// 명부에서 못 찾은 사람은 **조용히 빼지 않고 거부**한다 — 참석자가 소리 없이 빠지면
/// 그 사람만 회의를 모른 채 지나간다.
pub async fn resolve_subscribers(
    c: &std::sync::Arc<GwClient>,
    specs: &[String],
) -> Result<Vec<Value>> {
    let mut out = vec![subscriber_self(c)];
    let cleaned: Vec<&str> = specs.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if cleaned.is_empty() {
        return Ok(out);
    }

    let people = crate::modules::org::roster(c).await?;
    let g = |m: &Value, k: &str| m.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    // 본인은 이미 들어 있다 — 다시 지정해도 중복으로 넣지 않는다.
    let mut seen: std::collections::HashSet<String> = [c.emp_seq()].into_iter().collect();

    for q in cleaned {
        let hit = if q.chars().all(|ch| ch.is_ascii_digit()) {
            people.iter().find(|m| g(m, "empSeq") == q).cloned().ok_or_else(|| {
                InvalidInput::new(format!(
                    "참석자 empSeq '{q}' 를 조직도 명부에서 찾지 못했습니다. find_person 으로 확인하세요."
                ))
            })?
        } else {
            let ql = q.to_lowercase();
            let exact: Vec<&Value> = people.iter().filter(|m| g(m, "name").to_lowercase() == ql).collect();
            let hits = if exact.is_empty() {
                people.iter().filter(|m| g(m, "name").to_lowercase().contains(&ql)).collect::<Vec<_>>()
            } else {
                exact
            };
            match hits.len() {
                0 => {
                    return Err(InvalidInput::new(format!(
                        "참석자 '{q}' 를 찾지 못했습니다. find_person 으로 확인 후 이름이나 empSeq를 지정하세요."
                    ))
                    .into())
                }
                1 => hits[0].clone(),
                _ => {
                    // 조용히 하나를 고르지 않는다 — 엉뚱한 사람을 회의에 넣는 것은 되돌리기 어렵다.
                    let cands: Vec<String> = hits
                        .iter()
                        .take(10)
                        .map(|m| format!("{}({}, {})", g(m, "name"), g(m, "empSeq"), g(m, "deptName")))
                        .collect();
                    return Err(InvalidInput::new(format!(
                        "참석자 '{q}' 가 {}명입니다. empSeq로 지정하세요: {}",
                        hits.len(),
                        cands.join(", ")
                    ))
                    .into());
                }
            }
        };
        let emp_seq = g(&hit, "empSeq");
        if seen.insert(emp_seq.clone()) {
            out.push(json!({
                "groupSeq": c.group_seq(),
                "compSeq": c.comp_seq(),
                // ⚠️ 예약자 부서가 아니라 **그 사람의 부서**다.
                "deptSeq": g(&hit, "deptId"),
                "empSeq": emp_seq
            }));
        }
    }
    Ok(out)
}

/// 상세(`rs121A10`)의 `subscriberList`를 **쓰기 API가 요구하는 4키로 투영**한다.
///
/// 실측(2026-08-20): 상세 응답이 참석자를 그대로 돌려준다 —
/// `[{compSeq, deptName, deptSeq, empName, empSeq, groupSeq, loginId, useYn}]`.
/// 쓰기(`rs121A06`/`A12`)가 받는 것은 그중 `{groupSeq, compSeq, deptSeq, empSeq}` 4개다.
fn project_subscribers(list: &[Value]) -> Vec<Value> {
    let g = |m: &Value, k: &str| m.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    list.iter()
        .filter(|m| !g(m, "empSeq").is_empty())
        .map(|m| json!({
            "groupSeq": g(m, "groupSeq"),
            "compSeq": g(m, "compSeq"),
            "deptSeq": g(m, "deptSeq"),
            "empSeq": g(m, "empSeq")
        }))
        .collect()
}

/// 수정 시 참석자를 **지정하지 않았을 때** 기존 참석자를 그대로 유지한다.
///
/// ⚠️ **이 함수가 없으면 수정이 참석자를 조용히 날린다.** 쓰기 API는 `resSubscriberList`를
/// 통째로 덮어쓰므로, 예전처럼 `[본인]`을 고정으로 보내면 예약명만 고쳐도 참석자 전원이 빠진다
/// (그 사람들에게는 아무 통지도 가지 않는다).
///
/// 상세에서 참석자를 **읽지 못하면 중단**한다 — "참석자 없음"으로 넘기면 위와 같은 사고가 난다.
fn keep_subscribers(detail: &Value) -> Result<Vec<Value>> {
    let Some(list) = detail.get("subscriberList").and_then(|v| v.as_array()) else {
        bail!(
            "예약 상세에서 참석자(subscriberList)를 읽지 못해 기존 참석자를 유지할 수 없다 — \
             그대로 수정하면 참석자가 조용히 빠지므로 중단한다. attendees 인자로 참석자를 \
             명시하면 진행할 수 있다"
        );
    };
    Ok(project_subscribers(list))
}

/// 예약 상세에서 참석자를 사람이 읽을 형태로(이름 목록). read-back 보고용.
fn subscriber_names(detail: &Value) -> Vec<String> {
    detail
        .get("subscriberList")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|m| m.get("empName").and_then(|v| v.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// 예약 등록 — `rs121A06`. 날짜는 `YYYYMMDDHHmm`. 반환에 `seqNum`/`resIdx`.
#[allow(clippy::too_many_arguments)]
pub async fn create_reservation(
    c: &GwClient,
    res_seq: &str,
    req_text: &str,
    start: &str,
    end: &str,
    desc: &str,
    subscribers: &[Value],
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
            "resSubscriberList": subscribers,
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
    subscribers: &[Value],
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
            "resSubscriberList": subscribers,
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

/// 아마란스 화면에 실제로 찍히는 문구 = `[예약자명] 자원명`.
///
/// ⚠️ **예약명(`reqText`)이 아니다.** 자원 HOME 타임라인은 예약명을 아예 표시하지 않고 이 문자열만
/// 보여준다(2026-08-06 웹 실측 — 타 사용자 예약 12건 전부 같은 형식). 그래서 "예약명을 '회의'로 넣었는데
/// 화면엔 '[이재학] 회의실 B'로 나온다 = 잘못 들어갔다"는 오해가 생긴다. 두 값을 같이 반환해 구분시킨다.
///
/// 목록 API(`rs121A05`)는 이 값을 `resTitleDisplay`로 주지만 **상세(`rs121A10`)에는 없어서**
/// 등록·수정 응답에서는 같은 규칙으로 조립한다.
fn display_title(emp_name: &str, res_name: &str) -> String {
    format!("[{emp_name}] {res_name}")
}

/// 예약 1건을 조회용 슬림 형태로. 원본은 74필드에 회의 안건 전문(`descText`)까지 실려 온다.
pub fn slim_reservation(r: &Value) -> Value {
    let s = |k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    // 서버가 준 값을 우선하고, 없을 때만 조립한다(표시 규칙은 서버 소유).
    let display = match s("resTitleDisplay") {
        t if !t.is_empty() => t,
        _ => display_title(&s("empName"), &s("resName")),
    };
    json!({
        "resSeq": s("resSeq"),
        "resName": s("resName"),
        "seqNum": r.get("seqNum").cloned().unwrap_or(Value::Null),
        "resIdx": r.get("resIdx").cloned().unwrap_or(Value::Null),
        "start": iso(&s("resStartDate")),
        "end": iso(&s("resEndDate")),
        "title": s("reqText"),
        "displayTitle": display,
        "owner": s("empName"),
        "ownerEmpSeq": s("empSeq"),
        "attendees": s("resUserName"),
        "allDay": s("alldayYn") == "Y"
    })
}

/// 사내 점심시간(자정 기준 분). **아마란스 서버가 아는 값이 아니라 사내 규칙**이라 여기 상수로 둔다.
/// 서버는 이 시간대 예약을 막지 않으므로, MCP가 ①빈 시간에서 빼고 ②겹치면 경고를 붙인다.
const LUNCH: (i64, i64) = (13 * 60, 14 * 60);
/// 사용자에게 보이는 표기. 응답 문구가 여러 곳에 나오므로 한 곳에서 만든다.
const LUNCH_LABEL: &str = "13:00~14:00";

/// 예약 구간(`YYYYMMDDHHmm`)이 점심시간과 겹치는가.
/// 여러 날에 걸친 예약은 **하루라도** 점심시간을 덮으면 true(그래서 날짜 경과일 계산이 필요하다).
/// 형식이 깨졌으면 판단하지 않는다(false) — 경고는 부가 정보라 파싱 실패로 예약을 막지 않는다.
fn overlaps_lunch(start: &str, end: &str) -> bool {
    let abs = |s: &str| -> Option<i64> {
        (s.len() == 12).then_some(())?;
        let day = crate::util::ymd_to_days(&s[..8])?;
        let h: i64 = s[8..10].parse().ok()?;
        let m: i64 = s[10..12].parse().ok()?;
        Some(day * 1440 + h * 60 + m)
    };
    let (Some(a), Some(z)) = (abs(start), abs(end)) else {
        return false;
    };
    if z <= a {
        return false;
    }
    // 걸친 각 날짜의 점심 구간과 비교. 다일 예약도 있으므로(공용좌석 등) 날짜 수만큼 순회한다.
    let (d0, d1) = (a.div_euclid(1440), (z - 1).div_euclid(1440));
    (d0..=d1).any(|d| a < d * 1440 + LUNCH.1 && z > d * 1440 + LUNCH.0)
}

/// 점심시간이 걸릴 때 응답에 실을 경고. 겹치지 않으면 `None`.
/// **막지 않고 알리기만 한다** — 실제로 점심시간에 회의를 잡는 경우가 있어서, 판단은 사용자 몫이다.
fn lunch_warning(start: &str, end: &str) -> Option<String> {
    overlaps_lunch(start, end).then(|| {
        format!("⚠️ 예약 시간에 점심시간({LUNCH_LABEL})이 포함됩니다. 의도한 것인지 사용자에게 확인하세요.")
    })
}

/// 점유 구간 목록 → `duration_min` 이상인 빈 구간 목록(자정 기준 분).
/// `busy`는 정렬돼 있지 않아도 되고, 탐색 창(`win_start`~`win_end`) 바깥은 클립해 버린다
/// (다일 예약 센티넬이 그대로 들어와도 여기서 창 크기로 잘린다).
/// 네트워크에 의존하지 않는 계산 전부 — `find_free_slots`는 조회 결과를 이 함수에 넘기기만 한다.
///
/// ⚠️ **`duration_min >= 1`을 전제한다.** 그 검증은 유일한 호출부인 `find_free_slots`에 있고
/// (`duration_min <= 0`이면 bail) 이 함수 자신은 막지 않는다. 0 이하를 넘겨도 panic하지는 않지만
/// **길이 0짜리 빈 구간이 결과에 섞인다** — `a - cursor >= duration_min`이 `a == cursor`에서도
/// 참이 되기 때문이다(창이 예약으로 꽉 차 있으면 `[(win_start, win_start), (win_end, win_end)]`가
/// 나온다). 음수도 0과 같게 동작한다(`a - cursor`가 음수가 되는 경우가 없다).
fn free_slots(
    busy: &[(i64, i64)],
    win_start: i64,
    win_end: i64,
    duration_min: i64,
) -> Vec<(i64, i64)> {
    let mut clipped: Vec<(i64, i64)> = busy
        .iter()
        .filter_map(|&(a, z)| {
            let (a, z) = (a.max(win_start), z.min(win_end));
            (z > a).then_some((a, z))
        })
        .collect();
    clipped.sort();

    // 겹침 병합 후 빈 구간 산출
    let mut merged: Vec<(i64, i64)> = Vec::new();
    for (a, z) in clipped {
        match merged.last_mut() {
            Some(last) if a <= last.1 => last.1 = last.1.max(z),
            _ => merged.push((a, z)),
        }
    }
    let mut free: Vec<(i64, i64)> = Vec::new();
    let mut cursor = win_start;
    for (a, z) in merged.iter().chain(std::iter::once(&(win_end, win_end))) {
        if a - cursor >= duration_min {
            free.push((cursor, *a));
        }
        cursor = cursor.max(*z);
    }
    free
}

/// 빈 시간 찾기. `date`=YYYYMMDD, `duration_min`=필요 시간(분), `window`=탐색 구간(HHmm-HHmm),
/// `group`=""|"본사"|"구로". 자원별로 예약을 빼고 `duration_min` 이상인 구간만 남긴다.
/// 종일·다일 예약(예: 반년짜리 공용좌석)은 해당일 전체 점유로 처리한다.
/// **점심시간(13:00~14:00)도 예약과 똑같이 점유로 처리**해 빈 구간에 들어가지 않게 한다
/// (예: 12:00~15:00이 비어도 2시간 요청은 12:00~13:00·14:00~15:00으로 쪼개져 후보에서 빠진다).
/// `include_lunch=true`면 이 제외를 끄고 점심시간도 후보에 넣는다(점심 회의를 잡아야 할 때).
pub async fn find_free_slots(
    c: &GwClient,
    date: &str,
    duration_min: i64,
    window: &str,
    group: &str,
    include_lunch: bool,
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
        // 이 방의 점유 구간
        let mut busy: Vec<(i64, i64)> = rows
            .iter()
            .filter(|b| b.get("resSeq").and_then(|v| v.as_str()) == Some(seq))
            .filter_map(|b| {
                let st = b.get("resStartDate").and_then(|v| v.as_str())?;
                let en = b.get("resEndDate").and_then(|v| v.as_str())?;
                Some((minutes_of(st, date)?, minutes_of(en, date)?))
            })
            .collect();
        // 점심시간을 예약과 동일한 점유로 취급. include_lunch면 건너뛴다.
        if !include_lunch {
            busy.push(LUNCH);
        }

        let free: Vec<Value> = free_slots(&busy, win_start, win_end, duration_min)
            .into_iter()
            .map(|(from, to)| {
                json!({ "from": hhmm(from), "to": hhmm(to), "minutes": to - from })
            })
            .collect();
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
        "lunchBreak": LUNCH_LABEL,
        "lunchExcluded": !include_lunch,
        "note": if include_lunch {
            format!("include_lunch=true — 점심시간({LUNCH_LABEL})도 후보에 포함했습니다.")
        } else {
            format!("점심시간({LUNCH_LABEL})은 빈 구간에서 제외했습니다. 점심시간에도 찾으려면 include_lunch=true.")
        },
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

/// JSON 값을 인덱스 문자열로. **자원 API 전용 특성** — 서버가 `resIdx`를 number("3")로도
/// string("3")으로도 준다(실측). 다른 모듈에서 쓰이지 않으므로 util이 아니라 여기 둔다.
pub(crate) fn json_idx(v: Option<&Value>) -> Option<String> {
    v.map(|x| match x {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    })
}

/// 소유권 가드 — 예약 상세의 `empSeq`가 로그인 사용자와 같은지.
/// ⚠️ `empSeq` 필드가 없으면 `""`와 비교해 **거부**한다(불일치). 이동 전 동작 그대로다.
fn check_owner(detail: &Value, me: &str, action: &'static str) -> Result<()> {
    let owner = detail.get("empSeq").and_then(|v| v.as_str()).unwrap_or("");
    if owner != me {
        return Err(NotOwner {
            relation: "소유",
            kind: "예약",
            action,
            owner: owner.to_string(),
            me: me.to_string(),
        }
        .into());
    }
    Ok(())
}

/// 수정 시 서버(`rs121A12`)에 보낼 전체 필드. 이 API는 변경분만 받지 않고 **전 필드를 요구**해서,
/// 지정되지 않은 항목은 현재 예약값으로 채운다.
#[derive(Debug, PartialEq)]
pub(crate) struct UpdateFields {
    pub req_text: String,
    pub start: String,
    pub end: String,
    pub desc: String,
    /// 원본 식별키(변경 전 시작시각) — 변경 후 값이 아니라 **기존 값**이어야 한다.
    pub orig_start: String,
    pub create_date: String,
    pub res_name: String,
}

/// 현재 예약 상세 + 변경 요청 → 서버로 보낼 전체 필드(미지정 항목은 기존값 유지).
fn merge_update(
    detail: &Value,
    req_text: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    desc: Option<&str>,
) -> UpdateFields {
    let get = |k: &str| detail.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    UpdateFields {
        req_text: req_text.map(String::from).unwrap_or_else(|| get("reqText")),
        start: start.map(String::from).unwrap_or_else(|| get("startDate")),
        end: end.map(String::from).unwrap_or_else(|| get("endDate")),
        desc: desc.map(String::from).unwrap_or_else(|| get("descText")),
        orig_start: get("startDate"),
        create_date: get("createDate"),
        res_name: get("resName"),
    }
}

/// 예약 등록 + **read-back 검증**. 등록 응답의 successTf를 믿지 않고 재조회로 확인한다.
#[allow(clippy::too_many_arguments)]
pub async fn reserve_and_verify(
    c: &std::sync::Arc<GwClient>,
    res_seq: &str,
    req_text: &str,
    start: &str,
    end: &str,
    desc: &str,
    attendees: &[String],
) -> Result<Value> {
    let subscribers = resolve_subscribers(c, attendees).await?;
    let reg = create_reservation(c, res_seq, req_text, start, end, desc, &subscribers)
        .await
        .map_err(|e| anyhow!("예약 등록 실패: {e}"))?;

    let seq_num = reg
        .get("seqNum")
        .and_then(|v| v.as_i64())
        .ok_or_else(|| anyhow!("등록 응답에 seqNum 없음"))?;
    let res_idx = json_idx(reg.get("resIdx")).unwrap_or_else(|| "1".to_string());

    let detail = get_reservation(c, res_seq, seq_num, &res_idx)
        .await
        .map_err(|e| anyhow!("등록 후 재조회 실패: {e}"))?;
    let d = |k: &str| detail.get(k).and_then(|v| v.as_str()).unwrap_or("");
    let stored = d("reqText");
    let reflected = stored == req_text;

    // ⚠️ `reqText`는 **요청값이 아니라 재조회로 읽은 실제 저장값**을 싣는다. 요청값을 그대로
    // 되돌려주면 서버가 다르게 저장했을 때(`ok:false`) 응답만 보고는 무엇이 저장됐는지 알 수 없다.
    // 수정(`update_and_verify`)도 같은 규칙이다.
    // 참석자는 **재조회한 subscriberList**로 확인한다 — 요청값 echo가 아니다.
    // 아마란스는 모르는 값을 조용히 버리므로 "보냈다"가 "들어갔다"의 근거가 못 된다.
    let stored_att = subscriber_names(&detail);
    let att_ok = stored_att.len() == subscribers.len();
    let mut msg = json!({
        "ok": reflected && att_ok,
        "seqNum": seq_num,
        "resIdx": res_idx,
        "resSeq": res_seq,
        "reqText": stored,
        "displayTitle": display_title(d("empName"), d("resName")),
        "period": format!("{start}~{end}"),
        "attendees": stored_att,
        "attendeesVerified": att_ok,
        "verified_by_readback": reflected && att_ok
    });
    if !att_ok {
        msg["attendeesWarning"] = json!(format!(
            "참석자 {}명을 보냈는데 재조회에는 {}명이다 — 서버가 일부를 받지 않았다. \
             아마란스 웹에서 예약을 열어 참석자를 확인할 것",
            subscribers.len(), stored_att.len()
        ));
    }
    if let Some(w) = lunch_warning(start, end) {
        msg["lunchWarning"] = json!(w);
    }
    Ok(msg)
}

/// 예약 수정 + **소유권 가드 + read-back 검증**. 변경분만 지정하고 나머지는 기존값을 유지한다.
/// ⚠️ **시간을 바꾸면 서버가 예약을 재발급해 `seqNum`/`resIdx`가 바뀐다**(rs121A12 동작 특성).
/// 그래서 read-back은 요청에 쓴 값이 아니라 **응답이 준 새 ID**로 한다.
#[allow(clippy::too_many_arguments)]
pub async fn update_and_verify(
    c: &std::sync::Arc<GwClient>,
    res_seq: &str,
    seq_num: i64,
    res_idx: Option<&str>,
    req_text: Option<&str>,
    start: Option<&str>,
    end: Option<&str>,
    desc: Option<&str>,
    attendees: Option<&[String]>,
) -> Result<Value> {
    let res_idx = res_idx.unwrap_or("1");
    let detail = get_reservation(c, res_seq, seq_num, res_idx)
        .await
        .map_err(|e| anyhow!("예약 조회 실패(없거나 접근불가): {e}"))?;
    check_owner(&detail, &c.emp_seq(), "수정")?;

    let f = merge_update(&detail, req_text, start, end, desc);
    let subscribers = match attendees {
        Some(specs) => resolve_subscribers(c, specs).await?,
        None => keep_subscribers(&detail)?,
    };
    let upd = update_reservation(
        c, res_seq, seq_num, res_idx,
        &f.req_text, &f.start, &f.end, &f.desc,
        &f.orig_start, &f.create_date, &f.res_name,
        &subscribers,
    )
    .await
    .map_err(|e| anyhow!("예약 수정 실패: {e}"))?;

    let new_seq = upd.get("seqNum").and_then(|v| v.as_i64()).unwrap_or(seq_num);
    let new_idx = json_idx(upd.get("resIdx")).unwrap_or_else(|| res_idx.to_string());

    let after = get_reservation(c, res_seq, new_seq, &new_idx)
        .await
        .map_err(|e| anyhow!("수정 후 재조회 실패: {e}"))?;
    let ag = |k: &str| after.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let reflected = ag("startDate") == f.start && ag("endDate") == f.end && ag("reqText") == f.req_text;

    // 참석자도 재조회로 확인한다 — 특히 **미지정 수정이 기존 참석자를 지키는지**가 여기서 드러난다.
    let stored_att = subscriber_names(&after);
    let att_ok = stored_att.len() == subscribers.len();
    let mut msg = json!({
        "ok": reflected && att_ok,
        "seqNum": new_seq,
        "resIdx": new_idx,
        "prev_seqNum": seq_num,
        "reissued": new_seq != seq_num,
        "reqText": ag("reqText"),
        "displayTitle": display_title(&ag("empName"), &ag("resName")),
        "period": format!("{}~{}", ag("startDate"), ag("endDate")),
        "attendees": stored_att,
        "attendeesVerified": att_ok,
        "verified_by_readback": reflected && att_ok
    });
    if !att_ok {
        msg["attendeesWarning"] = json!(format!(
            "참석자 {}명을 보냈는데 재조회에는 {}명이다 — 수정 과정에서 참석자가 빠졌을 수 있다. \
             아마란스 웹에서 예약을 열어 확인할 것",
            subscribers.len(), stored_att.len()
        ));
    }
    if let Some(w) = lunch_warning(&f.start, &f.end) {
        msg["lunchWarning"] = json!(w);
    }
    Ok(msg)
}

/// 예약 취소 + **소유권 가드 + read-back 검증**.
/// ⚠️ 삭제 성공 판정은 **재조회 실패**다(자원 API 규약 — 지워진 예약은 조회가 에러를 낸다).
pub async fn cancel_and_verify(
    c: &GwClient,
    res_seq: &str,
    seq_num: i64,
    res_idx: Option<&str>,
) -> Result<Value> {
    let res_idx = res_idx.unwrap_or("1");
    let detail = get_reservation(c, res_seq, seq_num, res_idx)
        .await
        .map_err(|e| anyhow!("예약 조회 실패(없거나 접근불가): {e}"))?;
    check_owner(&detail, &c.emp_seq(), "취소")?;

    let get = |k: &str| detail.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    delete_reservation(
        c, res_seq, seq_num, res_idx,
        &get("reqText"), &get("startDate"), &get("endDate"),
        &get("createDate"), &get("resName"),
    )
    .await
    .map_err(|e| anyhow!("예약 취소 실패: {e}"))?;

    let gone = get_reservation(c, res_seq, seq_num, res_idx).await.is_err();
    Ok(json!({
        "ok": gone,
        "seqNum": seq_num,
        "canceled": true,
        "verified_by_readback": gone
    }))
}

/// 기간·자원별 예약 현황(조회용 가공 포함).
/// `res_seqs`가 비면 **전체 회의실**을 대상으로 하고, `verbose=false`면 슬림 형태로 축약한다
/// (원본은 74필드 + 회의 안건 전문까지 실려 와 토큰을 크게 먹는다).
pub async fn reservations_view(
    c: &GwClient,
    start: &str,
    end: &str,
    res_seqs: &[String],
    verbose: bool,
) -> Result<Value> {
    let owned: Vec<String>;
    let seqs: &[String] = if res_seqs.is_empty() {
        owned = resources_in_group(c, "")
            .await?
            .iter()
            .filter_map(|r| r.get("resSeq").and_then(|s| s.as_str()).map(String::from))
            .collect();
        &owned
    } else {
        res_seqs
    };
    let refs: Vec<&str> = seqs.iter().map(|s| s.as_str()).collect();
    let data = list_reservations(c, start, end, &refs).await?;
    if verbose {
        return Ok(data);
    }
    let rows: Vec<Value> = data
        .get("resultList")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().map(slim_reservation).collect())
        .unwrap_or_default();
    Ok(json!({
        "period": format!("{start}~{end}"),
        "count": rows.len(),
        "reservations": rows
    }))
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

    /// 예약이 하나도 없으면 탐색 창 전체가 하나의 빈 구간이다.
    #[test]
    fn free_slots는_예약이_없으면_창_전체를_준다() {
        assert_eq!(free_slots(&[], 540, 1080, 30), vec![(540, 1080)]);
    }

    /// 겹치는 두 예약은 한 덩어리로 병합돼야 한다 — 안 그러면 그 사이에 0분짜리 빈 구간이 생긴다.
    #[test]
    fn free_slots는_겹치는_구간을_병합한다() {
        let busy = [(600, 700), (650, 800)];
        assert_eq!(free_slots(&busy, 540, 1080, 30), vec![(540, 600), (800, 1080)]);
    }

    /// 입력 순서를 타지 않고(내부에서 정렬한다), 끝과 시작이 맞닿은(틈 0) 예약 사이에
    /// 0분짜리 빈 구간을 만들지 않는다.
    /// ⚠️ 병합 분기(`a <= last.1`) 자체는 이 단언으로 구분되지 않는다 — `duration_min >= 1`인 한
    /// 병합을 빼도 뒤의 `cursor.max(z)`와 길이 필터가 같은 결과를 낸다. 그 분기는 계약 밖
    /// (`duration_min <= 0`) 입력으로 고정하며, 그쪽은 아래 특성화 테스트가 맡는다.
    #[test]
    fn free_slots는_입력_순서를_타지_않고_맞닿는_예약에_틈을_만들지_않는다() {
        let busy = [(600, 700), (700, 800)];
        assert_eq!(free_slots(&busy, 540, 1080, 30), vec![(540, 600), (800, 1080)]);
        // 입력 순서가 뒤집혀도 같아야 한다(내부에서 정렬한다).
        let reversed = [(700, 800), (600, 700)];
        assert_eq!(free_slots(&reversed, 540, 1080, 30), free_slots(&busy, 540, 1080, 30));
    }

    /// 겹침 병합 분기(`Some(last) if a <= last.1`)를 고정하는 **특성화 테스트**.
    ///
    /// ⚠️ **여기 적힌 값은 "옳은 답"이 아니라 "지금 이렇게 동작한다"이다.** `free_slots`의 계약은
    /// doc 주석대로 `duration_min >= 1`이고 유일한 호출부 `find_free_slots`가 그것을 강제한다.
    /// 그런데 병합 분기는 **계약 안에서는 결과를 전혀 바꾸지 않아**(끝점 0..8·구간 3개까지
    /// 완전탐색: `duration_min >= 1`에서 병합 유무 차이 **0건**) 계약 안 입력으로는 고정할 수 없다.
    /// `free_slots`는 private이라 계약 밖 입력을 여기서 직접 넘길 수 있으므로, 그 입력으로
    /// **분기가 조용히 사라지는 것만** 막는다.
    #[test]
    fn free_slots의_겹침병합은_계약밖_입력에서만_결과를_바꾼다() {
        // ① 맞닿은 예약 + duration_min=0 — 병합하지 않으면 두 예약 사이에 **길이 0짜리 빈 구간**
        //    (700,700)이 끼어든다. 병합하면 한 덩어리라 그 틈 자체가 없다.
        assert_eq!(
            free_slots(&[(600, 700), (700, 800)], 540, 1080, 0),
            vec![(540, 600), (800, 1080)],
            "병합이 빠지면 (700,700) 0분짜리 구간이 낀다"
        );

        // ② 완전히 겹치는 예약 + 음수 — 병합하지 않으면 `a - cursor`가 음수인 채로 필터를 통과해
        //    **시작 > 끝인 뒤집힌 구간** (700,600)이 나온다. 병합이 그것을 막는다.
        assert_eq!(
            free_slots(&[(600, 700), (600, 700)], 540, 1080, -100),
            vec![(540, 600), (700, 1080)],
            "병합이 빠지면 (700,600)처럼 뒤집힌 구간이 나온다"
        );

        // ③ 계약 안에서는 병합이 결과를 바꾸지 않는다는 사실 자체를 남겨둔다 — 겹치거나 맞닿은
        //    입력이 "이미 병합된 입력"과 같은 답을 낸다. (이 단언은 분기가 없어도 통과한다.
        //    분기를 지키는 것은 ①·②다.)
        for d in [1, 30, 100] {
            assert_eq!(
                free_slots(&[(600, 700), (700, 800)], 540, 1080, d),
                free_slots(&[(600, 800)], 540, 1080, d)
            );
            assert_eq!(
                free_slots(&[(600, 700), (600, 700)], 540, 1080, d),
                free_slots(&[(600, 700)], 540, 1080, d)
            );
        }
    }

    /// 창을 넘겨 덮는 예약(다일 예약 센티넬 포함)은 창 전체 점유 → 후보 없음.
    #[test]
    fn free_slots는_창이_다_차면_빈_구간이_없다() {
        assert!(free_slots(&[(500, 1200)], 540, 1080, 30).is_empty());
        assert!(free_slots(&[(i64::MIN / 4, i64::MAX / 4)], 540, 1080, 30).is_empty());
    }

    /// 창 밖 예약은 판정에서 빠진다 — 창을 침범하지 않으면 없는 것과 같다.
    #[test]
    fn free_slots는_창_밖_예약을_무시한다() {
        let busy = [(0, 300), (1200, 1300)];
        assert_eq!(free_slots(&busy, 540, 1080, 30), vec![(540, 1080)]);
    }

    /// `duration_min`보다 짧은 틈은 후보가 아니다.
    #[test]
    fn free_slots는_요청_시간보다_짧은_틈을_버린다() {
        let busy = [(600, 700), (720, 800)]; // 700~720 = 20분
        assert_eq!(free_slots(&busy, 540, 1080, 30), vec![(540, 600), (800, 1080)]);
        assert_eq!(
            free_slots(&busy, 540, 1080, 20),
            vec![(540, 600), (700, 720), (800, 1080)]
        );
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

    /// 상세 응답의 참석자를 **쓰기 API가 받는 4키로만** 투영하는지.
    /// 여분 키(deptName/empName/loginId/useYn)를 그대로 실어 보내면 서버 동작이 미실측 영역이 된다.
    #[test]
    fn 참석자_투영은_쓰기_4키만_남긴다() {
        let list = vec![json!({
            "compSeq": "1000", "deptName": "네이티브 플랫폼팀", "deptSeq": "2993",
            "empName": "이재학", "empSeq": "3166", "groupSeq": "G1",
            "loginId": "jaehak.lee", "useYn": "Y"
        })];
        let out = project_subscribers(&list);
        assert_eq!(out.len(), 1);
        let keys: Vec<&String> = out[0].as_object().unwrap().keys().collect();
        assert_eq!(keys, vec!["compSeq", "deptSeq", "empSeq", "groupSeq"]);
        assert_eq!(out[0]["deptSeq"], "2993");
        assert_eq!(out[0]["empSeq"], "3166");

        // empSeq 없는 항목은 버린다(그대로 보내면 서버가 뭘 할지 모른다).
        assert!(project_subscribers(&[json!({ "empName": "누구" })]).is_empty());
    }

    /// ⭐ **수정이 참석자를 조용히 날리지 않는지.** 예전 구현은 `[본인]`을 고정으로 보내서
    /// 예약명만 고쳐도 참석자 전원이 빠졌다(그 사람들에게는 통지도 가지 않는다).
    #[test]
    fn 참석자_미지정_수정은_기존_참석자를_지킨다() {
        let detail = json!({ "subscriberList": [
            { "compSeq": "1000", "deptSeq": "2993", "empSeq": "3166", "groupSeq": "G1" },
            { "compSeq": "1000", "deptSeq": "2986", "empSeq": "3137", "groupSeq": "G1" },
        ]});
        let kept = keep_subscribers(&detail).unwrap();
        assert_eq!(kept.len(), 2, "기존 참석자가 그대로 유지돼야 한다");
        assert_eq!(kept[1]["empSeq"], "3137");
        // ⚠️ 각자의 부서가 보존돼야 한다 — 예약자 부서로 덮으면 남의 부서가 틀리게 박힌다.
        assert_eq!(kept[1]["deptSeq"], "2986");

        // 읽지 못하면 "참석자 없음"이 아니라 **중단**이다.
        assert!(keep_subscribers(&json!({})).is_err());
        assert!(keep_subscribers(&json!({ "subscriberList": null })).is_err());
        // 빈 배열은 읽은 것이다(참석자가 실제로 없음) — 중단이 아니다.
        assert!(keep_subscribers(&json!({ "subscriberList": [] })).unwrap().is_empty());
    }

    /// read-back 보고용 이름 추출.
    #[test]
    fn 참석자_이름은_상세에서_뽑는다() {
        let detail = json!({ "subscriberList": [
            { "empName": "이재학", "empSeq": "3166" },
            { "empName": "홍길동", "empSeq": "3137" },
        ]});
        assert_eq!(subscriber_names(&detail), vec!["이재학", "홍길동"]);
        assert!(subscriber_names(&json!({})).is_empty());
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

    /// 서버가 `resIdx`를 number로도 string으로도 준다 — 둘 다 같은 인덱스 문자열이 돼야
    /// 예약 수정·취소가 대상을 놓치지 않는다. (mcp.rs에서 이 모듈로 이동)
    #[test]
    fn json_idx는_number와_string을_같게_만든다() {
        assert_eq!(json_idx(Some(&json!("3"))), Some("3".into()));
        assert_eq!(json_idx(Some(&json!(3))), Some("3".into()));
        assert_eq!(json_idx(None), None);
        assert_eq!(json_idx(Some(&Value::Null)), Some("null".into()));
    }

    /// 소유권 가드(§7.2). ⚠️ `empSeq`가 **없으면 거부**하는 것도 계약이다 — mcp.rs에 있던
    /// 동작(`unwrap_or("")` 후 비교)을 그대로 옮겼는지 확인하는 것이 이 테스트의 목적.
    #[test]
    fn check_owner는_타인과_필드누락을_거부한다() {
        let mine = json!({ "empSeq": "3166", "reqText": "내 예약" });
        assert!(check_owner(&mine, "3166", "수정").is_ok());

        let others = json!({ "empSeq": "12345", "reqText": "남의 예약" });
        let err = check_owner(&others, "3166", "수정").unwrap_err();
        assert!(err.downcast_ref::<NotOwner>().is_some(), "NotOwner 타입으로 올라와야 invalid_params로 매핑된다");
        assert!(err.to_string().contains("본인 소유 예약이 아니라 수정할 수 없습니다"));

        // empSeq 필드 자체가 없으면 ""와 비교 → 불일치 → 거부
        assert!(check_owner(&json!({}), "3166", "취소").is_err());
    }

    /// 화면 표시명은 **예약명과 별개**다. 서버가 준 값이 있으면 그걸 쓰고, 없으면 같은 규칙으로 조립한다.
    /// (목록 `rs121A05`에는 `resTitleDisplay`가 있지만 상세 `rs121A10`에는 없다 — 2026-08-06 실측.)
    #[test]
    fn slim_reservation은_예약명과_화면표시명을_구분해_담는다() {
        let raw = json!({
            "resName": "회의실 B", "empName": "이재학", "reqText": "회의",
            "resTitleDisplay": "[이재학] 회의실 B"
        });
        let v = slim_reservation(&raw);
        assert_eq!(v["title"], "회의", "title은 예약명(reqText)");
        assert_eq!(v["displayTitle"], "[이재학] 회의실 B", "화면에 찍히는 문구");

        // 서버가 안 줬을 때만 조립
        let mut raw2 = raw.clone();
        raw2["resTitleDisplay"] = json!("");
        assert_eq!(slim_reservation(&raw2)["displayTitle"], "[이재학] 회의실 B");
    }

    #[test]
    fn display_title은_예약자와_자원명을_대괄호로_묶는다() {
        assert_eq!(display_title("이재학", "회의실 B"), "[이재학] 회의실 B");
        assert_eq!(display_title("", ""), "[] ");
    }

    /// 점심시간 겹침 판정. 경계(13:00 종료·14:00 시작)는 **겹치지 않는 것**이 핵심 —
    /// 12:00~13:00 회의에 경고가 뜨면 매일 오전 회의마다 잡음이 된다.
    #[test]
    fn overlaps_lunch는_경계를_제외하고_판정한다() {
        // 겹침
        assert!(overlaps_lunch("202608061300", "202608061400")); // 정확히 점심
        assert!(overlaps_lunch("202608061230", "202608061330")); // 앞에서 물림
        assert!(overlaps_lunch("202608061330", "202608061430")); // 뒤로 물림
        assert!(overlaps_lunch("202608061000", "202608061800")); // 통째로 포함
        assert!(overlaps_lunch("202608061310", "202608061320")); // 점심 안쪽

        // 안 겹침 — 경계 맞닿음
        assert!(!overlaps_lunch("202608061200", "202608061300"));
        assert!(!overlaps_lunch("202608061400", "202608061500"));
        assert!(!overlaps_lunch("202608060900", "202608061000"));

        // 다일 예약: 하루라도 점심을 덮으면 겹침
        assert!(overlaps_lunch("202608061800", "202608081000"), "중간 날(8/7) 점심을 덮는다");
        assert!(!overlaps_lunch("202608061800", "202608071000"), "8/6 저녁~8/7 오전은 점심 없음");
        assert!(overlaps_lunch("202608311800", "202609011500"), "월 경계 — 9/1 점심을 덮는다");

        // 형식 오류·역전 구간은 판단하지 않는다(경고는 부가 정보라 예약을 막지 않는다)
        assert!(!overlaps_lunch("20260806", "202608061400"));
        assert!(!overlaps_lunch("202608061400", "202608061300"));
        assert!(!overlaps_lunch("", ""));
    }

    #[test]
    fn lunch_warning은_겹칠때만_문구를_준다() {
        assert!(lunch_warning("202608061230", "202608061330")
            .is_some_and(|w| w.contains("13:00~14:00")));
        assert_eq!(lunch_warning("202608061400", "202608061500"), None);
    }

    /// rs121A12는 전체 필드를 요구하므로, 미지정 항목은 현재 예약값으로 채워야 한다.
    /// `orig_start`는 **변경 후가 아니라 변경 전** 시작시각(식별키)이라는 점이 핵심.
    #[test]
    fn merge_update는_지정한_필드만_덮어쓴다() {
        let current = json!({
            "reqText": "기존제목", "startDate": "202608051000", "endDate": "202608051100",
            "descText": "기존내용", "createDate": "20260801", "resName": "대회의실"
        });
        let f = merge_update(&current, Some("새제목"), None, None, None);
        assert_eq!(f.req_text, "새제목");
        assert_eq!(f.start, "202608051000"); // 미지정 → 기존 유지
        assert_eq!(f.end, "202608051100");
        assert_eq!(f.desc, "기존내용");
        assert_eq!(f.orig_start, "202608051000");
        assert_eq!(f.create_date, "20260801");
        assert_eq!(f.res_name, "대회의실");

        // 시간만 바꾸면 orig_start는 여전히 기존값이어야 한다(식별키가 깨지면 수정이 실패한다).
        let f2 = merge_update(&current, None, Some("202608051400"), Some("202608051500"), None);
        assert_eq!(f2.start, "202608051400");
        assert_eq!(f2.orig_start, "202608051000");
        assert_eq!(f2.req_text, "기존제목");
    }

    #[test]
    fn merge_update는_빈_상세도_견딘다() {
        let f = merge_update(&json!({}), None, None, None, None);
        assert_eq!(f, UpdateFields {
            req_text: "".into(), start: "".into(), end: "".into(), desc: "".into(),
            orig_start: "".into(), create_date: "".into(), res_name: "".into(),
        });
    }
}
