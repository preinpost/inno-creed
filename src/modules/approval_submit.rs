//! 전자결재 상신/상신취소(쓰기) — `eap110A06`(상신) / `eap110A98`+`eap110A18`(상신취소).
//! 실측 캡처: `07-eapproval-api-capture.md` §8.8~§8.10.
//! ⚠️ **실제 결재가 발생**한다(결재요청·수신참조 통지가 나감). 테스트는 반드시 테스트 결재라인으로.
//!
//! 상신 흐름(팝업이 하던 일을 재현):
//!  0) approkey = "ERP_<uuid>" 생성(클라 생성이 맞음 — 서버 발급 토큰 아님).
//!  1) 근태 양식: 0hr00011(검증/스테이징) → create(HP신청 커밋 → appSq/appDt 반환).
//!  2) eap110A03(appLineId=line_id)로 **완전 병합된 결재선(kyuljaeResult=양식필수 합의+개인라인 결재)
//!     + 수신참조(m_Refer) + 시행자(m_Oper) + form_info.form_d_tp(양식별 interlock 식별자)** 획득.
//!  3) 근태 양식: HP interlock 등록 3콜 — GetLinkKey(→linkKey) → saveAttendApplicationLinkKey(linkKey↔appSq 바인딩)
//!     → SetEnageGroup(approKey에 linkKey·콜백API 등록). ⭐ **이게 2099의 핵심**(아래).
//!  4) pTEAG_APPDOC_LINE = kyuljaeResult 무가공, pRefer = m_Refer(+org_div), pOper = m_Oper(+org_div).
//!     — 이 셋은 성공 브라우저 상신 payload와 바이트 동일(실측 대조).
//!  5) eap110A06 POST → resultData.result = 신규 docId.
//!
//! ⚠️ **2099(HP_HPD0110_000XX)의 원인은 interlock 등록 3콜 누락**이다(§10.19~§10.20, 2026-08-05 무필터 전량 캡처로 확정).
//!    eap110A06의 eap→HP 서버간 연동은 approKey에 등록된 linkKey를 찾는데, 등록이 없으면 대상이 없어 HP가 500을 준다.
//!    (GetLinkKey/SetEnageGroup 누락 → "Internal Server Error", saveAttendApplicationLinkKey 누락 → "종결 처리 오류".)
//!    초기 캡처가 `/human/`·`/eap/`만 필터링해 `/system/apiUtilEap/*`·`/personal/hpd0110/*`를 통째로 놓친 게
//!    장기 오진의 원인이었다. **반증된 가설(재도입 금지)**: payload/pOper 누락, 잔여 임시 draft·대기신청 충돌,
//!    날짜, doc_sts(10/20), eap prep 콜, 쿠키·토큰·헤더·전송계층 지문, 포털로그인(gw050B01) 세션 — 전부 실측 반증.
//!    HP↔eap 링크도 "서버가 empCd+atDt로 매칭"이 아니라 **linkKey↔appSq 명시 바인딩**이다(§10.20).

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::util::days_to_ymd;

