//! 근태 출퇴근(human 모듈) — `/human/common/judgeTimeManagement/*`. ⚠️ **실제 근태 기록(쓰기)**.
//! 실측: `06-attendance-api-capture.md`. `attendFg` **1=출근(clock in) / 4=퇴근(clock out)**.
//! empCd/deptCd/coCd는 ensure_session(gw050A02)에서 확보. 성공 판정은 응답 successCount가 아니라
//! read-back(getTodayComeLeaveInfo)의 comeTm/leaveTm으로 — [[verify-mutations-with-readback]].

use anyhow::Result;
use serde_json::{json, Value};

use crate::client::GwClient;

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

/// 오늘(KST) YYYYMMDD. 근태 workDt는 로컬(KST) 기준이라 UTC에 +9h 보정 후 날짜 산출.
pub fn today_kst() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, m, d) = days_to_ymd((secs + 9 * 3600) / 86400);
    format!("{y:04}{m:02}{d:02}")
}

/// epoch days → (year, month, day). Howard Hinnant civil_from_days.
fn days_to_ymd(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}
