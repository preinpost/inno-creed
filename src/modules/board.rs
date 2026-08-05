//! 게시판(UF) 모듈 — `/board/APIHandler/*`. 읽기 전용(목록·상세).
//! 인증은 헤더 서명만으로 완결(companyInfo 불필요).

use anyhow::{anyhow, Result};
use serde_json::{json, Value};

use crate::client::GwClient;
// json_str은 util 소유. mail 모듈이 `board::json_str`로 참조해 온 경로를 유지하려고 재수출한다.
pub(crate) use crate::util::json_str;
use crate::util::digits_only;

/// 최근 공지/게시글 목록 — `ViewBoardNewAndNoticeArtList`.
/// `use_list_art_content:Y`로 본문 프리뷰까지 내려온다. 결과는 유용 필드만 추려서 반환.
///
/// 필터(빈 문자열이면 미적용):
/// - `search`: 검색어. `field`로 대상 지정 — "title"(제목)/"content"(내용)/"author"(작성자),
///   그 외/빈값은 통합검색(searchTotal).
/// - `start_date`/`end_date`: 등록일 범위(YYYY-MM-DD).
///
/// 게시판별 필터는 이 엔드포인트(전 게시판 공지 집계)에서 지원되지 않는다 — searchBoard를
/// 넣어도 무시됨(실측). 출력의 `boardId`(cat_seq_no)로 클라이언트단 구분만 가능.
#[allow(clippy::too_many_arguments)]
pub async fn list_notices(
    c: &GwClient,
    page: i64,
    page_size: i64,
    search: &str,
    field: &str,
    start_date: &str,
    end_date: &str,
) -> Result<Value> {
    let (total, title, desc, nick) = match field {
        "title" => ("", search, "", ""),
        "content" => ("", "", search, ""),
        "author" => ("", "", "", search),
        _ => (search, "", "", ""),
    };
    // 서버는 날짜를 YYYYMMDD(구분자 없음)만 받는다 — 대시가 있으면 500. 숫자만 남긴다.
    let start_date = digits_only(start_date);
    let end_date = digits_only(end_date);
    let body = json!({
        "adminPage": "N", "searchAuthType": "U",
        "searchTotal": total, "searchTitle": title, "searchNick": nick, "searchDesc": desc,
        "searchBoard": "", "searchRemarkNo": "", "searchEtcValue": "",
        "searchStartDate": start_date, "searchEndDate": end_date, "searchStartTerm": "", "searchEndTerm": "",
        "eventStatus": "", "reserveStatus": "", "counselingOk": "", "searchMailFrom": "",
        "sort": "write_date", "project_id": Value::Null, "page": page, "pageSize": page_size,
        "sortType": "desc", "menuCode": "UFA", "pageCode": "UFA1000", "moduleCode": "UF",
        "noticeYn": "Y", "apiName": "ViewBoardNewAndNoticeArtList", "use_list_art_content": "Y"
    });
    let data = c
        .call("/board/APIHandler/ViewBoardNewAndNoticeArtList", &body)
        .await?;

    let s = |v: &Value, k: &str| json_str(v.get(k));
    let articles: Vec<Value> = data
        .get("articleList")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|a| {
                    let preview = collapse_ws(&s(a, "art_content"));
                    json!({
                        "artSeqNo": s(a, "art_seq_no"),
                        "title": s(a, "art_title"),
                        "board": s(a, "cat_title"),
                        "boardId": s(a, "cat_seq_no"),
                        "writer": s(a, "mbr_nick"),
                        "dept": s(a, "dept_name"),
                        "writeDate": s(a, "write_date"),
                        "readCnt": s(a, "read_cnt"),
                        "fileCnt": s(a, "file_cnt"),
                        "attachmentUid": s(a, "uid"),
                        "isNew": s(a, "is_new_yn") == "Y",
                        "read": s(a, "art_read_yn") == "Y",
                        "preview": truncate(&preview, 200)
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(json!({ "totalCnt": data.get("totalCnt").cloned().unwrap_or(Value::Null), "articles": articles }))
}

/// 게시글 상세 — `ViewPost`. 본문(HTML→텍스트)·댓글 포함.
/// ⚠️ 호출 시 조회수가 증가한다(실제 열람 처리) — 순수 조회는 아님.
pub async fn read_post(c: &GwClient, art_seq_no: &str) -> Result<Value> {
    let body = json!({
        "art_seq_no": art_seq_no, "adminPage": "N", "externalYn": "N",
        "menuCode": "UFA", "pageCode": "UFA1000", "moduleCode": "UF",
        "presentPassword": "", "isPrint": "N", "searchParams": Value::Null
    });
    let data = c.call("/board/APIHandler/ViewPost", &body).await?;
    let art = data
        .get("art")
        .ok_or_else(|| anyhow!("ViewPost 응답에 art 없음 (art_seq_no={art_seq_no})"))?;

    let s = |v: &Value, k: &str| json_str(v.get(k));
    // 댓글 본문 필드명은 미실측(캡처한 글은 댓글 0개) → 유력 키를 방어적으로 탐색.
    let first = |v: &Value, keys: &[&str]| {
        keys.iter()
            .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
            .unwrap_or_default()
            .to_string()
    };
    let comments: Vec<Value> = data
        .get("remarkList")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|r| {
                    json!({
                        "writer": s(r, "mbr_nick"),
                        "writeDate": s(r, "write_date"),
                        "content": collapse_ws(&html_to_text(
                            &first(r, &["remark_desc", "remark_content", "content", "art_content"])
                        ))
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    // 상세의 art.cat_title은 null → 게시판명은 resultData.board.cat_title에 있음.
    let board_name = json_str(data.get("board").and_then(|b| b.get("cat_title")));
    Ok(json!({
        "artSeqNo": s(art, "art_seq_no"),
        "title": s(art, "art_title"),
        "board": board_name,
        "writer": s(art, "mbr_nick"),
        "dept": s(art, "dept_name"),
        "writeDate": s(art, "write_date"),
        "readCnt": s(art, "read_cnt"),
        "fileCnt": s(art, "file_cnt"),
        "attachmentUid": s(art, "uid"),
        "content": collapse_ws(&html_to_text(&s(art, "art_content"))),
        "comments": comments
    }))
}

/// 게시글 첨부파일 목록 — `/ecm/ecm001A04`(x-www-form-urlencoded, authKeyMap). `art_seq_no`는
/// 게시글 번호, `uid`는 게시글의 attachmentUid(fileIds). 파일별 fileId/이름/확장자/크기/저장경로 반환.
/// (실측: `moduleGbn=BOARD&authKeyMap={empSeq,cat_seq_no:"U",art_seq_no,fileIds}&fileSn=0&condition=99`)
pub async fn list_attachments(c: &GwClient, art_seq_no: &str, uid: &str) -> Result<Value> {
    if split_uids(uid).is_empty() {
        return Ok(json!({ "files": [] }));
    }
    let auth = json!({
        "empSeq": c.emp_seq(),
        "cat_seq_no": "U",
        "art_seq_no": art_seq_no,
        "survey_no": "",
        "reply_seq_no": "",
        "fileIds": uid
    })
    .to_string();
    let data = c
        .call_form(
            "/ecm/ecm001A04",
            &[
                ("moduleGbn", "BOARD"),
                ("authKeyMap", &auth),
                ("fileSn", "0"),
                ("condition", "99"),
            ],
        )
        .await?;
    let list = data
        .get("list")
        .or_else(|| data.get("storageList"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let files: Vec<Value> = list
        .iter()
        .map(|f| {
            json!({
                "fileId": json_str(f.get("fileId").or_else(|| f.get("fileUID"))),
                "fileName": json_str(f.get("originalFileName").or_else(|| f.get("fileName"))),
                "fileExt": json_str(f.get("fileExtsn").or_else(|| f.get("fileExt"))),
                "fileSize": json_str(f.get("fileSize")),
                "storagePath": json_str(f.get("linkedFilePath"))
            })
        })
        .collect();
    Ok(json!({ "files": files }))
}

/// 게시글 첨부파일 다운로드 — `/ecm/ecm001A03`(x-www-form-urlencoded, authKeyMap, 서명헤더 필수).
/// `art_seq_no`/`uid`는 목록과 동일, `file_sn`은 목록에서 받은 파일 순번(0-base, 단건 다운로드).
/// `out_path`에 바이트 저장. 서버가 JSON 봉투를 주면(=에러) 실패 처리.
pub async fn download_attachment(
    c: &GwClient,
    art_seq_no: &str,
    uid: &str,
    file_sn: i64,
    out_path: &str,
) -> Result<Value> {
    if split_uids(uid).is_empty() {
        anyhow::bail!("download_attachment: uid 비어있음");
    }
    let auth = json!({
        "empSeq": c.emp_seq(),
        "cat_seq_no": "U",
        "art_seq_no": art_seq_no,
        "survey_no": "",
        "reply_seq_no": "",
        "fileIds": uid
    })
    .to_string();
    let sn = file_sn.to_string();
    let (size, filename) = c
        .download_form(
            "/ecm/ecm001A03",
            &[
                ("moduleGbn", "BOARD"),
                ("authKeyMap", &auth),
                ("fileSn", &sn),
                ("condition", "99"),
            ],
            out_path,
        )
        .await?;
    Ok(json!({
        "ok": true,
        "path": out_path,
        "bytes": size,
        "serverFileName": filename
    }))
}


/// 콤마 구분 uid 문자열 → 개별 uid 벡터(공백/빈값 제거).
fn split_uids(uid: &str) -> Vec<String> {
    uid.split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}


/// 게시글 HTML 본문을 대략적인 평문으로 변환(외부 크레이트 없이). 블록 경계는 개행으로,
/// 태그 제거, 주요 엔티티 디코드. 완벽한 렌더링이 아니라 에이전트 읽기용 근사.
/// ⛔ **`approval::html_to_text`와 통합 금지 — 동작이 반대다.** 이쪽(게시판·메일)은 블록 태그
/// (br/p/div/tr/li/h1~h3)를 개행으로, td/th를 탭으로 바꿔 **구조를 보존**한다. 결재 쪽은 모든
/// 태그를 공백 하나로 눌러 한 줄로 만든다. 이름이 같다고 합치면 본문 표시가 조용히 바뀐다.
pub(crate) fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut tag = String::new();
    for ch in html.chars() {
        match ch {
            '<' => {
                in_tag = true;
                tag.clear();
            }
            '>' => {
                in_tag = false;
                let t = tag.to_ascii_lowercase();
                let t = t.trim_start_matches('/').trim();
                if t.starts_with("br")
                    || t.starts_with("p")
                    || t.starts_with("div")
                    || t.starts_with("tr")
                    || t.starts_with("li")
                    || t.starts_with("h1")
                    || t.starts_with("h2")
                    || t.starts_with("h3")
                {
                    out.push('\n');
                } else if t.starts_with("td") || t.starts_with("th") {
                    out.push('\t');
                }
            }
            _ if in_tag => tag.push(ch),
            _ => out.push(ch),
        }
    }
    out.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// 연속 공백/개행 정리(태그 제거 후 남는 과도한 공백 축약). 빈 줄은 최대 1개까지 유지.
/// ⛔ **`approval::collapse_ws`와 통합 금지** — 이쪽은 **개행을 보존**하고 저쪽은 전부 없앤다.
pub(crate) fn collapse_ws(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for line in s.split('\n') {
        let t = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() {
            if matches!(lines.last(), Some(l) if !l.is_empty()) {
                lines.push(String::new());
            }
        } else {
            lines.push(t);
        }
    }
    lines.join("\n").trim().to_string()
}

/// 문자 기준 잘라내기(멀티바이트 안전, 말줄임 추가).
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠️ 이 모듈의 `html_to_text`/`collapse_ws`는 `approval` 의 동명 함수와 **의도적으로 다르다**
    /// (게시판·메일 본문은 줄바꿈을 살리고, 결재 본문은 한 줄로 누른다).
    /// 이름이 같다고 합치면 게시판/메일 본문 표시가 조용히 바뀐다 — 그걸 막는 테스트다.
    /// 근거: `.claude-workspace/todo/refactor-structure/05-shared-util-extraction.md` (B)절.
    #[test]
    fn html_to_text는_블록구조를_개행으로_살린다() {
        assert_eq!(html_to_text("<p>가</p><p>나</p>"), "\n가\n\n나\n");
        assert_eq!(html_to_text("가<br>나"), "가\n나");
        assert_eq!(html_to_text("<td>가</td><td>나</td>"), "\t가\t\t나\t"); // 셀은 탭
        assert_eq!(html_to_text("<span>가</span>나"), "가나");            // 인라인은 그대로
    }

    #[test]
    fn html_to_text는_엔티티를_디코드한다() {
        assert_eq!(html_to_text("a&nbsp;b"), "a b");
        assert_eq!(html_to_text("&lt;tag&gt;"), "<tag>");
        assert_eq!(html_to_text("&quot;q&quot; &#39;s&#39; &amp;"), "\"q\" 's' &");
    }

    #[test]
    fn collapse_ws는_빈줄을_최대_한개만_남긴다() {
        assert_eq!(collapse_ws("가\n\n\n\n나"), "가\n\n나");
        assert_eq!(collapse_ws("  가   나  "), "가 나");
        assert_eq!(collapse_ws("\n\n가\n\n"), "가"); // 앞뒤는 trim
        assert!(collapse_ws("가\n나").contains('\n'), "게시판/메일은 개행을 보존해야 한다");
    }

    #[test]
    fn truncate는_바이트가_아니라_문자로_자른다() {
        assert_eq!(truncate("가나다라마", 3), "가나다…");
        assert_eq!(truncate("가나다", 3), "가나다");   // 경계값: 자르지 않음
        assert_eq!(truncate("가나다", 10), "가나다");
        assert_eq!(truncate("", 3), "");
        assert_eq!(truncate("가나다라", 0), "…");
        // 멀티바이트를 바이트로 자르면 패닉이 난다 — 안 나는 것 자체가 계약이다.
        assert_eq!(truncate("🙂🙂🙂", 2).chars().count(), 3); // 이모지 2 + 말줄임
    }

    #[test]
    fn split_uids는_공백과_빈값을_거른다() {
        assert_eq!(split_uids("a,b,c"), vec!["a", "b", "c"]);
        assert_eq!(split_uids(" a , b "), vec!["a", "b"]);
        assert_eq!(split_uids("a,,b,"), vec!["a", "b"]);
        assert!(split_uids("").is_empty());
        assert!(split_uids(" , ").is_empty());
    }
}
