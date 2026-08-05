//! 전자결재 상신/상신취소(쓰기) — `eap110A06`(상신) / `eap110A98`+`eap110A18`(상신취소).
//! 실측 캡처: `07-eapproval-api-capture.md` §8.8~§8.10.
//! ⚠️ **실제 결재가 발생**한다(결재요청·수신참조 통지가 나감). 테스트는 반드시 테스트 결재라인으로.
//!
//! 상신 흐름(팝업이 하던 일을 재현):
//!  1) approkey = "ERP_<uuid>" 생성.
//!  2) eap110A03(appLineId="")로 **양식필수 합의자(kyuljaeResult)+수신참조(m_Refer)** 획득.
//!  3) read_line(line_id)로 **개인결재라인 결재자** 획득.
//!  4) pTEAG_APPDOC_LINE = [합의자] + [결재자] (seq 재번호), pRefer = 수신참조.
//!  5) eap110A06 POST → resultData.result = 신규 docId.

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::modules::approval_line;

/// 상신취소 — eap110A98(사전조회) + eap110A18(실행). doc_id는 상신된 문서의 docId.
/// ⚠️ 필드명: 사전조회는 `docId`(소문자), 실행은 `docID`(대문자) — 실측 확정.
/// 성공 시 문서는 임시보관(doc_sts 10)으로 복귀하고 채번은 삭제된다.
pub async fn cancel_approval(c: &GwClient, doc_id: &str) -> Result<Value> {
    // ① 사전조회(현재 상태 확인)
    let pre = c
        .call(
            "/eap/eap110A98",
            &json!({ "docId": doc_id, "pageCode": "UBAP002" }),
        )
        .await?;
    let doc_sts = pre
        .get("doc_sts")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    // ② 실행(docID 대문자). 응답 resultData는 null이지만 resultCode 0이면 성공.
    c.call(
        "/eap/eap110A18",
        &json!({ "docID": doc_id, "pageCode": "UBAP002" }),
    )
    .await?;
    Ok(json!({
        "kind": "approvalCancelled",
        "docId": doc_id,
        "preDocSts": doc_sts,
        "note": "상신취소 실행 완료. 문서는 임시보관(doc_sts 10)으로 복귀하고 품의번호(채번)는 삭제됨. read_approval이 resultCode 2385(임시저장)로 실패하거나 approval_counts의 sent(상신)가 감소하면 취소 성공."
    }))
}

/// 페이로드의 신원 필드를 로그인 사용자 값으로 덮어쓴다(존재하는 키만 교체 — 새 키 추가 안 함).
/// draftHelp 예시 템플릿에 박힌 타인 신원(empCd/deptCd/coCd/이름)이 그대로 상신되는 것을 방지.
/// 부서명(deptNm)·직위·직책 등 세션이 모르는 표시문자열은 건드리지 않는다(cosmetic).
fn overwrite_if_present(v: &mut Value, key: &str, val: &str) {
    if let Some(obj) = v.as_object_mut() {
        if obj.contains_key(key) {
            obj.insert(key.to_string(), Value::String(val.to_string()));
        }
    }
}

fn inject_identity(item: &mut Value, co: &str, dept: &str, emp: &str, name: &str) {
    for (k, val) in [
        ("coCd", co), ("deptCd", dept), ("empCd", emp),
        ("empNm", name), ("empName", name), ("korNm", name),
    ] {
        overwrite_if_present(item, k, val);
    }
}