/// 상신 문서 취소 — 상태(doc_sts)에 따라 결재취소→상신취소→(옵션)임시보관삭제를 순차 실행.
/// 상태 전이(실측): 30(결재 진행중) --eap110A54 결재취소--> 20(상신) --eap110A18 상신취소--> 10(임시보관) --eap110A19 삭제--> 소멸.
/// ⚠️ 필드명: 사전조회 eap110A98은 `docId`(소문자), 실행 콜들은 `docID`(대문자) — 실측 확정.
/// doc_sts 30이면 결재취소가 선행돼야 하고 그 콜(eap110A54)은 `form_id`를 요구한다(eap110A98 응답엔 없음 → caller가 list_approvals의 formId 전달).
/// purge=true면 임시보관(10)까지 되돌린 뒤 eap110A19로 완전 삭제. false면 임시보관에 남는다.
pub async fn cancel_approval(
    c: &GwClient,
    doc_id: &str,
    form_id: &str,
    purge: bool,
) -> Result<Value> {
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
    let mut steps: Vec<&str> = Vec::new();

    // ② 결재취소(진행중 30 → 상신 20). eap110A54는 formID 필요.
    if doc_sts == "30" {
        if form_id.trim().is_empty() {
            anyhow::bail!(
                "doc_sts=30(결재 진행중) 문서는 결재취소(eap110A54)가 선행돼야 하며 form_id가 필요합니다 — list_approvals의 formId를 넘기세요."
            );
        }
        c.call(
            "/eap/eap110A54",
            &json!({ "docID": doc_id, "formID": form_id, "actID": "", "pageCode": "UBAP002" }),
        )
        .await
        .map_err(|e| anyhow!("결재취소(eap110A54) 실패: {e}"))?;
        steps.push("결재취소(eap110A54)");
    }

    // ③ 상신취소(상신 20 → 임시보관 10). 응답 resultData는 null이지만 resultCode 0이면 성공.
    if doc_sts == "30" || doc_sts == "20" {
        c.call(
            "/eap/eap110A18",
            &json!({ "docID": doc_id, "pageCode": "UBAP002" }),
        )
        .await
        .map_err(|e| anyhow!("상신취소(eap110A18) 실패: {e}"))?;
        steps.push("상신취소(eap110A18)");
    }

    // ④ (옵션) 임시보관 삭제(10 → 소멸).
    if purge {
        c.call(
            "/eap/eap110A19",
            &json!({ "docID": doc_id, "pageCode": "UBAP001" }),
        )
        .await
        .map_err(|e| anyhow!("임시보관 삭제(eap110A19) 실패: {e}"))?;
        steps.push("임시보관삭제(eap110A19)");
    }

    Ok(json!({
        "kind": "approvalCancelled",
        "docId": doc_id,
        "preDocSts": doc_sts,
        "steps": steps,
        "purged": purge,
        "note": "취소 실행 완료. purge=false면 문서는 임시보관(doc_sts 10)으로 복귀(채번 삭제), purge=true면 완전 삭제됨. 검증: read_approval이 2385(임시저장)면 임시보관 복귀, approval_counts의 sent 감소면 상신취소 성공, list_approvals(draft)에서 사라졌으면 삭제 성공."
    }))
}

/// 페이로드의 신원 필드를 로그인 사용자 값으로 덮어쓴다(존재하는 키만 교체 — 새 키 추가 안 함).
/// draftHelp 예시 템플릿에 박힌 타인 신원(empCd/deptCd/coCd/이름)이 그대로 상신되는 것을 방지.
/// 빈 문자열은 덮어쓰지 않는다 — 조직도 조회 실패 시 예시값을 지워버리는 것보다 남기는 편이 낫다.
fn overwrite_if_present(v: &mut Value, key: &str, val: &str) {
    if val.is_empty() {
        return;
    }
    if let Some(obj) = v.as_object_mut()
        && obj.contains_key(key)
    {
        obj.insert(key.to_string(), Value::String(val.to_string()));
    }
}

/// 로그인 사용자의 신원·표시정보. 코드계(co/dept/emp)는 세션에서, 표시문자열(부서명/직책/직급)은
/// `org::my_profile`(조직도 1콜, 30분 캐시)에서 온다. 조직도가 안 잡히면 표시문자열만 빈 값이 되고
/// 그 필드는 예시값이 유지된다(상신은 그대로 진행).
struct Identity {
    co: String,
    dept: String,
    emp: String,
    name: String,
    dept_nm: String,
    duty: String,
    position: String,
    co_nm: String,
}

