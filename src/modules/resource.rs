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