/// 문서 상신 — eap110A06. 근태 계열 양식(외근/연차 등) 대상.
/// - `form_id`: 양식 ID(41 외근/36 연차 …).
/// - `doc_title`: 문서 제목.
/// - `line_id`: 사용할 개인결재라인 ID(그 결재자들이 **결재(3000) 노드**로 실림). save_approval_line으로 준비.
/// - `bind_data_json`: KISS 폼 본문 데이터 JSON 텍스트(외근=`{"ITEMS":{...},"TABLE":{...}}`). 서버엔 이중인코딩되어 전송.
/// - `doc_contents_html`: 표시용 본문 HTML(raw). 내부에서 encodeURIComponent로 인코딩해 전송.
/// - `numbering_id`: 채번 규칙(기본 "1001").
///
/// 양식필수 합의자/수신참조는 eap110A03에서 서버가 해석한 것을 자동 병합한다.
#[allow(clippy::too_many_arguments)]
pub async fn submit_approval(
    c: &GwClient,
    form_id: i64,
    doc_title: &str,
    line_id: i64,
    hp_application_json: &str,
    bind_data_json: &str,
    doc_contents_html: &str,
    numbering_id: &str,
) -> Result<Value> {
    let co_id = c.comp_seq();
    let dept_id = c.dept_seq();
    let user_id = c.emp_seq();
    let user_nm = c.emp_name();

    // 신원 자동 주입값(ERP 코드 체계 — seq와 별개). hp/bind 페이로드의 신원 필드를 이 값으로 덮어씀.
    let id_co = c.co_cd().to_string();
    let id_dept = c.dept_cd().to_string();
    let id_emp = c.emp_cd().to_string();
    let id_name = c.emp_name().to_string();

    // bindData 검증(유효 JSON이어야 함)
    let mut bind_obj: Value = serde_json::from_str(bind_data_json)
        .map_err(|e| anyhow!("bind_data_json이 유효한 JSON이 아님: {e}"))?;
    // 신원 자동 주입: bindData ITEMS의 신원 표시필드(empNm 등)를 로그인 사용자 값으로.
    if let Some(items) = bind_obj.get_mut("ITEMS") {
        inject_identity(items, &id_co, &id_dept, &id_emp, &id_name);
    }
    // 이중 인코딩: 최종 wire 값 = JSON.stringify(JSON.stringify(bindObj)).
    let s1 = serde_json::to_string(&bind_obj)?; // {"ITEMS":...}
    let bind_data_field = Value::String(serde_json::to_string(&s1)?); // "{\"ITEMS\":...}"

    let numbering_id = if numbering_id.trim().is_empty() {
        "1001"
    } else {
        numbering_id.trim()
    };
    let approkey = gen_approkey();

    // ── 0) HP 근태 신청 저장 (2-phase의 1단계, eap110A06가 참조할 근태 레코드 생성) ──
    // 근태 양식(HPD0110)은 상신(eap110A06) 전에 신청완료가 이 콜로 HP draft를 먼저 만든다.
    // 이 단계를 건너뛰면 eap110A06 연동이 resultCode 2099(HP_HPD0110)로 실패한다.
    if !hp_application_json.trim().is_empty() {
        let mut hp_body: Value = serde_json::from_str(hp_application_json)
            .map_err(|e| anyhow!("hp_application_json이 유효한 JSON이 아님: {e}"))?;
        // 신원 자동 주입: applicationList/employeeList 각 항목의 coCd/deptCd/empCd/이름을 로그인 사용자 값으로.
        for key in ["applicationList", "employeeList"] {
            if let Some(list) = hp_body.get_mut(key).and_then(|v| v.as_array_mut()) {
                for it in list.iter_mut() {
                    inject_identity(it, &id_co, &id_dept, &id_emp, &id_name);
                }
            }
        }
        c.call("/human/attendapplication/0hr00011", &hp_body)
            .await
            .map_err(|e| anyhow!("HP 근태신청 저장(0hr00011) 실패: {e}"))?;
    }

    // ── 1) eap110A03: 양식필수 합의자 + 수신참조 해석 ─────────────────────────
    let a03 = c
        .call(
            "/eap/eap110A03",
            &json!({
                "docID": 0, "formID": form_id.to_string(), "approkey": approkey,
                "appLineId": "", "draftTp": "", "reDraft": "", "docType": "",
                "doc_auth": 0, "pageCode": "UBAP001"
            }),
        )
        .await?;
    let result_map = a03.get("resultMap").cloned().unwrap_or(Value::Null);
    let kyuljae = result_map
        .get("kyuljaeResult")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let m_refer = result_map
        .get("m_Refer")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // ── 2) 개인결재라인 결재자 ────────────────────────────────────────────────
    let line = approval_line::read_line(c, &line_id.to_string()).await?;
    let members = line
        .get("members")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    if members.is_empty() {
        return Err(anyhow!("결재라인 {line_id}에 결재자가 없음"));
    }

    // ── 3) pTEAG_APPDOC_LINE = [합의자(kyuljae)] + [결재자(개인라인)] ─────────
    let mut line_nodes: Vec<Value> = Vec::new();
    for src in kyuljae.iter() {
        line_nodes.push(norm_line_node(src, 4000, &co_id));
    }
    for src in members.iter() {
        line_nodes.push(norm_line_node(src, 3000, &co_id));
    }
    // seq / doc_line_*_seq 를 1-base 로 재번호(합의 먼저, 결재 다음)
    for (i, n) in line_nodes.iter_mut().enumerate() {
        let seq = i as i64 + 1;
        if let Some(o) = n.as_object_mut() {
            o.insert("seq".into(), json!(seq));
            o.insert("doc_line_mseq".into(), json!(seq));
            o.insert("doc_line_m_seq".into(), json!(seq));
            o.insert("doc_line_sseq".into(), json!(1));
            o.insert("doc_line_s_seq".into(), json!(1));
        }
    }

    // ── 4) pRefer = 수신참조(부서) ────────────────────────────────────────────
    let mut refer_nodes: Vec<Value> = Vec::new();
    for (i, src) in m_refer.iter().enumerate() {
        refer_nodes.push(norm_refer(src, i as i64 + 1, &co_id));
    }

    // ── 5) modifyDocInfo compact 뷰 ──────────────────────────────────────────
    let line_compact: Vec<Value> = line_nodes
        .iter()
        .map(|n| {
            json!({
                "doc_line_m_seq": n.get("doc_line_m_seq").cloned().unwrap_or(json!(0)),
                "doc_line_s_seq": 1,
                "act_id": n.get("act_id").cloned().unwrap_or(json!(3000)),
                "co_id": n.get("co_id").cloned().unwrap_or(json!(co_id)),
                "dept_id": n.get("dept_id").cloned().unwrap_or(Value::Null),
                "user_id": n.get("user_id").cloned().unwrap_or(Value::Null),
                "doc_line_gb": "1"
            })
        })
        .collect();
    let receive_list: Vec<Value> = refer_nodes
        .iter()
        .map(|n| {
            json!({
                "receive_div": "10",
                "org_div": "d",
                "org_id": n.get("org_id").cloned().unwrap_or(Value::Null)
            })
        })
        .collect();

    let doc_contents = encode_uri_component(doc_contents_html);
    let rep_dt = now_kst_datetime();

    // ── 6) eap110A06 상신 ────────────────────────────────────────────────────
    let param_item = json!({
        "bindData": bind_data_field,
        "interDivId": "divInterJson", "interDocTp": "json",
        "doc_id": 0, "form_id": form_id.to_string(), "numbering_id": numbering_id,
        "rep_dt": rep_dt, "repdt_mod_yn": "0",
        "co_id": co_id, "dept_id": dept_id, "biz_id": co_id, "user_id": user_id,
        "co_nm": "(주)이노그리드", "dept_nm": "", "user_nm": user_nm,
        "doc_title": doc_title, "doc_sts": "20", "inservice_time": "0",
        "doc_level": "001", "emergency_level": "1", "doc_security": "0", "use_yn": "1",
        "approkey": approkey, "contents_tp": "10", "doc_contents": doc_contents,
        "pTEAG_APPDOC_LINE": line_nodes,
        "pVKD_TKDDITEM": [], "pVCM_ATTACHFILEINFO": [],
        "pRefer": refer_nodes, "pReceive": [], "pOper": [], "pTEAG_APPDOC_REF": [],
        "pTEAG_TOC_FOLDER": "", "pDraftTp": "", "seal_use_yn": "", "receipient": "",
        "receipt": "", "iframeHtml": "", "re_draft": "",
        "modifyAppLineYn": "Y", "modifyReceive10": "Y", "modifyReceive20": "Y",
        "modifyReceive30": "Y", "modifyReceive40": "Y", "modifyTitle": "Y",
        "modifyContent": "Y", "modifyRef": "Y", "modifyAttach": "Y", "modifyAddItem": "Y",
        "modifyInservice": "Y", "modifyDoclevel": "Y", "modifyEmergency": "Y",
        "modifySeal": "Y", "modifyEabox": "Y", "modifyFileList": "",
        "delFileSnList": [], "auditorYn": "0",
        "modifyDocInfo": {
            "docId": 0,
            "appdoc": {
                "inservice_time": "0", "doc_level": "001", "doc_security": "0",
                "emergency_level": "1", "doc_title": doc_title
            },
            "appdocReceiveList": receive_list,
            "appdocLineList": line_compact,
            "appdocFileList": [], "appdocFolderList": [{ "menu_id": "" }], "appdocRefList": []
        },
        "modifyItemList": Value::Null, "isLatestVerContentsFile": true,
        "versionCheck": Value::Null, "formLang": "kr", "aiVerifyHistories": [],
        "aiVerifyAutoOnSubmit": false, "aiVerifyUseYn": "0"
    });
    let d = c
        .call("/eap/eap110A06", &json!({ "paramItem": param_item, "pageCode": "UBAP001" }))
        .await?;

    let new_doc_id = d
        .get("result")
        .cloned()
        .unwrap_or(Value::Null);
    Ok(json!({
        "kind": "approvalSubmitted",
        "docId": new_doc_id,
        "formId": form_id,
        "title": doc_title,
        "lineCount": line_nodes.len(),
        "referCount": refer_nodes.len(),
        "note": "상신 응답을 성공으로 단정 말 것. ⚠️ 일부 양식(특히 근태/외근/연차 등 HP 연동 양식)은 draft(임시보관)에 같은 form_id 문서가 남아있으면 상신이 조용히 막히거나 2099로 실패한다. list_approvals(box_name:\"sent\")로 이 문서가 실제로 떴는지 확인하고, sent 목록에 없으면 list_approvals(box_name:\"draft\")로 임시보관을 조회해 delete_temp_approval로 정리한 뒤 재시도하라. 취소는 cancel_approval(docId)."
    }))
}