/// 신원 코드 + **문서에 렌더되는 표시문자열**까지 주입한다.
/// ⚠️ 표시필드를 안 채우면 draftHelp 예시에 박힌 **타인의 이름·부서·직급이 결재문서 본문에 그대로
/// 찍힌다**(예시 작성자 기준값). cosmetic이 아니라 실제 출력값이라 반드시 덮어쓴다.
/// 대상 필드(존재할 때만): 코드계 `coCd/deptCd/empCd`, 이름 `empNm/empName/korNm`,
/// 부서명 `deptNm/deptName/singleDeptNm`, 회사명 `divNm`, 직급 `singlePositionNm`,
/// 직책 `singleDutyNm`, 조합문자열 `empNmDutyNm`("이름 직책")·`employees`("이름 직급").
/// `employees`는 신청 대상자 목록이지만 MCP 상신은 항상 **본인 1인** 기준이라 단일값으로 채운다.
fn inject_identity(item: &mut Value, id: &Identity) {
    let emp_duty = if id.duty.is_empty() {
        String::new()
    } else {
        format!("{} {}", id.name, id.duty)
    };
    let emp_position = if id.position.is_empty() {
        String::new()
    } else {
        format!("{} {}", id.name, id.position)
    };
    for (k, val) in [
        ("coCd", id.co.as_str()), ("deptCd", id.dept.as_str()), ("empCd", id.emp.as_str()),
        ("empNm", id.name.as_str()), ("empName", id.name.as_str()), ("korNm", id.name.as_str()),
        ("deptNm", id.dept_nm.as_str()), ("deptName", id.dept_nm.as_str()),
        ("singleDeptNm", id.dept_nm.as_str()), ("divNm", id.co_nm.as_str()),
        ("singlePositionNm", id.position.as_str()), ("singleDutyNm", id.duty.as_str()),
        ("empNmDutyNm", emp_duty.as_str()), ("employees", emp_position.as_str()),
    ] {
        overwrite_if_present(item, k, val);
    }
}

