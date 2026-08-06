//! 근태 출퇴근(human 모듈) — `/human/common/judgeTimeManagement/*`. ⚠️ **실제 근태 기록(쓰기)**.
//! 실측: `06-attendance-api-capture.md`. `attendFg` **1=출근(clock in) / 4=퇴근(clock out)**.
//! empCd/deptCd/coCd는 ensure_session(gw050A02)에서 확보. 성공 판정은 응답 successCount가 아니라
//! read-back(getTodayComeLeaveInfo)의 comeTm/leaveTm으로 — [[verify-mutations-with-readback]].

use anyhow::Result;
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::util::days_to_ymd;

const BASE: &str = "/human/common/judgeTimeManagement";

/// 오늘 출퇴근 현황 — getTodayComeLeaveInfo(읽기). `work_dt`=YYYYMMDD.
pub async fn today(c: &GwClient, work_dt: &str) -> Result<Value> {
    let body = json!({ "empCd": c.emp_cd(), "coCd": c.co_cd(), "workDt": work_dt });
    let d = c.call(&format!("{BASE}/getTodayComeLeaveInfo"), &body).await?;
    Ok(json!({
        "workDt": work_dt,
        "comeTm": d.get("comeTm").cloned().unwrap_or(Value::Null),   // 출근 YYYYMMDDHHmm (빈문자=미등록)
        "leaveTm": d.get("leaveTm").cloned().unwrap_or(Value::Null), // 퇴근
        "holidayYn": d.get("holidayYn").cloned().unwrap_or(Value::Null)
    }))
}

/// 사전 체크 — confirmApplicationStatus(읽기). 채널 허용 플래그(webYn 등) 확인.
async fn confirm_status(c: &GwClient) -> Result<Value> {
    let body = json!({ "empCd": c.emp_cd(), "deptCd": c.dept_cd(), "coCd": c.co_cd() });
    c.call(&format!("{BASE}/confirmApplicationStatus"), &body).await
}

/// 실제 punch — getJudgeTimeManagement(쓰기). `attend_fg` 1=출근/4=퇴근.
async fn punch(c: &GwClient, attend_fg: &str) -> Result<Value> {
    let body = json!({
        "type": "WEB",
        "judgeData": { "empCd": c.emp_cd(), "deptCd": c.dept_cd(), "coCd": c.co_cd(), "attendFg": attend_fg }
    });
    c.call(&format!("{BASE}/getJudgeTimeManagement"), &body).await
}

/// 출근/퇴근 punch + read-back 검증. `attend_fg` 1=출근/4=퇴근.
/// ⚠️ **중복 방지 가드**: 해당 기록(출근=comeTm/퇴근=leaveTm)이 이미 있으면 재punch하지 않음
/// (기존 시각 덮어쓰기 방지). 재기록이 필요하면 호출부에서 명시.
pub async fn punch_and_verify(c: &GwClient, attend_fg: &str) -> Result<Value> {
    let kind = if attend_fg == "1" { "출근" } else { "퇴근" };
    let field = if attend_fg == "1" { "comeTm" } else { "leaveTm" };
    let wd = today_kst();

    // 현재 상태 확인(가드)
    let before = today(c, &wd).await?;
    let existing = before.get(field).and_then(|v| v.as_str()).unwrap_or("");
    if !existing.is_empty() {
        return Ok(json!({
            "ok": true, "already": true, "kind": kind, "workDt": wd,
            "comeTm": before.get("comeTm"), "leaveTm": before.get("leaveTm"),
            "note": format!("이미 {kind} 기록({existing})이 있어 재punch하지 않음(덮어쓰기 방지).")
        }));
    }

    // pre-check(정보성, 실패해도 진행) → punch
    let _ = confirm_status(c).await;
    punch(c, attend_fg).await?;

    // read-back: successCount는 신뢰 불가 → comeTm/leaveTm 반영으로 판정
    let after = today(c, &wd).await?;
    let now_val = after.get(field).and_then(|v| v.as_str()).unwrap_or("");
    let reflected = !now_val.is_empty();
    Ok(json!({
        "ok": reflected, "kind": kind, "workDt": wd,
        "comeTm": after.get("comeTm"), "leaveTm": after.get("leaveTm"),
        "verified_by_readback": reflected,
        "note": if reflected {
            format!("{kind} 기록 확인({now_val}).")
        } else {
            format!("punch 응답은 왔으나 read-back에 {field} 미반영 — 수동 확인 필요.")
        }
    }))
}

