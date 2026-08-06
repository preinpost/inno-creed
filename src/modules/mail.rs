//! 메일 모듈 — `/mail/mail0*`

use anyhow::{anyhow, bail, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
use crate::modules::board::{collapse_ws, html_to_text, json_str};

pub const INBOX: i64 = 26986;
pub const SENT: i64 = 26989;

/// 메일함(폴더) 목록 — `mail000A01`
pub async fn list_mailboxes(c: &GwClient) -> Result<Value> {
    c.call("/mail/mail000A01", &json!({})).await
}

/// 메일 목록 조회 — `mail003A01`. `page_size`를 크게 주면 전량 조회(상한 없음).
pub async fn list_mails(c: &GwClient, mbox_seq: i64, page: i64, page_size: i64) -> Result<Value> {
    c.call(
        "/mail/mail003A01",
        &json!({
            "boxName": "INBOX",
            "mainApiCode": "mail003A01",
            "mboxSeq": mbox_seq,
            "page": page,
            "pageSize": page_size,
            "sort": "rfc822date",
            "sortType": "desc",
            "listType": "",
            "showType": "",
            "seen": false
        }),
    )
    .await
}

/// 작성폼 초기화 — `mail014A01`. 발송(A04)에 필요한 `sessionKey`/`fileDir`을 확보하기 위해 선행 호출.
pub async fn compose_init(c: &GwClient) -> Result<Value> {
    c.call(
        "/mail/mail014A01",
        &json!({ "mainApiCode": "mail014A01", "mailKind": "me" }),
    )
    .await
}

/// 메일 발송 — `mail014A01`(작성폼 초기화) → `mail014A04`(multipart) 2단계.
/// A01 응답에서 sessionKey/filedir/email/externalSendLimit/bigFileDay/groupMailOption을 동적 취득.
/// ⚠️ 발송은 헤더 서명 + **body 내 authToken**(형식 `loginId|groupSeq|empSeq|secret`)을 함께 요구.
/// `to`는 표시형("이름 <email>") 또는 이메일. 실측 FormData 전 필드를 그대로 재현.
pub async fn send_mail(
    c: &GwClient,
    to: &str,
    subject: &str,
    html: &str,
    attachments: &[String],
) -> Result<Value> {
    let init = compose_init(c).await?;

    // 첨부: 로컬 파일을 mail014A06(multipart `file[]`)로 업로드 → uidAuthList 조립.
    let (uid_auth_list, big_file_cnt) = if attachments.is_empty() {
        ("".to_string(), "0".to_string())
    } else {
        let uploaded = upload_files(c, attachments).await?;
        (uploaded, attachments.len().to_string())
    };
    // `init`(mail014A01 응답)에서 오는 값들 — sessionKey/filedir/email/bigFileDay/externalSendLimit/
    // insideDomainArray/groupMailOption. 크레덴셜이 아니라 **작성폼 세션**에서 온 것이라 재취득으로
    // 바뀌지 않는다. 다만 발송 직전에 토큰이 만료되면 이 스냅샷도 함께 낡을 수 있는데, 폼 조립은
    // 동기 클로저라 여기서 A01을 다시 부를 수 없다. 그 경우 재시도가 실패하고 로그인 안내로 끝난다
    // (다음 호출의 compose_init이 새 세션을 받으므로 사용자가 다시 보내면 정상 발송된다).
    let g = |k: &str| init.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let gmo = init.get("groupMailOption");
    let gm = |k: &str| {
        gmo.and_then(|o| o.get(k))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let email = g("email"); // 발신 이메일(순수 형식) — from/email 필드에 사용
    let inside = match init.get("insideDomainArray") {
        Some(v) if !v.is_null() => v.to_string(),
        _ => "[]".to_string(),
    };
    // 폼을 "만드는 방법"으로 넘긴다 — 401 재취득 재시도 때 클라이언트가 재조립해야 하기 때문
    // (`multipart::Form`은 Clone이 아니고 전송이 소비한다). 호출당 최대 2회 평가된다.
    //
    // ⚠️ **크레덴셜에서 파생되는 값은 반드시 이 클로저 안에서 읽어야 한다.** 밖에서 스냅샷하면
    // 재취득 후 재조립해도 옛 토큰이 그대로 실려 재시도가 형식만 남는다.
    // (`init`에서 온 값들은 크레덴셜 파생이 아니다 — 위 `g` 정의의 주석 참조.)
    let form = || {
        // body용 authToken: 헤더용(groupSeq|empSeq|secret) 앞에 loginId를 덧붙인 형식.
        // 조립할 때마다 새로 읽는다 — 재취득된 토큰이 여기 반영돼야 재시도가 의미를 갖는다.
        let body_auth = format!("{}|{}", c.email_addr(), c.auth_token());
        reqwest::multipart::Form::new()
        .text("from", email.clone())
        .text("fromName", c.emp_name().to_string())
        .text("to", to.to_string())
        .text("cc", "")
        .text("bcc", "")
        .text("htmlContents", html.to_string())
        .text("email", email.clone())
        .text("fileDir", g("filedir"))
        .text("bigFile", "")
        .text("bigFileDay", g("bigFileDay"))
        .text("bigFileCnt", big_file_cnt.clone())
        .text("bigFilePeriod", "")
        .text("mail_kind", "me")
        .text("uidAuthList", uid_auth_list.clone())
        .text("fwFile", "")
        .text("urlList", "")
        .text("fileNameList", "")
        .text("receipt_notific", "")
        .text("securitymailuse", "")
        .text("securitymailpass_enc_web", "")
        .text("immediately", "false")
        .text("toBeDeleted", "false")
        .text("expirationDate", "Invalid date")
        .text("importantmailuse", "")
        .text("eachTrans", "")
        .text("neobizaddr", gm("groupMailAddr"))
        .text("neobizIntedAddr", gm("groupMailIntedAddr"))
        .text("neobizOrg", gm("groupMailOrg"))
        .text("muid", "0")
        .text("domainSeq", "")
        .text("mimeHeader", "")
        .text("sessionKey", g("sessionKey"))
        .text("externalSendLimit", g("externalSendLimit"))
        .text("insideDomainArray", inside.clone())
        .text("aiResultJSON", "")
        .text("subject", subject.to_string())
        .text("authToken", body_auth)
    };

    let v = c.call_multipart("/mail/mail014A04", form).await?;
    // 발송 응답은 표준 봉투로 감싸짐: {"resultCode":0,"resultData":{"result":true,"muid":..,"resultMessage":"SUCCESS"}}
    let rd = v.get("resultData").unwrap_or(&v);
    let ok = rd.get("result").and_then(|r| r.as_bool()).unwrap_or(false);
    if !ok {
        bail!("메일 발송 실패: {v}");
    }
    Ok(rd.clone())
}

/// 로컬 파일들을 `mail014A06`(multipart `file[]`)로 업로드하고, 발송의 `uidAuthList`
/// (JSON 문자열)을 조립한다. 업로드 응답 `resultData.list[]`의 `fileId`를 그대로 사용.
/// (필드명 `file[]`·응답구조는 프론트 번들 실측)
async fn upload_files(c: &GwClient, paths: &[String]) -> Result<String> {
    // 파일 내용을 먼저 읽어둔다 — 폼 조립은 401 재시도 때 한 번 더 일어날 수 있으므로
    // 디스크 I/O(와 그 실패 처리)를 조립 밖으로 뺀다.
    let mut files = Vec::new();
    for p in paths {
        let bytes = std::fs::read(p).map_err(|e| anyhow!("첨부 읽기 실패 {p}: {e}"))?;
        let fname = std::path::Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        files.push((fname, bytes));
    }
    // 이 폼은 파일 내용뿐이라 크레덴셜 파생 값이 없다(인증은 서명 헤더가 전담 — 재시도 때
    // `signed()`가 새 토큰으로 다시 계산한다). 발송 폼의 body authToken 같은 함정이 없다.
    let form = || {
        files.iter().fold(reqwest::multipart::Form::new(), |f, (name, bytes)| {
            let part = reqwest::multipart::Part::bytes(bytes.clone())
                .file_name(name.clone())
                .mime_str("application/octet-stream")
                .expect("고정 MIME 문자열 — 파싱 실패할 수 없다");
            f.part("file[]", part)
        })
    };
    let up = c.call_multipart("/mail/mail014A06", form).await?;
    let list = up
        .pointer("/resultData/list")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow!("mail014A06 업로드 응답에 resultData.list 없음: {up}"))?;

    let items: Vec<Value> = list
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let orig = json_str(f.get("originalFileName"));
            let ext = json_str(f.get("fileExtsn"));
            let size: i64 = json_str(f.get("fileSize")).parse().unwrap_or(0);
            let size_str = format!("{size} Bytes");
            let path = if ext.is_empty() {
                orig.clone()
            } else {
                format!("{orig}.{ext}")
            };
            json!({
                "fileName": orig,
                "fileSize": size_str,
                "fileExtsn": ext,
                "title": format!("{orig}{size_str}"),
                "fileClass": format!("icon_{ext}"),
                "fileId": json_str(f.get("fileId")),
                "filePath": path,
                "fileThumUrl": "", "fileUrl": "", "filePublicYn": "N",
                "noConvertFileSize": size,
                "modifyLocalAttach": "N", "link": "N", "fileDeleteYN": "Y",
                "id": i, "moduleGbn": "MAIL"
            })
        })
        .collect();
    Ok(Value::Array(items).to_string())
}