/// 임시보관 전자결재 문서 삭제 — `GET /eap/sse/eap107A25?docIdList=<csv>`(SSE 스트림).
/// 콤마구분 docId를 한 콜로 일괄삭제. 같은 form_id의 잔여 임시보관 문서가 신규 상신을 막을 때
/// 정리용(07 §8.11). 응답 이벤트별 resultCode + resultData.failCnt로 성공 판정.
pub async fn delete_temp_approval(c: &GwClient, doc_ids: &str) -> Result<Value> {
    let ids = doc_ids.trim();
    if ids.is_empty() {
        anyhow::bail!("doc_ids(콤마구분 docId)가 비어있음");
    }
    let path = format!("/eap/sse/eap107A25?docIdList={ids}");
    let events = c.call_get_sse("/eap/sse/eap107A25", &path).await?;

    let mut deleted: Vec<String> = Vec::new();
    let mut fail: i64 = 0;
    for e in &events {
        let code = e.get("resultCode").and_then(|v| v.as_i64()).unwrap_or(-1);
        if !(code == 0 || code == 200) {
            fail += 1;
            continue;
        }
        if let Some(rd) = e.get("resultData") {
            fail += rd.get("failCnt").and_then(|v| v.as_i64()).unwrap_or(0);
            if let Some(id) = rd.get("docId").and_then(|v| v.as_str()) {
                if !id.is_empty() {
                    deleted.push(id.to_string());
                }
            }
        }
    }
    Ok(json!({
        "kind": "tempApprovalDeleted",
        "requested": ids,
        "deletedDocIds": deleted,
        "failCount": fail,
        "note": "임시보관 문서 삭제(eap107A25). list_approvals(box_name:\"draft\")로 사라졌는지 재확인 권장."
    }))
}

