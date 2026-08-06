//! 도메인 무관 순수 함수 모음.
//!
//! **경계**: 여기 있는 것은 아마란스 API·모듈 도메인을 전혀 모르는 순수 함수뿐이다.
//! 특정 모듈의 규칙(엔드포인트·필드 매핑·본문 평문화 방식 등)이 섞이기 시작하면 이 파일이
//! "잡동사니 서랍"이 된다 — 그런 코드는 해당 도메인 모듈에 둘 것.
//!
//! ## ⚠️ 여기로 합치면 **안 되는** 것들 (이름이 비슷하다고 통합 금지)
//!
//! | 함수 | 위치 | 왜 별개인가 |
//! |---|---|---|
//! | `html_to_text`·`collapse_ws` | `modules::board` / `modules::approval` | **2쌍이 의도적으로 다르다.** board(+mail)는 블록 태그를 개행으로 살리고 빈 줄을 1개까지 유지, approval은 태그를 공백으로 바꾸고 개행을 전부 없앤다(본문 한 줄). 합치면 게시판/메일 또는 결재 본문 표시가 조용히 바뀐다. 각 모듈의 테스트가 이 차이를 못박고 있다. |
//! | `s(&Value, &str)` | `modules::search` | 여기 `s`와 달리 **다국어 객체(`{kr,en,…}`)에서 `kr`을 꺼낸다**(결재 deptNm/userNm/formNm 실측 대응). 대신 `Bool`을 처리하지 않는다. 통합하면 통합검색 결과의 부서·작성자가 빈 문자열이 된다. |
//! | `pct_decode` | `client` | 범용 퍼센트 디코더(응답 헤더 파일명용). |
//! | `url_decode` | `creds` | authToken 전용 최소 구현(`%7C`→`\|`만). 범용 디코더로 바꾸면 동작이 달라진다. |
//! | `encode_uri_component` | `modules::approval_submit` | JS 동등 인코더(공백→`%20`). `client::form_urlencode`(공백→`+`)와 규칙이 다르고 상신 본문 인코딩에 종속적이다. |
//!
//! 이 표는 공통 유틸 추출(2026-08-05) 때 "이름이 같아도 동작이 다르면 합치지 않는다"고 판단한 결과다.

use serde_json::Value;

/// epoch days → (year, month, day). Howard Hinnant `civil_from_days`.
/// `chrono` 의존성 없이 날짜를 다루기 위한 자체 구현(프로젝트에 날짜 크레이트가 없다).
pub fn days_to_ymd(z: i64) -> (i64, i64, i64) {
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

/// `YYYYMMDD` → epoch days. `days_to_ymd`의 역함수(Howard Hinnant `days_from_civil`).
/// 8자리가 아니거나 숫자가 아니면 `None`.
/// 날짜 간 경과일이 필요할 때 쓴다(예: 여러 날에 걸친 예약이 며칠을 덮는지).
pub fn ymd_to_days(ymd: &str) -> Option<i64> {
    if ymd.len() != 8 || !ymd.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let y: i64 = ymd[0..4].parse().ok()?;
    let m: i64 = ymd[4..6].parse().ok()?;
    let d: i64 = ymd[6..8].parse().ok()?;
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Some(era * 146097 + doe - 719468)
}

/// (y, m, d) → `YYYYMMDD`. `days_to_ymd`와 짝으로 쓴다.
pub fn fmt_ymd((y, m, d): (i64, i64, i64)) -> String {
    format!("{y:04}{m:02}{d:02}")
}

/// 숫자만 남긴다(`2026-08-05` → `20260805`). 날짜 인자 정규화용.
pub fn digits_only(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit()).collect()
}

/// JSON 필드를 문자열로. 서버가 number/string을 혼용한다
/// (예: `read_cnt`가 목록에선 문자열, 상세에선 정수).
/// `Option`을 받는 형태라 `a.get("x").or_else(|| a.get("y"))` 같은 폴백과 바로 이어진다.
pub fn json_str(v: Option<&Value>) -> String {
    match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => String::new(),
    }
}

/// `json_str(v.get(k))` 축약형. 필드를 바로 꺼낼 때 쓴다.
pub fn s(v: &Value, k: &str) -> String {
    json_str(v.get(k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 기대값은 python `date(1970,1,1)+timedelta(days=n)`로 독립 산출.
    #[test]
    fn days_to_ymd는_기준일과_경계를_맞춘다() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
        assert_eq!(days_to_ymd(-1), (1969, 12, 31)); // 에폭 이전(음수)
        assert_eq!(days_to_ymd(19723), (2024, 1, 1));
        assert_eq!(days_to_ymd(19782), (2024, 2, 29)); // 윤일
        assert_eq!(days_to_ymd(20670), (2026, 8, 5));
    }

    /// `days_to_ymd`의 역함수라는 것이 유일한 계약 — 왕복이 항등이어야 한다.
    #[test]
    fn ymd_to_days는_days_to_ymd의_역함수다() {
        for d in [0i64, -1, 19723, 19782, 20670, 25000] {
            assert_eq!(ymd_to_days(&fmt_ymd(days_to_ymd(d))), Some(d), "왕복 실패: {d}");
        }
        // 경과일 1 — 월/연 경계와 윤일에서도(문자열 뺄셈으로는 못 얻는 값)
        let diff = |a: &str, b: &str| ymd_to_days(a).unwrap() - ymd_to_days(b).unwrap();
        assert_eq!(diff("20260807", "20260806"), 1);
        assert_eq!(diff("20260901", "20260831"), 1);
        assert_eq!(diff("20270101", "20261231"), 1);
        assert_eq!(diff("20240301", "20240229"), 1); // 윤일

        assert_eq!(ymd_to_days("2026-08-06"), None); // 8자리 아님
        assert_eq!(ymd_to_days("2026080a"), None);
    }

    #[test]
    fn fmt_ymd는_8자리로_0패딩한다() {
        assert_eq!(fmt_ymd(days_to_ymd(20670)), "20260805");
        assert_eq!(fmt_ymd(days_to_ymd(0)), "19700101");
        assert_eq!(fmt_ymd(days_to_ymd(19782)), "20240229");
        assert_eq!(fmt_ymd((2026, 1, 2)), "20260102");
    }

    #[test]
    fn digits_only는_숫자만_남긴다() {
        assert_eq!(digits_only("2026-08-05"), "20260805");
        assert_eq!(digits_only("a1b2-3"), "123");
        assert_eq!(digits_only("가나"), "");
        assert_eq!(digits_only(""), "");
    }

    #[test]
    fn json_str은_number_string_bool을_흡수한다() {
        let v = json!({ "s": "1", "n": 2, "b": true, "null": null, "arr": [] });
        assert_eq!(json_str(v.get("s")), "1");
        assert_eq!(json_str(v.get("n")), "2");
        assert_eq!(json_str(v.get("b")), "true");
        assert_eq!(json_str(v.get("null")), "");
        assert_eq!(json_str(v.get("arr")), "");
        assert_eq!(json_str(None), "");
    }

    /// ⚠️ `search::s`와 달리 **다국어 객체를 풀지 않는다**(그쪽은 `kr`을 꺼낸다).
    /// 이 차이가 두 함수를 통합하면 안 되는 이유다.
    #[test]
    fn s는_객체를_문자열로_풀지_않는다() {
        let v = json!({ "k": "v", "multi": {"kr": "한국어"} });
        assert_eq!(s(&v, "k"), "v");
        assert_eq!(s(&v, "multi"), "", "객체는 빈 문자열 — search::s와 다른 지점");
        assert_eq!(s(&v, "없는키"), "");
    }
}
