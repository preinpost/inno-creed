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
///
/// ⚠️ **`boxName`은 서버가 무시한다**(실측: `mboxSeq`=SENT에 `boxName="INBOX"`를 실어도 보낸메일이
/// 나온다). 판단 기준은 `mboxSeq` 하나뿐이라 임시보관함 조회도 seq만 바꿔 이 함수를 그대로 쓴다.
/// 값을 고치지 않고 두는 이유는 받은메일함 조회가 이 값으로 이미 실증돼 있기 때문이다.
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

/// 임시보관함 메일함 이름. `INBOX`/`SENT`처럼 seq를 상수로 박지 않는다 —
/// **메일함 seq는 계정마다 다르므로** 이름으로 해석해야 다른 계정에서도 맞는 함을 본다.
pub const DRAFTS: &str = "DRAFTS";

/// `list_mailboxes` 응답에서 이름이 `want`인 메일함의 `mboxSeq`를 찾는다.
///
/// 응답이 중첩 구조로 올 수 있어(계정·서버 버전차) 키 존재로 노드를 판별하며 트리를 훑는다.
/// `fullname`("DRAFTS")과 `name` 둘 다 본다 — 어느 쪽에 들어오는지가 함마다 다르다.
/// `mboxSeq`가 숫자/문자열 양쪽으로 오는 것은 `json_str`로 흡수한다(이 API 계열의 고질).
fn find_mbox_seq(node: &Value, want: &str) -> Option<i64> {
    match node {
        Value::Object(map) => {
            if map.contains_key("mboxSeq")
                && [map.get("fullname"), map.get("name")]
                    .iter()
                    .any(|v| json_str(*v).eq_ignore_ascii_case(want))
                && let Ok(seq) = json_str(map.get("mboxSeq")).parse::<i64>()
            {
                return Some(seq);
            }
            map.values().find_map(|v| find_mbox_seq(v, want))
        }
        Value::Array(items) => items.iter().find_map(|v| find_mbox_seq(v, want)),
        _ => None,
    }
}

/// 메일함 이름 → `mboxSeq`. 못 찾으면 어느 이름을 찾았는지 밝힌 에러.
pub async fn mbox_seq(c: &GwClient, name: &str) -> Result<i64> {
    let boxes = list_mailboxes(c).await?;
    find_mbox_seq(&boxes, name).ok_or_else(|| {
        anyhow!("메일함 '{name}' 을 찾지 못했다 — list_mailboxes 응답에 그 이름의 mboxSeq가 없다")
    })
}

/// 임시보관함(DRAFTS) 목록 — 이름으로 seq를 해석한 뒤 `list_mails`.
/// 응답은 받은메일함과 동일한 서버 원본 봉투(메일 배열 키는 `Records`).
pub async fn list_drafts(c: &GwClient, page: i64, page_size: i64) -> Result<Value> {
    let seq = mbox_seq(c, DRAFTS).await?;
    list_mails(c, seq, page, page_size).await
}

/// 목록 응답(`Records`)에 그 `muid`가 있는지. read-back 검증용.
fn has_muid(list: &Value, muid: &str) -> bool {
    list.get("Records")
        .and_then(|r| r.as_array())
        .is_some_and(|rows| rows.iter().any(|m| json_str(m.get("muid")) == muid))
}

/// 작성폼 초기화 — `mail014A01`. 발송(A04)·임시저장(A14)에 필요한 `sessionKey`/`fileDir`을
/// 확보하기 위해 선행 호출. 응답에는 재저장 때 되돌려줘야 하는 `mailkey`도 들어 있다.
pub async fn compose_init(c: &GwClient) -> Result<Value> {
    c.call(
        "/mail/mail014A01",
        &json!({ "mainApiCode": "mail014A01", "mailKind": "me" }),
    )
    .await
}

