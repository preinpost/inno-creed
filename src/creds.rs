//! 크레덴셜(authToken/signKey) 취득 — Chrome/Firefox, macOS·Linux·Windows 크로스플랫폼.
//! Chrome 쿠키 복호화는 OS마다 방식이 다르다:
//!  · macOS  : 키체인 `Chrome Safe Storage` → PBKDF2(SHA1,1003) → AES-128-CBC(iv=0x20×16)
//!  · Linux  : 고정 비번 "peanuts"(키링 미사용시) → PBKDF2(SHA1,1) → AES-128-CBC(iv=0x20×16)
//!  · Windows: Local State의 DPAPI 래핑 키 복호화 → AES-256-GCM
//! Firefox `cookies.sqlite`는 전 OS 평문이라 프로필 경로만 OS별로 분기한다.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

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

// ─────────────────────────────── Chrome ───────────────────────────────

/// Chrome 쿠키에서 크레덴셜 취득(OS별 복호화).
pub fn from_chrome() -> Result<Creds> {
    let cookies = read_chrome_cookies().context("Chrome 쿠키 DB 읽기 실패")?;
    let key = chrome_key().context("Chrome 복호화 키 취득 실패")?;

    let mut auth_token = None;
    let mut sign_key = None;
    for (name, enc) in cookies {
        let val = match decrypt_chrome(&enc, &key) {
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

/// Chrome User Data 루트 디렉토리(OS별).
fn chrome_user_data_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME")?;
        return Ok(PathBuf::from(format!(
            "{home}/Library/Application Support/Google/Chrome"
        )));
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME")?;
        let gc = PathBuf::from(format!("{home}/.config/google-chrome")); // 스테이블 우선
        if gc.exists() {
            return Ok(gc);
        }
        return Ok(PathBuf::from(format!("{home}/.config/chromium"))); // Chromium 폴백
    }
    #[cfg(target_os = "windows")]
    {
        let local = std::env::var("LOCALAPPDATA")?;
        return Ok(PathBuf::from(format!("{local}\\Google\\Chrome\\User Data")));
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("지원하지 않는 OS")
    }
}

/// 쿠키 DB 경로 — 신버전(Default/Network/Cookies) 우선, 없으면 구버전(Default/Cookies).
fn chrome_cookie_db() -> Result<PathBuf> {
    let root = chrome_user_data_dir()?;
    let net = root.join("Default").join("Network").join("Cookies");
    if net.exists() {
        return Ok(net);
    }
    let old = root.join("Default").join("Cookies");
    if old.exists() {
        return Ok(old);
    }
    bail!("Chrome 쿠키 DB를 찾을 수 없음(루트: {})", root.display())
}

fn read_chrome_cookies() -> Result<Vec<(String, Vec<u8>)>> {
    let db = chrome_cookie_db()?;
    // 잠금 회피: 복사본을 읽음.
    let tmp = std::env::temp_dir().join("inno_creed_ck.db");
    std::fs::copy(&db, &tmp)?;
    let conn = rusqlite::Connection::open(&tmp)?;
    let mut stmt =
        conn.prepare("SELECT name, encrypted_value FROM cookies WHERE host_key='gw.innogrid.com'")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
    let mut out = Vec::new();
    for r in rows {
        out.push(r?);
    }
    drop(stmt);
    drop(conn);
    let _ = std::fs::remove_file(&tmp);
    Ok(out)
}

/// OS별 Chrome 복호화 키. mac/linux=AES-128-CBC용 16B, windows=AES-256-GCM용 32B.
#[cfg(target_os = "macos")]
fn chrome_key() -> Result<Vec<u8>> {
    let pw = keychain_password().context("Chrome Safe Storage 키체인 접근 실패")?;
    Ok(pbkdf2_key(&pw, 1003, 16))
}
#[cfg(target_os = "linux")]
fn chrome_key() -> Result<Vec<u8>> {
    // 키링(gnome-keyring/kwallet) 미사용 Chrome은 고정 비번 "peanuts", 반복 1회.
    // 키링 사용(v11 쿠키)이면 이 키로 복호화 실패 → Firefox로 폴백된다.
    Ok(pbkdf2_key(b"peanuts", 1, 16))
}
#[cfg(target_os = "windows")]
fn chrome_key() -> Result<Vec<u8>> {
    windows_chrome_key()
}

#[cfg(not(target_os = "windows"))]
fn pbkdf2_key(pw: &[u8], iters: u32, len: usize) -> Vec<u8> {
    let mut key = vec![0u8; len];
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(pw, b"saltysalt", iters, &mut key);
    key
}

/// mac/linux: v10 접두(3B) 제거 → AES-128-CBC(iv=0x20×16, Pkcs7).
#[cfg(not(target_os = "windows"))]
fn decrypt_chrome(enc: &[u8], key: &[u8]) -> Result<String> {
    use aes::Aes128;
    use cbc::cipher::{block_padding::Pkcs7, BlockModeDecrypt, KeyIvInit};
    type Dec = cbc::Decryptor<Aes128>;

    if enc.len() < 3 {
        bail!("encrypted value too short");
    }
    let k: [u8; 16] = key.try_into().map_err(|_| anyhow::anyhow!("CBC 키 길이 오류"))?;
    let iv = [0x20u8; 16];
    let mut buf = enc[3..].to_vec();
    let pt = Dec::new(&k.into(), &iv.into())
        .decrypt_padded::<Pkcs7>(&mut buf)
        .map_err(|e| anyhow::anyhow!("AES-CBC 복호화 실패: {e}"))?;
    Ok(strip_domain_hash(pt.to_vec()))
}

/// windows: v10 접두(3B) + nonce(12B) + ciphertext + tag(16B) → AES-256-GCM.
#[cfg(target_os = "windows")]
fn decrypt_chrome(enc: &[u8], key: &[u8]) -> Result<String> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    if enc.len() < 3 + 12 + 16 {
        bail!("gcm cookie too short");
    }
    let nonce = Nonce::try_from(&enc[3..15]).map_err(|_| anyhow::anyhow!("GCM nonce 길이 오류"))?;
    let ct = &enc[15..];
    let cipher =
        Aes256Gcm::new_from_slice(key).map_err(|_| anyhow::anyhow!("GCM 키 길이 오류(32B 필요)"))?;
    let pt = cipher
        .decrypt(&nonce, ct)
        .map_err(|_| anyhow::anyhow!("AES-GCM 복호화 실패(app-bound 암호화면 미지원)"))?;
    Ok(strip_domain_hash(pt))
}