/// 문서 상신 — eap110A06. 근태 계열 양식(외근/연차 등) 대상.
/// - `form_id`: 양식 ID(41 외근/36 연차 …).
/// - `doc_title`: 문서 제목.
/// - `line_id`: 사용할 개인결재라인 ID. a03에 appLineId로 넘겨 완전 병합된 결재선을 받는다. save_approval_line으로 준비.
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

    // 신원 자동 주입값. 코드계(ERP — seq와 별개)는 세션에서, 표시문자열(부서명/직책/직급)은
    // 조직도 1콜(30분 캐시)에서. hp/bind 페이로드의 해당 필드를 이 값으로 덮어쓴다.
    let prof = crate::modules::org::my_profile(c).await;
    let ps = |k: &str| prof.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let id = Identity {
        co: c.co_cd().to_string(),
        dept: c.dept_cd().to_string(),
        emp: c.emp_cd().to_string(),
        name: c.emp_name().to_string(),
        dept_nm: ps("deptName"),
        duty: ps("duty"),
        position: ps("position"),
        co_nm: ps("coName"),
    };

    // bindData 검증(유효 JSON이어야 함)
    let mut bind_obj: Value = serde_json::from_str(bind_data_json)
        .map_err(|e| anyhow!("bind_data_json이 유효한 JSON이 아님: {e}"))?;
    // 신원 자동 주입: bindData ITEMS의 신원 표시필드(empNm 등)를 로그인 사용자 값으로.
    if let Some(items) = bind_obj.get_mut("ITEMS") {
        inject_identity(items, &id);
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

    // create가 반환하는 HP 신청 식별자(appSq/appDt) — interlock linkKey 바인딩에 필요.
    let mut app_sq: Option<i64> = None;
    let mut app_dt = String::new();

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
                    inject_identity(it, &id);
                }
            }
        }
        let create_body = json!({
            "coCd": "", "appDt": "", "appEmpCd": id.emp, "deptCd": "",
            "titleDc": doc_title, "approLineId": line_id.to_string(),
            "calLinkKey": "", "linkKey": "", "approState": "", "fileGroup": 0, "version": "v2",
            "employeeList": hp_body.get("employeeList").cloned().unwrap_or(json!([])),
            "applicationList": hp_body.get("applicationList").cloned().unwrap_or(json!([])),
        });
        // 1단계: 0hr00011 (검증/스테이징). 응답은 빈 SUCCESS.
        c.call("/human/attendapplication/0hr00011", &hp_body)
            .await
            .map_err(|e| anyhow!("HP 근태신청 저장(0hr00011) 실패: {e}"))?;
        // 2단계: create (HP신청 커밋 — approLineId에 묶인 대기 HP신청 등록, appSq 반환).
        let create_res = c.call("/human/attendapplication/create", &create_body)
            .await
            .map_err(|e| anyhow!("HP 근태신청 커밋(create) 실패: {e}"))?;
        app_sq = create_res.get("appSq").and_then(|v| v.as_i64());
        app_dt = create_res.get("appDt").and_then(|v| v.as_str()).unwrap_or("").to_string();
    }

    // ── 1) eap110A03: 결재선 해석 + form_d_tp(양식별 interlock 식별자) 취득 ─────
    // interlock 등록(SetEnageGroup)이 form_d_tp를 요구하므로 a03를 먼저 호출해 얻는다.
    let a03 = c
        .call(
            "/eap/eap110A03",
            &json!({
                "docID": 0, "formID": form_id.to_string(), "approkey": approkey,
                "appLineId": line_id.to_string(), "draftTp": "", "reDraft": "", "docType": "",
                "doc_auth": 0, "pageCode": "UBAP001"
            }),
        )
        .await?;
    let result_map = a03.get("resultMap").cloned().unwrap_or(Value::Null);
    // form_d_tp = 양식별 HP interlock 식별자(연차36 _00011 / 출장40 _00021 / 외근41 _00031 / 휴일43 _00051).
    // 하드코딩 금지 — 양식마다 다르다(틀리면 eap110A06가 HP_HPD0110_000XX로 2099). a03가 formID 기준으로 반환.
    let form_d_tp = result_map
        .get("form_info")
        .and_then(|f| f.get("form_d_tp"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("HP_HPD0110_00011")
        .to_string();

    // ── 2) 근태 interlock 등록 (eap110A06 성공의 핵심) ──────────────────────
    // eap110A06의 eap→HP 서버간 연동은 이 3콜 등록을 요구한다. 누락 시:
    //   · GetLinkKey/SetEnageGroup 없으면 → HP_HPD0110_000XX "Internal Server Error"(연동 대상 없음)
    //   · saveAttendApplicationLinkKey(linkKey↔appSq 바인딩) 없으면 → "근태신청서 종결 처리 오류"
    // 브라우저는 이 콜들을 치지만 /system//personal/ 경로라 초기 캡처(/human//eap/만)가 놓쳤던 조각. (§10.19)
    // menuCode(HPD0110)는 근태 공통 상수지만 formDTp는 양식별(위 form_d_tp). 콜백 API는 eap가 상신 시 서버간 호출하는 HP 엔드포인트.
    if !hp_application_json.trim().is_empty() {
        let glk = c
            .call(
                "/system/apiUtilEap/GetLinkKey",
                &json!({"menuCode":"HPD0110","approKey":approkey,"vPCoCd":id.co,"coCd":id.co}),
            )
            .await
            .map_err(|e| anyhow!("GetLinkKey 실패: {e}"))?;
        let link_key = glk.get("linkKey").and_then(|v| v.as_str()).unwrap_or("").to_string();
        // linkKey ↔ 실제 HP 신청(appSq) 바인딩. 없으면 finalize가 대상 신청을 못 찾아 '종결 처리 오류'.
        c.call(
            "/personal/hpd0110/saveAttendApplicationLinkKey",
            &json!({"linkKey": link_key, "appSq": app_sq, "coCd": id.co, "appDt": app_dt}),
        )
        .await
        .map_err(|e| anyhow!("saveAttendApplicationLinkKey 실패: {e}"))?;
        c.call(
            "/system/apiUtilEap/SetEnageGroup",
            &json!({
                "approKey": approkey, "formDTp": form_d_tp, "formId": form_id.to_string(),
                "linkKey": link_key, "formNm": doc_title, "docTitle": doc_title, "contents": "",
                "contentsApi": "/human/attendapplication/interlock/getInterlockFormContents",
                "statusApi": "/human/attendapplication/interlock/setInterlockSync",
                "dummy1": "", "link": "", "vPCoCd": id.co, "coCd": id.co
            }),
        )
        .await
        .map_err(|e| anyhow!("SetEnageGroup 실패: {e}"))?;
    }

    // ── 3) 결재선(양식필수 합의자/수신참조/시행자) 해석 — 위 a03의 result_map 재사용 ──
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
    // 필수 시행자(m_Oper) — 브라우저 성공 payload와 동일하게 pOper로 그대로 실어 보낸다(정합성).
    // ※ "pOper 누락이 2099의 원인"이라는 이전 가설은 반증됨(§8.14) — 원인은 interlock 등록 누락.
    let m_oper = result_map
        .get("m_Oper")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // ── 2) pTEAG_APPDOC_LINE = kyuljaeResult 원본 그대로 ──────────────────────
    // a03에 appLineId를 주면 kyuljaeResult가 [양식필수 합의 + 개인결재라인]까지 완전히 병합된
    // 결재선으로 온다. 브라우저는 이걸 무가공으로 pTEAG_APPDOC_LINE에 실어 보낸다(실측 확인).
    // 직접 재구성(act_id 강제/read_line 병합)하면 브라우저와 어긋나므로 그대로 패스스루한다.
    if kyuljae.is_empty() {
        return Err(anyhow!(
            "a03가 결재선(kyuljaeResult)을 반환하지 않음 — 결재라인 {line_id} 확인 필요"
        ));
    }
    let line_nodes: Vec<Value> = kyuljae.clone();

    // ── 3) pRefer = 수신참조, pOper = 시행자 — a03 원본 패스스루(+org_div) ──────
    let refer_nodes: Vec<Value> = m_refer.iter().map(norm_participant).collect();
    let oper_nodes: Vec<Value> = m_oper.iter().map(norm_participant).collect();

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
    // appdocReceiveList: 시행자(40) + 수신참조(10). org_div/org_id는 각 노드 원본에서. (실측)
    let recv_of = |n: &Value, div: &str| {
        json!({
            "receive_div": div,
            "org_div": n.get("org_div").cloned().unwrap_or(Value::Null),
            "org_id": n.get("org_id").cloned().unwrap_or(Value::Null)
        })
    };
    let mut receive_list: Vec<Value> = Vec::new();
    for n in oper_nodes.iter() {
        receive_list.push(recv_of(n, "40"));
    }
    for n in refer_nodes.iter() {
        receive_list.push(recv_of(n, "10"));
    }

    let doc_contents = encode_uri_component(doc_contents_html);
    let rep_dt = now_kst_datetime();

    // ── 6) eap110A06 상신 ────────────────────────────────────────────────────
    let param_item = json!({
        "bindData": bind_data_field,
        "interDivId": "divInterJson", "interDocTp": "json",
        "doc_id": 0, "form_id": form_id.to_string(), "numbering_id": numbering_id,
        "rep_dt": rep_dt, "repdt_mod_yn": "0",
        "co_id": co_id, "dept_id": dept_id, "biz_id": co_id, "user_id": user_id,
        // dept_nm: 브라우저는 기안부서명을 싣는다(§10.17 diff의 유일한 차이였음) — 조직도 값으로 채운다.
        "co_nm": "(주)이노그리드", "dept_nm": id.dept_nm, "user_nm": user_nm,
        "doc_title": doc_title, "doc_sts": "20", "inservice_time": "0",
        "doc_level": "001", "emergency_level": "1", "doc_security": "0", "use_yn": "1",
        "approkey": approkey, "contents_tp": "10", "doc_contents": doc_contents,
        "pTEAG_APPDOC_LINE": line_nodes,
        "pVKD_TKDDITEM": [], "pVCM_ATTACHFILEINFO": [],
        "pRefer": refer_nodes, "pReceive": [], "pOper": oper_nodes, "pTEAG_APPDOC_REF": [],
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
        "note": "상신 응답을 성공으로 단정 말 것 — list_approvals(box_name:\"sent\")로 실제 접수 확인, 취소는 cancel_approval(docId). 근태 양식은 create→GetLinkKey→saveAttendApplicationLinkKey→SetEnageGroup(HP interlock 등록) 후 eap110A06으로 상신한다(§10.19). 등록 누락 시 2099(HP_HPD0110). result=null이면 상신 실패이므로 note가 아니라 docId 유무로 판정."
    }))
}