/// 발송(`mail014A04`)과 임시저장(`mail014A14`)이 공유하는 작성 폼.
///
/// 담기는 값은 **`mail014A01` 응답 스냅샷 + 호출 인자**뿐이다. 크레덴셜 파생값은 담지 않는다
/// (`fields`가 조립할 때마다 새로 읽는다 — 이유는 그쪽 주석).
struct ComposeForm {
    email: String,
    filedir: String,
    big_file_day: String,
    big_file_cnt: String,
    uid_auth_list: String,
    session_key: String,
    external_send_limit: String,
    inside_domain: String,
    neobiz_addr: String,
    neobiz_inted_addr: String,
    neobiz_org: String,
    to: String,
    subject: String,
    html: String,
}

impl ComposeForm {
    /// `init`은 `compose_init`(mail014A01) 응답. 여기서 오는 값들 — sessionKey/filedir/email/
    /// bigFileDay/externalSendLimit/insideDomainArray/groupMailOption — 은 크레덴셜이 아니라
    /// **작성폼 세션**에서 온 것이라 401 재취득으로 바뀌지 않는다. 다만 전송 직전에 토큰이 만료되면
    /// 이 스냅샷도 함께 낡을 수 있는데, 폼 조립은 동기 클로저라 거기서 A01을 다시 부를 수 없다.
    /// 그 경우 재시도가 실패하고 로그인 안내로 끝난다(다음 호출의 `compose_init`이 새 세션을 받으므로
    /// 사용자가 다시 시도하면 정상 동작한다).
    fn new(
        init: &Value,
        to: &str,
        subject: &str,
        html: &str,
        uid_auth_list: String,
        big_file_cnt: String,
    ) -> Self {
        let g = |k: &str| init.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let gmo = init.get("groupMailOption");
        let gm = |k: &str| {
            gmo.and_then(|o| o.get(k))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        };
        Self {
            email: g("email"), // 발신 이메일(순수 형식) — from/email 필드에 사용
            filedir: g("filedir"),
            big_file_day: g("bigFileDay"),
            big_file_cnt,
            uid_auth_list,
            session_key: g("sessionKey"),
            external_send_limit: g("externalSendLimit"),
            inside_domain: match init.get("insideDomainArray") {
                Some(v) if !v.is_null() => v.to_string(),
                _ => "[]".to_string(),
            },
            neobiz_addr: gm("groupMailAddr"),
            neobiz_inted_addr: gm("groupMailIntedAddr"),
            neobiz_org: gm("groupMailOrg"),
            to: to.to_string(),
            subject: subject.to_string(),
            html: html.to_string(),
        }
    }

    /// 실측 FormData 전 필드.
    ///
    /// ⚠️ **크레덴셜에서 파생되는 값(`authToken`)은 호출할 때마다 여기서 새로 읽는다.**
    /// 밖에서 스냅샷하면 401 재취득 후 폼을 재조립해도 옛 토큰이 그대로 실려 재시도가 형식만 남는다.
    fn fields(&self, c: &GwClient) -> Vec<(&'static str, String)> {
        // body용 authToken: 헤더용(groupSeq|empSeq|secret) 앞에 loginId를 덧붙인 형식.
        let body_auth = format!("{}|{}", c.email_addr(), c.auth_token());
        vec![
            ("from", self.email.clone()),
            ("fromName", c.emp_name()),
            ("to", self.to.clone()),
            ("cc", String::new()),
            ("bcc", String::new()),
            ("htmlContents", self.html.clone()),
            ("email", self.email.clone()),
            ("fileDir", self.filedir.clone()),
            ("bigFile", String::new()),
            ("bigFileDay", self.big_file_day.clone()),
            ("bigFileCnt", self.big_file_cnt.clone()),
            ("bigFilePeriod", String::new()),
            ("mail_kind", "me".into()),
            ("uidAuthList", self.uid_auth_list.clone()),
            ("fwFile", String::new()),
            ("urlList", String::new()),
            ("fileNameList", String::new()),
            ("receipt_notific", String::new()),
            ("securitymailuse", String::new()),
            ("securitymailpass_enc_web", String::new()),
            ("immediately", "false".into()),
            ("toBeDeleted", "false".into()),
            ("expirationDate", "Invalid date".into()),
            ("importantmailuse", String::new()),
            ("eachTrans", String::new()),
            ("neobizaddr", self.neobiz_addr.clone()),
            ("neobizIntedAddr", self.neobiz_inted_addr.clone()),
            ("neobizOrg", self.neobiz_org.clone()),
            ("muid", "0".into()),
            ("domainSeq", String::new()),
            ("mimeHeader", String::new()),
            ("sessionKey", self.session_key.clone()),
            ("externalSendLimit", self.external_send_limit.clone()),
            ("insideDomainArray", self.inside_domain.clone()),
            ("aiResultJSON", String::new()),
            ("subject", self.subject.clone()),
            ("authToken", body_auth),
        ]
    }