/// eap110A06 결재선 노드 정규화 — 원본(kyuljaeResult 또는 eap102A05 member)에서
/// 필드를 뽑아 서버가 기대하는 형태로 채운다. seq류는 호출부에서 재번호.
fn norm_line_node(src: &Value, default_act: i64, co_id: &str) -> Value {
    let ss = |k: &str, d: &str| -> String {
        match src.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => d.to_string(),
        }
    };
    let num = |k: &str| src.get(k).cloned().unwrap_or(json!(0));
    let act_id = src.get("act_id").and_then(|v| v.as_i64()).unwrap_or(default_act);
    let act_nm = if act_id == 4000 { "합의" } else { "결재" };
    let uid = {
        let u = ss("user_id", "");
        if u.is_empty() { ss("org_id", "") } else { u }
    };
    let div = {
        let d = ss("div", "");
        if d.is_empty() { ss("org_div", "m") } else { d }
    };
    json!({
        "div": div, "org_div": div, "org_id": uid, "user_id": uid,
        "doc_line_gb": "1", "act_id": act_id, "act_nm": act_nm, "act_type": "10",
        "work_order": 1, "act_order": act_id,
        "co_id": co_id, "co_nm": ss("co_nm", "(주)이노그리드"),
        "biz_id": ss("biz_id", co_id), "biz_nm": ss("biz_nm", "(주)이노그리드"),
        "dept_id": ss("dept_id", ""), "dept_nm": ss("dept_nm", ""), "dept_nm_disp": ss("dept_nm", ""),
        "user_nm": ss("user_nm", ""),
        "grade_cd": ss("grade_cd", ""), "grade_nm": ss("grade_nm", ""), "grade_order": num("grade_order"),
        "duty_cd": ss("duty_cd", ""), "duty_nm": ss("duty_nm", ""), "duty_order": num("duty_order"),
        "arbitary_yn": "0", "app_yn": "0", "must_yn": "0", "deptline_yn": "0",
        "login_id": ss("login_id", ""), "path_name": ss("path_name", ""),
        "work_status": "999", "dp_nm_disp": Value::Null, "dept_line": false, "draftTp": ""
    })
}

