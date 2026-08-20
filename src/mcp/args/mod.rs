//! MCP 도구 **인자 스키마**만 모아 둔 곳. 도메인별 파일 9개로 나뉜다.
//!
//! ⚠️ 여기 있는 doc comment는 전부 **LLM에게 가는 프롬프트**다(MCP 스키마의 `description`).
//! 문구를 고치면 모델의 인자 채우기 동작이 바뀐다 — 타입 정의가 아니라 문서로 취급할 것.
//!
//! serde 기본값 함수는 여기 모아 둔다. `one()`이 게시판(`ListNoticesArgs.page`)과
//! 전자결재(`ListApprovalsArgs.page`) **양쪽에서 쓰이기** 때문이다 — 도메인 파일에 두면
//! 한쪽이 다른 쪽을 import 하게 된다. 나머지도 성격이 같아 함께 둔다.
//! (`util.rs`는 도메인 무관 순수 함수용이라 성격이 다르다.)

pub mod approval;
pub mod attendance;
pub mod board;
pub mod calendar;
pub mod mail;
pub mod org;
pub mod person_group;
pub mod resource;
pub mod search;

/// ID·순번·날짜코드 인자의 **타입 강제**.
///
/// ## 왜 필요한가 (실측 사고)
///
/// 이 서버의 조회 도구가 돌려주는 값과 쓰기 도구가 요구하는 타입이 **서로 어긋난다**:
///
/// | 값 | 주는 쪽 | 받는 쪽 |
/// |---|---|---|
/// | `lineId` | `list_approval_lines` → **문자열** `"2047"` | `save_approval_line.line_id` = **정수** |
/// | 〃 | 〃 | `read_approval_line.line_id` = **문자열** (정반대!) |
/// | `formId` | `list_approvals`·`get_approval_line_schema` → 문자열/정수 혼용 | `save_approval_line.form_id` = 정수 |
///
/// 호출자가 앞 도구의 출력을 **그대로 물리면 실패**하고, rmcp가 내는 에러
/// (`invalid type: string "2047", expected i64`)에는 **어느 인자인지도 안 나온다**.
/// 이건 호출자가 조심해서 피할 문제가 아니라 스키마가 만들어낸 함정이라, 양쪽 다 받게 한다.
///
/// 스키마도 `["integer","string"]`으로 넓혀 **둘 다 된다는 사실을 모델에게 알린다** —
/// 조용히 받아주기만 하면 모델은 여전히 한쪽을 찍고 실패할 수 있다.
mod flex {
    use rmcp::schemars::{JsonSchema, Schema, SchemaGenerator};
    use serde::{Deserialize, Deserializer};
    use serde_json::Value;

    fn coerce_err<E: serde::de::Error>(v: &Value, want: &str) -> E {
        E::custom(format!("{want}로 해석할 수 없는 값: {v}"))
    }