/// 메일 상세 읽기 — `mail002A01`(body `{uid: muid}`). 본문(HTML→평문)·헤더·첨부목록 반환.
/// ⚠️ 보안: 본문 HTML은 **렌더링하지 않고 서버측에서 평문화**한다 → 외부 이미지(추적 픽셀)를
/// 자동 fetch하지 않는다. 외부 리소스가 있으면 `remoteResourceCount`로 알리기만 한다.
/// 첨부는 메타데이터만 반환(다운로드는 별도 `download_attachment` — 실행 아님, 저장만).
pub async fn read_mail(c: &GwClient, muid: &str) -> Result<Value> {
    let d = c.call("/mail/mail002A01", &json!({ "uid": muid })).await?;
    let mime = d.get("mime").cloned().unwrap_or(json!({}));
    let dm = d.get("decodeMime").cloned().unwrap_or(json!({}));

    let html = mime.pointer("/body/html").and_then(|v| v.as_str()).unwrap_or("");
    let plain = mime.pointer("/body/plain").and_then(|v| v.as_str()).unwrap_or("");
    let body = if !plain.trim().is_empty() {
        collapse_ws(plain)
    } else {
        collapse_ws(&html_to_text(html))
    };
    let remote = count_remote_resources(html);

    let attachments: Vec<Value> = mime
        .get("fileList")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .enumerate()
                .map(|(i, f)| {
                    let ext = json_str(f.get("fileExtsn"));
                    let name = json_str(f.get("originalFileName"));
                    json!({
                        "index": i,
                        "fileName": if ext.is_empty() { name.clone() } else { format!("{name}.{ext}") },
                        "fileExt": ext,
                        "fileSizeApprox": approx_decoded_size(&json_str(f.get("fileSize"))),
                        "fileSn": json_str(f.get("fileSn")),
                        "isImage": matches!(json_str(f.get("fileExtsn")).to_ascii_lowercase().as_str(),
                            "png"|"jpg"|"jpeg"|"gif"|"bmp"|"webp"|"svg")
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({
        "muid": muid,
        "subject": html_to_text(&json_str(dm.get("subject"))),
        "from": html_to_text(&json_str(dm.get("from"))),
        "to": html_to_text(&json_str(dm.get("to"))),
        "date": json_str(dm.get("date")),
        "body": body,
        "attachments": attachments,
        "remoteResourceCount": remote
    }))
}

/// 메일 첨부 다운로드 — 2단계: `mail014A08`(fileSn→다운로드 fileId 변환) → `/ecm/ecm001A03`
/// (`moduleGbn=MAIL`, authKeyMap{muid,email,empSeq}, fileSn=fileId, 서명헤더). `out_path`에 저장.
/// `file_sn`은 `read_mail` 첨부목록의 `fileSn`. 게시판과 동일 ECM 다운로드 엔드포인트.
/// ⚠️ 바이트를 파일로 **저장만** 한다(열지·실행하지 않음) → 악성 첨부도 격리 분석에 안전.
pub async fn download_attachment(
    c: &GwClient,
    muid: &str,
    file_sn: &str,
    out_path: &str,
) -> Result<Value> {
    c.ensure_session().await?; // email(수신함 소유자) 확보
    let email = format!("{}@{}", c.email_addr(), c.email_domain());
    let emp = c.emp_seq();

    // 1) fileSn → 다운로드 fileId
    let auth = json!({ "email": email, "muid": muid, "empSeq": emp }).to_string();
    let a08 = c
        .call_form(
            "/mail/mail014A08",
            &[("moduleGbn", "MAIL"), ("authKeyMap", &auth), ("fileSn", file_sn)],
        )
        .await?;
    let item = a08
        .get("list")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow!("mail014A08: 첨부 없음(fileSn 불일치? muid/fileSn 확인)"))?;
    let file_id = item
        .get("fileId")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("mail014A08: fileId 없음"))?
        .to_string();
    let list_name = json_str(item.get("fileName"));

    // 2) ECM 다운로드(바이트)
    let auth_dl = json!({ "muid": muid, "email": email, "empSeq": emp }).to_string();
    let (size, srv_name) = c
        .download_form(
            "/ecm/ecm001A03",
            &[
                ("moduleGbn", "MAIL"),
                ("authKeyMap", &auth_dl),
                ("fileSn", &file_id),
                ("condition", "99"),
            ],
            out_path,
        )
        .await?;

    Ok(json!({
        "ok": true,
        "path": out_path,
        "bytes": size,
        "serverFileName": srv_name.filter(|s| !s.is_empty()).unwrap_or(list_name)
    }))
}

/// mail002A01 `fileSize`는 원본 바이트가 아니라 MIME 본문(base64+줄바꿈) 크기라 실제보다 ~33% 크다.
/// 원본 크기 근사치(≈ 인코딩크기 × 3/4)를 반환한다. **정확한 크기는 다운로드 결과의 `bytes`** 참조.
fn approx_decoded_size(encoded: &str) -> i64 {
    encoded.parse::<i64>().map(|n| n * 3 / 4).unwrap_or(0)
}

/// 본문 HTML의 외부(원격) 리소스 개수 추정 — `src="http` / `url(http`(외부 이미지·추적 픽셀).
/// fetch하지 않고 개수만 센다(자동 로드 금지 = 열람 유출 방지).
fn count_remote_resources(html: &str) -> usize {
    let h = html.to_ascii_lowercase();
    let mut n = 0;
    for pat in ["src=\"http", "src='http", "url(http", "background=\"http"] {
        n += h.matches(pat).count();
    }
    n
}

/// 메일 삭제(휴지통 이동) — `mail002A05`. `uids`=콤마구분 muid 리스트(다건).
/// ⚠️ 휴지통 이동 시 muid가 재부여되므로 이후 추적은 재조회 필요.
pub async fn delete_mails(c: &GwClient, uids: &str) -> Result<Value> {
    c.call(
        "/mail/mail002A05",
        &json!({ "uids": uids, "mailKey": "", "boxName": "" }),
    )
    .await
}