/// eap110A06 수신참조(pRefer) 노드 정규화 — m_Refer(부서) 원본에서.
fn norm_refer(src: &Value, seq: i64, co_id: &str) -> Value {
    let ss = |k: &str, d: &str| -> String {
        match src.get(k) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Number(n)) => n.to_string(),
            _ => d.to_string(),
        }
    };
    let dept_id = {
        let d = ss("dept_id", "");
        if d.is_empty() { ss("org_id", "") } else { d }
    };
    let dept_nm = ss("dept_nm", "");
    json!({
        "doc_line_gb": "1", "org_div": "d", "div": "d",
        "act_id": 5000, "act_nm": "수신참조", "act_type": "40", "act_order": 5000,
        "deptline_yn": "1", "dept_line": true,
        "user_nm": if dept_nm.is_empty() { ss("user_nm", "") } else { dept_nm.clone() }, "user_id": "0",
        "org_id": dept_id, "dept_id": dept_id, "dept_nm": dept_nm, "dept_nm_disp": ss("dept_nm", ""),
        "co_id": co_id, "co_nm": ss("co_nm", "(주)이노그리드"),
        "biz_id": co_id, "biz_nm": ss("biz_nm", "(주)이노그리드"),
        "seq": seq, "work_order": 1,
        "doc_line_m_seq": seq, "doc_line_mseq": seq, "doc_line_s_seq": 1, "doc_line_sseq": 1,
        "arbitary_yn": "0", "app_yn": "0", "must_yn": "0",
        "grade_cd": "", "grade_nm": "", "grade_order": Value::Null,
        "duty_cd": "", "duty_nm": "", "duty_order": Value::Null,
        "login_id": "", "path_name": ss("path_name", ""), "work_status": "", "dp_nm_disp": Value::Null
    })
}

/// approkey = "ERP_<uuid4-ish>" — 16 랜덤바이트를 uuid 포맷으로.
fn gen_approkey() -> String {
    let b: [u8; 16] = rand::random();
    format!(
        "ERP_{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// JS encodeURIComponent 동등 — A-Za-z0-9 와 `-_.!~*'()` 만 남기고 UTF-8 바이트를 %XX 로.
fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for &byte in s.as_bytes() {
        let keep = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')');
        if keep {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push_str(&format!("{byte:02X}"));
        }
    }
    out
}

/// 현재 KST(UTC+9) "YYYY-MM-DD HH:MM:SS".
fn now_kst_datetime() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + 9 * 3600;
    let days = secs / 86400;
    let tod = secs % 86400;
    let (y, m, d) = days_to_ymd(days);
    let (hh, mm, ss) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}:{ss:02}")
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