/// 임시보관 전자결재 문서 삭제 — `GET /eap/sse/eap107A25?docIdList=<csv>`(SSE 스트림).
/// 콤마구분 docId를 한 콜로 일괄삭제. 상신취소(purge=false)로 되돌아온 문서나 시험 잔여물 정리용
/// (07 §8.11.2). ⚠️ 상신 실패(2099)와는 무관 — 잔여 draft 원인설은 반증됨(§10.6).
/// 응답 이벤트별 resultCode + resultData.failCnt로 성공 판정.
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
            if let Some(id) = rd.get("docId").and_then(|v| v.as_str())
                && !id.is_empty()
            {
                deleted.push(id.to_string());
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

/// a03의 m_Oper/m_Refer 원본 노드를 pOper/pRefer 상신 노드로 패스스루.
/// 실측(브라우저 캡처) 확인: a03 노드는 org_id/dept_line/seq/doc_line_* 가 이미 정확하고,
/// 브라우저는 딱 하나 `org_div = div` 만 추가해 그대로 보낸다. 그 외 재구성은 하지 않는다.
/// (개인 시행자/참조자를 부서노드로 강제 변환하던 이전 로직은 브라우저와 어긋나 폐기 — 2099와는 무관했음.)
fn norm_participant(src: &Value) -> Value {
    let mut n = src.clone();
    if let Some(o) = n.as_object_mut() {
        let div = o.get("div").and_then(|v| v.as_str()).unwrap_or("m").to_string();
        o.insert("org_div".into(), json!(div));
    }
    n
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

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Identity {
        Identity {
            co: "1000".into(), dept: "BB999".into(), emp: "22222".into(), name: "김철수".into(),
            dept_nm: "인프라팀".into(), duty: "팀장".into(), position: "부장".into(),
            co_nm: "(주)이노그리드".into(),
        }
    }

    /// draftHelp 예시(출장 ITEMS)에 박힌 타인 표시값이 전부 로그인 사용자 값으로 바뀌어야 한다.
    /// 이게 안 되면 남의 이름·부서·직급이 결재문서 본문에 그대로 찍힌다.
    #[test]
    fn 표시필드까지_전부_주입된다() {
        let mut v = json!({
            "empNm": "이재학", "employees": "이재학 책임연구원", "empNmDutyNm": "이재학 팀원",
            "singleDeptNm": "네이티브 플랫폼팀", "singlePositionNm": "책임연구원", "singleDutyNm": "팀원",
            "deptNm": "네이티브 플랫폼팀", "divNm": "(주)타사", "empCd": "11097", "taskDc": "업무내용"
        });
        inject_identity(&mut v, &me());
        assert_eq!(v["empNm"], "김철수");
        assert_eq!(v["employees"], "김철수 부장");
        assert_eq!(v["empNmDutyNm"], "김철수 팀장");
        assert_eq!(v["singleDeptNm"], "인프라팀");
        assert_eq!(v["singlePositionNm"], "부장");
        assert_eq!(v["singleDutyNm"], "팀장");
        assert_eq!(v["deptNm"], "인프라팀");
        assert_eq!(v["divNm"], "(주)이노그리드");
        assert_eq!(v["empCd"], "22222");
        // 신원과 무관한 필드는 건드리지 않는다.
        assert_eq!(v["taskDc"], "업무내용");
    }

    /// 없는 키를 새로 만들지 않는다(양식마다 필드 구성이 달라 임의 추가는 위험).
    #[test]
    fn 없는_키는_추가하지_않는다() {
        let mut v = json!({ "empNm": "이재학" });
        inject_identity(&mut v, &me());
        assert!(v.get("singleDeptNm").is_none());
        assert!(v.get("employees").is_none());
    }

    /// 조직도 조회 실패(표시정보 빈 값) 시엔 예시값을 지우지 말고 그대로 둔다 —
    /// 빈 문자열로 덮으면 문서에 부서·직급이 통째로 사라진다.
    #[test]
    fn 표시정보를_모르면_예시값을_유지한다() {
        let mut v = json!({ "empNm": "이재학", "singleDeptNm": "네이티브 플랫폼팀", "employees": "이재학 책임연구원" });
        let unknown = Identity {
            co: "1000".into(), dept: "BB999".into(), emp: "22222".into(), name: "김철수".into(),
            dept_nm: String::new(), duty: String::new(), position: String::new(), co_nm: String::new(),
        };
        inject_identity(&mut v, &unknown);
        assert_eq!(v["empNm"], "김철수");                       // 아는 값은 바꾸고
        assert_eq!(v["singleDeptNm"], "네이티브 플랫폼팀");      // 모르는 값은 유지
        assert_eq!(v["employees"], "이재학 책임연구원");
    }

    /// JS `encodeURIComponent` 동등성. 틀리면 상신 본문이 깨지는데 증상이 서버 2099로만 드러나
    /// 원인 추적이 매우 어렵다. 기대값은 JS 규격(비이스케이프 집합 `A-Za-z0-9-_.!~*'()`)에서 도출.
    /// ⚠️ `client::form_urlencode`(공백→`+`)와 **규칙이 다르다** — 여기선 공백이 `%20`.
    #[test]
    fn encode_uri_component는_JS와_같은_규칙이다() {
        assert_eq!(encode_uri_component("a b"), "a%20b");
        assert_eq!(encode_uri_component("-_.!~*'()"), "-_.!~*'()");
        assert_eq!(encode_uri_component("azAZ09"), "azAZ09");
        assert_eq!(encode_uri_component("<div>"), "%3Cdiv%3E");
        assert_eq!(encode_uri_component("가"), "%EA%B0%80");
        assert_eq!(encode_uri_component("&=?#/"), "%26%3D%3F%23%2F");
        assert_eq!(encode_uri_component(""), "");
    }

    /// a03가 준 참가자 노드는 원본 그대로 두고 `org_div`(=div)만 덧붙인다 —
    /// 브라우저 성공 payload와의 유일한 차이라서 재구성하면 어긋난다(§8.14).
    #[test]
    fn norm_participant는_org_div만_덧붙인다() {
        let src = json!({ "org_id": "3052", "div": "d", "act_id": 5000, "dept_line": "x" });
        let out = norm_participant(&src);
        assert_eq!(out["org_div"], "d");
        assert_eq!(out["org_id"], "3052");
        assert_eq!(out["act_id"], 5000);
        assert_eq!(out["dept_line"], "x");
        assert_eq!(norm_participant(&json!({ "org_id": "1" }))["org_div"], "m");
    }

    #[test]
    fn now_kst_datetime은_상신일시_형식이다() {
        let t = now_kst_datetime();
        assert_eq!(t.len(), 19, "YYYY-MM-DD HH:MM:SS");
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[10..11], " ");
        assert_eq!(&t[13..14], ":");
        assert!(t.starts_with("20"));
    }
}

