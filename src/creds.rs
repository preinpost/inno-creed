//! 크레덴셜(authToken/signKey) 취득.
//! Chrome(쿠키 복호화) → Firefox(cookies.sqlite 평문) 순으로 시도, 둘 다 없으면 로그인 안내.
//! Chrome: 키체인 `Chrome Safe Storage` → PBKDF2(SHA1,1003,16B) → AES-128-CBC(iv=공백16).

use aes::Aes128;
use anyhow::{bail, Context, Result};
use cbc::cipher::{block_padding::Pkcs7, BlockModeDecrypt, KeyIvInit};
use std::process::Command;

type Aes128CbcDec = cbc::Decryptor<Aes128>;

#[derive(Clone)]
pub struct Creds {
    pub auth_token: String,
    pub sign_key: String,
}

/// 크레덴셜 취득 진입점: Chrome → Firefox 순으로 시도, 둘 다 실패면 로그인 안내 에러.
pub fn from_browser() -> Result<Creds> {
    let chrome_err = match from_chrome() {
        Ok(c) => return Ok(c),
        Err(e) => e,
    };
    let firefox_err = match from_firefox() {
        Ok(c) => return Ok(c),
        Err(e) => e,
    };
    bail!(
        "크레덴셜 취득 실패 — gw.innogrid.com에 로그인된 브라우저가 필요합니다.\n\
         · Chrome: {chrome_err}\n\
         · Firefox: {firefox_err}\n\
         해결: Chrome 또는 Firefox로 https://gw.innogrid.com 에 로그인한 뒤 다시 실행하세요."
    )
}

/// Chrome 쿠키에서 크레덴셜 취득(macOS).
pub fn from_chrome() -> Result<Creds> {
    let pw = keychain_password().context("Chrome Safe Storage 키체인 접근 실패")?;
    let key = derive_key(&pw);
    let cookies = read_gw_cookies().context("Chrome 쿠키 DB 읽기 실패")?;

    let mut auth_token = None;
    let mut sign_key = None;
    for (name, enc) in cookies {
        let val = match decrypt(&enc, &key) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match name.as_str() {
            "BIZCUBE_AT" => auth_token = Some(url_decode(&val)),
            "BIZCUBE_HK" => sign_key = Some(val),
            _ => {}
        }
    }
    Ok(Creds {
        auth_token: auth_token.context("BIZCUBE_AT 쿠키 없음(로그인 필요)")?,
        sign_key: sign_key.context("BIZCUBE_HK 쿠키 없음(로그인 필요)")?,
    })
}

/// Firefox 쿠키에서 크레덴셜 취득(macOS). `cookies.sqlite`는 **평문**이라 복호화 불필요.
/// 프로필 자동 탐색(`*.default*` 우선). Chrome이 없거나 미로그인일 때의 폴백.
pub fn from_firefox() -> Result<Creds> {
    let home = std::env::var("HOME")?;
    let profiles = format!("{home}/Library/Application Support/Firefox/Profiles");

    // cookies.sqlite가 있는 프로필 탐색 — 이름에 "default" 포함 프로필 우선.
    let mut db_path = None;
    for entry in std::fs::read_dir(&profiles).context("Firefox 프로필 디렉토리 없음")? {
        let dir = entry?.path();
        let ck = dir.join("cookies.sqlite");
        if ck.exists() {
            let is_default = dir
                .file_name()
                .map(|n| n.to_string_lossy().contains("default"))
                .unwrap_or(false);
            if is_default {
                db_path = Some(ck);
                break;
            }
            db_path.get_or_insert(ck);
        }
    }
    let src = db_path.context("Firefox cookies.sqlite 없음")?;

    // 잠금 회피: 복사본을 읽음.
    let tmp = std::env::temp_dir().join("inno_creed_ff.db");
    std::fs::copy(&src, &tmp)?;
    let conn = rusqlite::Connection::open(&tmp)?;
    let mut stmt =
        conn.prepare("SELECT name, value FROM moz_cookies WHERE host LIKE '%gw.innogrid.com'")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut auth_token = None;
    let mut sign_key = None;
    for r in rows {
        let (name, val) = r?;
        match name.as_str() {
            "BIZCUBE_AT" => auth_token = Some(url_decode(&val)),
            "BIZCUBE_HK" => sign_key = Some(val),
            _ => {}
        }
    }
    drop(stmt);
    drop(conn);
    let _ = std::fs::remove_file(&tmp);

    Ok(Creds {
        auth_token: auth_token.context("BIZCUBE_AT 쿠키 없음(Firefox 미로그인)")?,
        sign_key: sign_key.context("BIZCUBE_HK 쿠키 없음(Firefox 미로그인)")?,
    })
}

fn keychain_password() -> Result<Vec<u8>> {
    let out = Command::new("security")
        .args([
            "find-generic-password",
            "-w",
            "-s",
            "Chrome Safe Storage",
            "-a",
            "Chrome",
        ])
        .output()?;
    if !out.status.success() {
        bail!("security 명령 실패");
    }
    let mut p = out.stdout;
    while p.last() == Some(&b'\n') {
        p.pop();
    }
    Ok(p)
}

fn derive_key(pw: &[u8]) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(pw, b"saltysalt", 1003, &mut key);
    key
}

fn read_gw_cookies() -> Result<Vec<(String, Vec<u8>)>> {
    let home = std::env::var("HOME")?;
    let src = format!("{home}/Library/Application Support/Google/Chrome/Default/Cookies");
    // 잠금 회피: 복사본을 읽음
    let tmp = std::env::temp_dir().join("inno_creed_ck.db");
    std::fs::copy(&src, &tmp)?;
    let conn = rusqlite::Connection::open(&tmp)?;
    let mut stmt =
        conn.prepare("SELECT name, encrypted_value FROM cookies WHERE host_key='gw.innogrid.com'")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    drop(stmt);
    drop(conn);
    let _ = std::fs::remove_file(&tmp);
    Ok(out)
}

/// v10/v11 접두 제거 → AES-128-CBC 복호화 → (최신 Chrome은 평문 앞 32B 도메인해시) 문자열화.
fn decrypt(enc: &[u8], key: &[u8; 16]) -> Result<String> {
    if enc.len() < 3 {
        bail!("encrypted value too short");
    }
    let iv = [0x20u8; 16];
    let mut buf = enc[3..].to_vec();
    let pt = Aes128CbcDec::new(key.into(), &iv.into())
        .decrypt_padded::<Pkcs7>(&mut buf)
        .map_err(|e| anyhow::anyhow!("AES-CBC 복호화 실패: {e}"))?;
    match std::str::from_utf8(pt) {
        Ok(s) => Ok(s.to_string()),
        Err(_) => {
            let start = 32.min(pt.len());
            Ok(String::from_utf8_lossy(&pt[start..]).into_owned())
        }
    }
}

/// authToken은 `%7C`(=`|`)로 URL 인코딩되어 저장됨.
fn url_decode(s: &str) -> String {
    s.replace("%7C", "|").replace("%7c", "|")
}