    fn build(&self, c: &GwClient) -> reqwest::multipart::Form {
        self.fields(c)
            .into_iter()
            .fold(reqwest::multipart::Form::new(), |f, (k, v)| f.text(k, v))
    }
}

/// 첨부를 `mail014A06`에 올려 폼의 `uidAuthList`/`bigFileCnt` 짝을 만든다. 없으면 빈 값.
async fn attachment_fields(c: &GwClient, attachments: &[String]) -> Result<(String, String)> {
    if attachments.is_empty() {
        return Ok((String::new(), "0".to_string()));
    }
    let uploaded = upload_files(c, attachments).await?;
    Ok((uploaded, attachments.len().to_string()))
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
    let (uid_auth_list, big_file_cnt) = attachment_fields(c, attachments).await?;
    let cf = ComposeForm::new(&init, to, subject, html, uid_auth_list, big_file_cnt);

    // 폼을 "만드는 방법"으로 넘긴다 — 401 재취득 재시도 때 클라이언트가 재조립해야 하기 때문
    // (`multipart::Form`은 Clone이 아니고 전송이 소비한다). 호출당 최대 2회 평가된다.
    let v = c.call_multipart("/mail/mail014A04", || cf.build(c)).await?;
    // 발송 응답은 표준 봉투로 감싸짐: {"resultCode":0,"resultData":{"result":true,"muid":..,"resultMessage":"SUCCESS"}}
    let rd = v.get("resultData").unwrap_or(&v);
    let ok = rd.get("result").and_then(|r| r.as_bool()).unwrap_or(false);
    if !ok {
        bail!("메일 발송 실패: {v}");
    }
    Ok(rd.clone())
}

/// 임시저장(A14)이 발송 폼 위에 덧붙이는 전용 필드. 값은 **신규 저장** 기준이다
/// (재저장은 `autoMUID`/`beforeMUID`/`mailKey`에 직전 저장분 좌표가 들어가는데, 이 도구는 신규만 한다).
///
/// ⚠️ `isFirst`는 첫 저장이 **"0"**이다 — 프론트가 `isFirst === true ? "0" : "1"`로 보낸다.
/// 뒤집힌 것처럼 보이지만 서버가 그 값을 기대하므로 그대로 재현한다.
const DRAFT_FIELDS: &[(&str, &str)] = &[
    ("autoMUID", ""),
    ("beforeMailType", "me"), // compose_init이 mailKind:"me"로 연 폼이다
    ("beforeMUID", ""),
    ("mailKey", ""),
    ("isFirst", "0"),
    ("draftType", "true"),      // 수동 임시저장(파일 첨부 여부와 무관하게 true)
    ("autoDraftType", "false"), // 에디터 자동저장(A13)이 아님
];