/// 기간 근태 현황 — `/human/openapi/worktime/status/getWorkTimeStatusList`(읽기).
/// `judgeTimeManagement` 계열에는 월별 API가 없고 이 openapi 경로가 담당한다(10 문서 실측).
/// `start`/`end`=YYYYMMDD. 1행=1일이지만 **누락일이 있다**(마감 전인 오늘 등) → 날짜는
/// 행 순서가 아니라 `atDt`로 볼 것.
pub async fn work_time_status(c: &GwClient, start: &str, end: &str) -> Result<Value> {
    let body = json!({
        "coCd": c.co_cd(),
        "startDate": start,
        "endDate": end,
        "empCdList": [c.emp_cd()]
    });
    let d = c
        .call("/human/openapi/worktime/status/getWorkTimeStatusList", &body)
        .await?;
    let rows = d.as_array().cloned().unwrap_or_default();

    let s = |r: &Value, k: &str| r.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let n = |r: &Value, k: &str| r.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
    // 출퇴근은 HHmm 4자리로 온다(getTodayComeLeaveInfo의 12자리와 형식이 다름).
    let hm = |t: String| {
        if t.len() == 4 {
            format!("{}:{}", &t[0..2], &t[2..4])
        } else {
            t
        }
    };

    let mut days: Vec<Value> = Vec::new();
    let (mut work_days, mut total_work, mut total_over, mut late, mut absent) = (0, 0, 0, 0, 0);
    for r in &rows {
        let result = s(r, "attresultNm");
        let holi = s(r, "holiFg") == "1";
        let basic = n(r, "basicworkTm");
        let over = n(r, "overworkTm");
        match result.as_str() {
            "지각" => late += 1,
            "결근" => absent += 1,
            _ => {}
        }
        if holi && basic > 0 {
            work_days += 1;
            total_work += basic;
            total_over += over;
        }
        days.push(json!({
            "date": s(r, "atDt"),
            "dayType": s(r, "holiNm"),          // 근로일/주휴/무휴
            "result": result,                    // 정상근무/지각/조퇴/결근/휴일
            "come": hm(s(r, "comeTm")),
            "leave": hm(s(r, "leaveTm")),
            "workMin": basic,
            "overtimeMin": over,
            "reason": r.get("atNm").cloned().unwrap_or(Value::Null)  // 연차 등
        }));
    }
    days.sort_by(|a, b| a["date"].as_str().cmp(&b["date"].as_str()));

    Ok(json!({
        "kind": "attendancePeriod",
        "period": format!("{start}~{end}"),
        "rowCount": rows.len(),
        "summary": {
            "workDays": work_days,
            "totalWorkMin": total_work,
            "totalWorkHours": format!("{}h{:02}m", total_work / 60, total_work % 60),
            "overtimeMin": total_over,
            "lateCount": late,
            "absentCount": absent
        },
        "days": days
    }))
}

/// `YYYYMM` → (해당 월 1일, 말일) YYYYMMDD.
pub fn month_range(yyyymm: &str) -> Result<(String, String)> {
    if yyyymm.len() != 6 || !yyyymm.chars().all(|c| c.is_ascii_digit()) {
        anyhow::bail!("month는 YYYYMM 6자리여야 합니다: '{yyyymm}'");
    }
    let y: i64 = yyyymm[0..4].parse()?;
    let m: i64 = yyyymm[4..6].parse()?;
    if !(1..=12).contains(&m) {
        anyhow::bail!("월 범위 오류: '{yyyymm}'");
    }
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let last = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ => {
            if leap {
                29
            } else {
                28
            }
        }
    };
    Ok((format!("{yyyymm}01"), format!("{yyyymm}{last:02}")))
}

/// 오늘(KST) YYYYMMDD. 근태 workDt는 로컬(KST) 기준이라 UTC에 +9h 보정 후 날짜 산출.
pub fn today_kst() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = days_to_ymd((secs + 9 * 3600) / 86400);
    format!("{y:04}{m:02}{d:02}")
}


#[cfg(test)]
// 테스트 이름에 아마란스 실제 필드명(empSeq·delYn·boardType…)을 그대로 적는다 —
// 무엇을 검증하는지 이름만 보고 알기 위해서다. 소문자로 풀면 실재하지 않는 이름이 되므로
// 이름을 바꾸는 대신 lint를 끈다. (한글은 대소문자가 없어 경고 대상이 아니다.)
#[allow(non_snake_case)]
mod tests {
    use super::*;

    #[test]
    fn month_range는_윤년과_월경계를_처리한다() {
        assert_eq!(month_range("202402").unwrap(), ("20240201".into(), "20240229".into())); // 윤년
        assert_eq!(month_range("202602").unwrap(), ("20260201".into(), "20260228".into())); // 평년
        assert_eq!(month_range("210002").unwrap(), ("21000201".into(), "21000228".into())); // 100년=평년
        assert_eq!(month_range("200002").unwrap(), ("20000201".into(), "20000229".into())); // 400년=윤년
        assert_eq!(month_range("202612").unwrap(), ("20261201".into(), "20261231".into()));
        assert_eq!(month_range("202601").unwrap(), ("20260101".into(), "20260131".into()));
        assert_eq!(month_range("202604").unwrap(), ("20260401".into(), "20260430".into()));
    }

    #[test]
    fn month_range는_잘못된_입력을_거른다() {
        assert!(month_range("2026").is_err());   // 6자리 아님
        assert!(month_range("202613").is_err()); // 월 범위 초과
        assert!(month_range("202600").is_err()); // 0월
        assert!(month_range("20260a").is_err()); // 숫자 아님
        assert!(month_range("").is_err());
    }

    /// KST 보정이 실제로 걸리는지. 값 자체는 오늘에 의존하므로 형식·자기일관성만 본다.
    #[test]
    fn today_kst는_8자리_YYYYMMDD다() {
        let t = today_kst();
        assert_eq!(t.len(), 8);
        assert!(t.chars().all(|c| c.is_ascii_digit()));
        let (y, m, d) = (&t[0..4], &t[4..6], &t[6..8]);
        assert!(y >= "2020");
        assert!((1..=12).contains(&m.parse::<i64>().unwrap()));
        assert!((1..=31).contains(&d.parse::<i64>().unwrap()));
    }
}