    /// 정수 인자 — 문자열 `"36"`도 받는다.
    pub fn i64<'de, D: Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        match Value::deserialize(d)? {
            Value::Number(n) => n.as_i64().ok_or_else(|| coerce_err(&Value::Number(n.clone()), "정수")),
            Value::String(s) => s.trim().parse().map_err(|_| coerce_err(&Value::String(s.clone()), "정수")),
            other => Err(coerce_err(&other, "정수")),
        }
    }

    /// 문자열 ID 인자 — 숫자 `2047`도 받는다(`"2047"`로 변환).
    pub fn string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
        match Value::deserialize(d)? {
            Value::String(s) => Ok(s),
            Value::Number(n) => Ok(n.to_string()),
            other => Err(coerce_err(&other, "문자열")),
        }
    }

    /// 선택 문자열 ID 인자.
    pub fn string_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        match Value::deserialize(d)? {
            Value::Null => Ok(None),
            Value::String(s) => Ok(Some(s)),
            Value::Number(n) => Ok(Some(n.to_string())),
            other => Err(coerce_err(&other, "문자열")),
        }
    }

    /// 선택 문자열 **목록** 인자 — 항목마다 숫자를 받는다(`[3131, "송학현"]` → `["3131","송학현"]`).
    /// empSeq 목록은 앞 도구(`find_person`)의 출력을 그대로 물리는 자리라 혼용이 실제로 일어난다.
    pub fn string_vec_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
        let v = Value::deserialize(d)?;
        let items = match v {
            Value::Null => return Ok(None),
            // 하나만 줄 때 배열로 감싸지 않는 호출자를 받아준다.
            Value::String(s) => return Ok(Some(vec![s])),
            Value::Number(n) => return Ok(Some(vec![n.to_string()])),
            Value::Array(a) => a,
            other => return Err(coerce_err(&other, "문자열 목록")),
        };
        items
            .into_iter()
            .map(|x| match x {
                Value::String(s) => Ok(s),
                Value::Number(n) => Ok(n.to_string()),
                other => Err(coerce_err(&other, "문자열")),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some)
    }

    pub fn int_schema(g: &mut SchemaGenerator) -> Schema {
        widen(<i64 as JsonSchema>::json_schema(g), "integer")
    }

    pub fn str_schema(g: &mut SchemaGenerator) -> Schema {
        widen(<String as JsonSchema>::json_schema(g), "string")
    }

    pub fn str_opt_schema(g: &mut SchemaGenerator) -> Schema {
        widen(<Option<String> as JsonSchema>::json_schema(g), "string")
    }

    /// 목록 인자의 **항목** 타입을 넓힌다(바깥의 `["array","null"]`은 그대로 둔다).
    pub fn str_vec_opt_schema(g: &mut SchemaGenerator) -> Schema {
        let mut s = <Option<Vec<String>> as JsonSchema>::json_schema(g);
        if let Some(obj) = s.as_object_mut()
            && let Some(items) = obj.get_mut("items")
            && let Some(io) = items.as_object_mut()
        {
            io.insert("type".into(), serde_json::json!(["string", "integer"]));
        }
        s
    }

    /// 원래 스키마의 `type`에 짝이 되는 타입을 더한다(설명·기본값 등 나머지는 그대로 둔다).
    fn widen(mut s: Schema, base: &str) -> Schema {
        let other = if base == "integer" { "string" } else { "integer" };
        if let Some(obj) = s.as_object_mut() {
            obj.insert("type".into(), serde_json::json!([base, other]));
        }
        s
    }
}

/// 정수 인자에 붙인다: `#[serde(deserialize_with = "…")] #[schemars(schema_with = "…")]`
pub(super) use flex::{
    i64 as flex_i64, int_schema as flex_int_schema, str_opt_schema as flex_str_opt_schema,
    str_schema as flex_str_schema, str_vec_opt_schema as flex_str_vec_opt_schema,
    string as flex_string, string_opt as flex_string_opt, string_vec_opt as flex_string_vec_opt,
};

pub(super) fn one() -> i64 {
    1
}

pub(super) fn twenty() -> i64 {
    20
}

pub(super) fn box_pending() -> String {
    "pending".to_string()
}