/// 저장 직후 read-back이 훑는 임시보관함 통수. 목록은 최신순(`rfc822date desc`)이라 방금 저장한
/// 건이 맨 앞에 오므로 넉넉하다 — 크게 잡을 이유가 없다(조회 비용만 는다).
const DRAFT_READBACK_PAGE: i64 = 20;

/// 메일 임시저장 — `mail014A01`(작성폼 초기화) → `mail014A14`(multipart) 2단계.
/// **발송하지 않는다** — 임시보관함(DRAFTS)에 저장만 한다.
/// 폼은 발송(A04)과 동일하고 `DRAFT_FIELDS` 7개만 더 붙는다. 첨부 경로도 발송과 같다.
/// 반환: `draft_muid`(= 응답 `resultData.autoMUID`, 후속 조회·삭제 키),
/// `mail_key`(= A01의 `mailkey`, 재저장 때 `mailKey`로 되돌려줄 값),
/// `verified_by_readback`(임시보관함 재조회로 그 muid를 실제로 찾았는지 — 프로젝트 규약 §7).
pub async fn save_mail_draft(
    c: &GwClient,
    to: &str,
    subject: &str,
    html: &str,
    attachments: &[String],
) -> Result<Value> {
    let init = compose_init(c).await?;
    let (uid_auth_list, big_file_cnt) = attachment_fields(c, attachments).await?;
    // 프론트는 제목이 비면 "(제목없음)"으로 채워 저장한다.
    let subject = if subject.trim().is_empty() {
        "(제목없음)"
    } else {
        subject
    };
    let cf = ComposeForm::new(&init, to, subject, html, uid_auth_list, big_file_cnt);
    let form = || {
        DRAFT_FIELDS
            .iter()
            .fold(cf.build(c), |f, (k, v)| f.text(*k, *v))
    };

    let v = c.call_multipart("/mail/mail014A14", form).await?;
    // 999 = "로그인 사용자가 변경되어 임시저장을 할 수 없습니다"(프론트 문구). 전송 실패와 구분한다.
    if v.get("resultCode").and_then(|x| x.as_i64()) == Some(999) {
        bail!("메일 임시저장 실패: 로그인 사용자가 변경돼 임시저장할 수 없다(resultCode 999) — 브라우저에서 다시 로그인한 뒤 재시도할 것");
    }
    let rd = v.get("resultData").unwrap_or(&v);
    let draft_muid = json_str(rd.get("autoMUID"));
    if draft_muid.is_empty() {
        bail!("메일 임시저장 실패(autoMUID 없음): {v}");
    }
    // read-back — 성공 응답을 반영으로 단정하지 않는다(프로젝트 규약). 임시보관함을 재조회해
    // **그 muid가 목록에 실제로 있는지** 본다.
    // ⚠️ 통수(+1) 비교가 아니라 muid 대조인 이유: 저장 전후 사이에 메일이 들어오거나 다른 세션이
    // 초안을 지우면 통수는 조용히 오탐이 난다. muid는 우리가 만든 그 한 건만 가리킨다.
    // 조회 실패(권한·네트워크)는 "확인 못 함"이지 "저장 실패"가 아니므로 에러로 올리지 않는다.
    // 그 경우 `verified_by_readback: false`로 보고한다 — **저장이 안 됐다는 뜻이 아니라
    // 확인하지 못했다는 뜻**이고, 목록에 없어서 false인 경우와 응답에서 구분되지 않는다.
    // 어느 쪽이든 사람이 임시보관함을 눈으로 확인해야 한다.
    let verified = list_drafts(c, 1, DRAFT_READBACK_PAGE)
        .await
        .map(|list| has_muid(&list, &draft_muid))
        .unwrap_or(false);

    Ok(json!({
        "draft_muid": draft_muid,
        "mail_key": json_str(init.get("mailkey")),
        "verified_by_readback": verified
    }))
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

/// 발송·임시저장이 공유하는 폼의 **회귀 기준선**. 발송 폼을 `ComposeForm`으로 뽑아내면서
/// 필드가 빠지거나 값이 달라져도 컴파일러가 잡지 못하기 때문에, 실측 필드 집합을 여기 박아둔다.
#[cfg(test)]
mod tests {
    use super::*;

    fn sample_init() -> Value {
        json!({
            "email": "me@example.com",
            "filedir": "/dir/2026",
            "sessionKey": "SK-1",
            "bigFileDay": "30",
            "externalSendLimit": "50",
            "mailkey": "1785980031409_c7d8f89c",
            "insideDomainArray": ["example.com"],
            "groupMailOption": {
                "groupMailAddr": "g@example.com",
                "groupMailIntedAddr": "gi@example.com",
                "groupMailOrg": "조직"
            }
        })
    }

    fn fields_of(cf: &ComposeForm) -> std::collections::HashMap<String, String> {
        cf.fields(&GwClient::new(None))
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect()
    }

    #[test]
    fn 발송_폼은_실측_필드_37개를_그대로_담는다() {
        let cf = ComposeForm::new(
            &sample_init(),
            "홍길동 <hong@example.com>",
            "제목",
            "<p>본문</p>",
            "[]".to_string(),
            "2".to_string(),
        );
        let raw = cf.fields(&GwClient::new(None));
        let names: Vec<&str> = raw.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            names,
            vec![
                "from", "fromName", "to", "cc", "bcc", "htmlContents", "email", "fileDir",
                "bigFile", "bigFileDay", "bigFileCnt", "bigFilePeriod", "mail_kind", "uidAuthList",
                "fwFile", "urlList", "fileNameList", "receipt_notific", "securitymailuse",
                "securitymailpass_enc_web", "immediately", "toBeDeleted", "expirationDate",
                "importantmailuse", "eachTrans", "neobizaddr", "neobizIntedAddr", "neobizOrg",
                "muid", "domainSeq", "mimeHeader", "sessionKey", "externalSendLimit",
                "insideDomainArray", "aiResultJSON", "subject", "authToken",
            ],
            "발송 FormData 필드 집합이 변했다"
        );

        let f = fields_of(&cf);
        assert_eq!(f["from"], "me@example.com");
        assert_eq!(f["email"], "me@example.com");
        assert_eq!(f["to"], "홍길동 <hong@example.com>");
        assert_eq!(f["subject"], "제목");
        assert_eq!(f["htmlContents"], "<p>본문</p>");
        assert_eq!(f["fileDir"], "/dir/2026");
        assert_eq!(f["sessionKey"], "SK-1");
        assert_eq!(f["bigFileDay"], "30");
        assert_eq!(f["externalSendLimit"], "50");
        assert_eq!(f["insideDomainArray"], r#"["example.com"]"#);
        assert_eq!(f["neobizaddr"], "g@example.com");
        assert_eq!(f["neobizIntedAddr"], "gi@example.com");
        assert_eq!(f["neobizOrg"], "조직");
        assert_eq!(f["uidAuthList"], "[]");
        assert_eq!(f["bigFileCnt"], "2");
        assert_eq!(f["mail_kind"], "me");
        assert_eq!(f["muid"], "0"); // 0 = 신규(답장/전달이 아님)
        assert_eq!(f["immediately"], "false");
        assert_eq!(f["expirationDate"], "Invalid date");
    }

    /// 메일함 seq는 계정마다 달라 이름으로 찾는다 — 그 탐색이 중첩·타입혼용을 견디는지.
    #[test]
    fn 메일함_seq를_이름으로_찾는다() {
        let boxes = json!({
            "mailboxList": [
                { "fullname": "INBOX", "name": "INBOX", "mboxSeq": 26986, "exists": "3" },
                { "fullname": "SENT", "name": "SENT", "mboxSeq": "26989", "exists": "0" },
                { "children": [
                    { "fullname": "DRAFTS", "name": "DRAFTS", "mboxSeq": 26992, "exists": "0" }
                ]}
            ]
        });
        assert_eq!(find_mbox_seq(&boxes, "DRAFTS"), Some(26992)); // 중첩된 곳도 찾는다
        assert_eq!(find_mbox_seq(&boxes, "INBOX"), Some(26986));
        assert_eq!(find_mbox_seq(&boxes, "SENT"), Some(26989)); // mboxSeq가 문자열이어도 흡수
        assert_eq!(find_mbox_seq(&boxes, "drafts"), Some(26992)); // 대소문자 무시
        assert_eq!(find_mbox_seq(&boxes, "ARCHIVE"), None); // 없는 함은 None(엉뚱한 함 금지)
    }

    /// 이름이 `name`에만 있고 `fullname`이 비어도 찾아야 한다(함마다 어느 키에 오는지 다르다).
    #[test]
    fn 메일함_이름은_fullname이_없으면_name으로_본다() {
        let boxes = json!([{ "name": "DRAFTS", "mboxSeq": 1 }]);
        assert_eq!(find_mbox_seq(&boxes, "DRAFTS"), Some(1));
        // mboxSeq가 숫자로 파싱되지 않으면 그 노드는 후보가 아니다(빈 문자열 등).
        assert_eq!(find_mbox_seq(&json!([{ "name": "DRAFTS", "mboxSeq": "" }]), "DRAFTS"), None);
    }

    /// 임시저장 read-back의 판정부 — 통수가 아니라 muid로 본다.
    #[test]
    fn read_back은_목록에서_그_muid를_찾는다() {
        let list = json!({ "Records": [{ "muid": 13541170 }, { "muid": "13541171" }] });
        assert!(has_muid(&list, "13541170")); // 숫자로 와도 문자열로 흡수해 비교
        assert!(has_muid(&list, "13541171"));
        assert!(!has_muid(&list, "99999999")); // 없는 muid는 미검증
        assert!(!has_muid(&json!({}), "1")); // Records가 아예 없으면 미검증
        assert!(!has_muid(&json!({ "Records": [] }), "1"));
    }

    #[test]
    fn 도메인배열은_응답에_없으면_빈_배열로_보낸다() {
        let cf = ComposeForm::new(&json!({}), "to", "s", "", String::new(), "0".to_string());
        assert_eq!(fields_of(&cf)["insideDomainArray"], "[]");
    }

    #[test]
    fn 임시저장은_발송_폼에_전용필드_7개만_더한다() {
        let base: Vec<&str> = ComposeForm::new(&json!({}), "to", "s", "", String::new(), "0".into())
            .fields(&GwClient::new(None))
            .iter()
            .map(|(k, _)| *k)
            .collect();
        let extra: Vec<&str> = DRAFT_FIELDS.iter().map(|(k, _)| *k).collect();
        assert_eq!(
            extra,
            vec![
                "autoMUID",
                "beforeMailType",
                "beforeMUID",
                "mailKey",
                "isFirst",
                "draftType",
                "autoDraftType"
            ]
        );
        // 발송 폼과 겹치는 이름이 없어야 한다 — 겹치면 같은 필드를 두 번 보내게 된다.
        assert!(extra.iter().all(|k| !base.contains(k)));

        let v: std::collections::HashMap<&str, &str> = DRAFT_FIELDS.iter().copied().collect();
        assert_eq!(v["isFirst"], "0", "첫 저장은 '0'이다(프론트 삼항이 뒤집혀 있다)");
        assert_eq!(v["draftType"], "true");
        assert_eq!(v["autoDraftType"], "false");
        assert_eq!(v["beforeMailType"], "me");
        // 신규 저장이므로 직전 draft 좌표는 전부 빈 값
        assert!(["autoMUID", "beforeMUID", "mailKey"]
            .iter()
            .all(|k| v[k].is_empty()));
    }
}