/// 최신 Chrome은 평문 앞에 32B 도메인 SHA256을 붙인다 — utf8 파싱 실패 시 앞 32B 제거.
fn strip_domain_hash(pt: Vec<u8>) -> String {
    match std::str::from_utf8(&pt) {
        Ok(s) => s.to_string(),
        Err(_) => {
            let start = 32.min(pt.len());
            String::from_utf8_lossy(&pt[start..]).into_owned()
        }
    }
}

#[cfg(target_os = "macos")]
fn keychain_password() -> Result<Vec<u8>> {
    use std::process::Command;
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

/// Windows: Local State의 `os_crypt.encrypted_key`(base64, "DPAPI" 접두) → DPAPI 복호화 → 32B AES 키.
#[cfg(target_os = "windows")]
fn windows_chrome_key() -> Result<Vec<u8>> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let root = chrome_user_data_dir()?;
    let ls = root.join("Local State");
    let txt = std::fs::read_to_string(&ls)
        .with_context(|| format!("Local State 읽기 실패: {}", ls.display()))?;
    let v: serde_json::Value = serde_json::from_str(&txt)?;
    let b64 = v
        .get("os_crypt")
        .and_then(|o| o.get("encrypted_key"))
        .and_then(|k| k.as_str())
        .context("Local State에 os_crypt.encrypted_key 없음")?;
    let mut raw = STANDARD
        .decode(b64)
        .context("encrypted_key base64 디코드 실패")?;
    if raw.len() >= 5 && &raw[..5] == b"DPAPI" {
        raw.drain(0..5);
    }
    dpapi_unprotect(&raw).context("DPAPI 키 복호화 실패")
}

/// Windows DPAPI `CryptUnprotectData` FFI(crypt32). 현재 사용자 컨텍스트로 복호화.
#[cfg(target_os = "windows")]
fn dpapi_unprotect(data: &[u8]) -> Result<Vec<u8>> {
    use core::ffi::c_void;
    #[repr(C)]
    struct DataBlob {
        cb_data: u32,
        pb_data: *mut u8,
    }
    #[link(name = "crypt32")]
    unsafe extern "system" {
        fn CryptUnprotectData(
            p_data_in: *const DataBlob,
            ppsz_desc: *mut *mut u16,
            p_entropy: *const DataBlob,
            p_reserved: *mut c_void,
            p_prompt: *mut c_void,
            dw_flags: u32,
            p_data_out: *mut DataBlob,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn LocalFree(h_mem: *mut c_void) -> *mut c_void;
    }

    let in_blob = DataBlob {
        cb_data: data.len() as u32,
        pb_data: data.as_ptr() as *mut u8,
    };
    let mut out_blob = DataBlob {
        cb_data: 0,
        pb_data: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &in_blob,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            &mut out_blob,
        )
    };
    if ok == 0 {
        bail!("CryptUnprotectData 실패");
    }
    let out =
        unsafe { std::slice::from_raw_parts(out_blob.pb_data, out_blob.cb_data as usize).to_vec() };
    unsafe {
        LocalFree(out_blob.pb_data as *mut c_void);
    }
    Ok(out)
}

// ─────────────────────────────── Firefox ───────────────────────────────

/// Firefox 프로필 루트 디렉토리(OS별). 하위에 `*.default*` 프로필들이 있다.
fn firefox_profiles_dir() -> Result<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME")?;
        return Ok(PathBuf::from(format!(
            "{home}/Library/Application Support/Firefox/Profiles"
        )));
    }
    #[cfg(target_os = "linux")]
    {
        let home = std::env::var("HOME")?;
        return Ok(PathBuf::from(format!("{home}/.mozilla/firefox")));
    }
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA")?;
        return Ok(PathBuf::from(format!(
            "{appdata}\\Mozilla\\Firefox\\Profiles"
        )));
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        bail!("지원하지 않는 OS")
    }
}

/// Firefox 쿠키에서 크레덴셜 취득. `cookies.sqlite`는 **평문**이라 복호화 불필요.
/// 프로필 자동 탐색(`*.default*` 우선). Chrome이 없거나 미로그인일 때의 폴백.
pub fn from_firefox() -> Result<Creds> {
    let profiles = firefox_profiles_dir()?;

    let mut db_path = None;
    for entry in std::fs::read_dir(&profiles)
        .with_context(|| format!("Firefox 프로필 디렉토리 없음: {}", profiles.display()))?
    {
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
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
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

/// authToken은 `%7C`(=`|`)로 URL 인코딩되어 저장됨.
fn url_decode(s: &str) -> String {
    s.replace("%7C", "|").replace("%7c", "|")
}