pub(super) fn thirty() -> i64 {
    30
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 조회 도구가 문자열로 주는 ID를 쓰기 도구에 그대로 물려도 통과해야 한다.
    /// (`list_approval_lines` → `lineId:"2047"` → `save_approval_line.line_id`)
    #[test]
    fn 숫자_인자는_문자열도_받는다() {
        let a: approval::SaveApprovalLineArgs = serde_json::from_value(json!({
            "line_id": "2047", "form_id": "36", "line_nm": "x", "detail_line_json": "[]"
        }))
        .expect("문자열 ID를 받아야 한다");
        assert_eq!((a.line_id, a.form_id), (2047, 36));

        // 숫자도 그대로
        let b: approval::SaveApprovalLineArgs = serde_json::from_value(json!({
            "line_id": 2047, "form_id": 36, "line_nm": "x", "detail_line_json": "[]"
        }))
        .unwrap();
        assert_eq!((b.line_id, b.form_id), (2047, 36));

        // 숫자가 아닌 문자열은 여전히 거절 — 조용히 0으로 만들지 않는다
        assert!(serde_json::from_value::<approval::SaveApprovalLineArgs>(json!({
            "line_id": "abc", "form_id": 36, "line_nm": "x", "detail_line_json": "[]"
        }))
        .is_err());
    }

    /// 반대 방향 — 문자열 인자에 숫자를 줘도 통과해야 한다.
    /// (`read_approval_line.line_id`는 String인데 모델이 숫자를 찍기 쉽다)
    #[test]
    #[allow(non_snake_case)] // 이름 속 `ID` — 대문자를 살려야 뜻이 통하는 표기라 소문자로 풀지 않는다
    fn 문자열_ID_인자는_숫자도_받는다() {
        let a: approval::ReadApprovalLineArgs =
            serde_json::from_value(json!({ "line_id": 2047 })).unwrap();
        assert_eq!(a.line_id, "2047");
        let b: approval::ReadApprovalLineArgs =
            serde_json::from_value(json!({ "line_id": "2047" })).unwrap();
        assert_eq!(b.line_id, "2047");
    }

    /// 선택 인자(Option<String>)도 같은 규칙. `null`/생략은 None.
    #[test]
    fn 선택_문자열_인자도_숫자를_받는다() {
        let a: resource::UpdateArgs = serde_json::from_value(json!({
            "res_seq": 47, "seq_num": "71581", "res_idx": 2
        }))
        .unwrap();
        assert_eq!((a.res_seq.as_str(), a.seq_num, a.res_idx.as_deref()), ("47", 71581, Some("2")));

        let b: resource::UpdateArgs =
            serde_json::from_value(json!({ "res_seq": "47", "seq_num": 1 })).unwrap();
        assert_eq!(b.res_idx, None);
    }

    /// 스키마가 두 타입을 **광고**해야 한다 — 조용히 받아주기만 하면 모델은 계속 한쪽만 찍는다.
    #[test]
    fn 스키마가_integer와_string을_모두_노출한다() {
        let s = rmcp::schemars::schema_for!(approval::SaveApprovalLineArgs);
        let v = serde_json::to_value(&s).unwrap();
        let t = &v["properties"]["form_id"]["type"];
        assert_eq!(t, &json!(["integer", "string"]), "form_id 스키마: {t}");
        let t2 = &serde_json::to_value(rmcp::schemars::schema_for!(resource::CancelArgs)).unwrap()
            ["properties"]["res_seq"]["type"];
        assert_eq!(t2, &json!(["string", "integer"]), "res_seq 스키마: {t2}");
    }

    /// 참여자 목록은 `find_person`의 empSeq(문자열)와 모델이 쓰기 쉬운 숫자가 섞여 온다.
    #[test]
    fn 참여자_목록은_숫자와_문자열을_섞어_받는다() {
        let a: calendar::CreateEventArgs = serde_json::from_value(json!({
            "title": "회의", "start": 202608071100i64, "end": "202608071200",
            "participants": [3131, "송학현", "3137"]
        }))
        .expect("숫자 항목이 섞여도 통과해야 한다");
        assert_eq!(
            a.participants.unwrap(),
            vec!["3131".to_string(), "송학현".into(), "3137".into()]
        );
    }

    /// 한 명만 넣을 때 배열로 감싸지 않는 호출자를 받아준다.
    #[test]
    fn 참여자는_배열이_아니어도_한명으로_받는다() {
        let a: calendar::CreateEventArgs = serde_json::from_value(json!({
            "title": "회의", "start": "202608071100", "end": "202608071200",
            "participants": 3131
        }))
        .unwrap();
        assert_eq!(a.participants.unwrap(), vec!["3131".to_string()]);
    }

    #[test]
    fn 참여자_미지정은_없음이다() {
        let a: calendar::CreateEventArgs = serde_json::from_value(json!({
            "title": "회의", "start": "202608071100", "end": "202608071200"
        }))
        .unwrap();
        assert!(a.participants.is_none(), "미지정은 기존 동작(본인만) 유지");
    }

    #[test]
    fn 참여자_목록_스키마는_항목타입을_넓힌다() {
        let s = serde_json::to_value(rmcp::schemars::schema_for!(calendar::CreateEventArgs)).unwrap();
        let t = &s["properties"]["participants"]["items"]["type"];
        assert_eq!(t, &json!(["string", "integer"]), "participants 항목 스키마: {t}");
    }
}
