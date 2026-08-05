//! 크레덴셜(authToken/signKey) 취득 — Chrome/Firefox, macOS·Linux·Windows 크로스플랫폼.
//! Chrome 쿠키 복호화는 OS마다 방식이 다르다:
//!  · macOS  : 키체인 `Chrome Safe Storage` → PBKDF2(SHA1,1003) → AES-128-CBC(iv=0x20×16)
//!  · Linux  : 고정 비번 "peanuts"(키링 미사용시) → PBKDF2(SHA1,1) → AES-128-CBC(iv=0x20×16)
//!  · Windows: v10=Local State DPAPI 키, v20(app-bound)=Chrome Elevator COM(IElevator::DecryptData) → AES-256-GCM
//! Firefox `cookies.sqlite`는 전 OS 평문이라 프로필 경로만 OS별로 분기한다.

use anyhow::{bail, Context, Result};
use std::path::PathBuf;

#[derive(Clone)]
pub struct Creds {
    pub auth_token: String,
    pub sign_key: String,
}

/// 크레덴셜 취득 진입점: 수동 입력(env) → Chrome → Firefox 순, 모두 실패면 로그인 안내 에러.
pub fn from_browser() -> Result<Creds> {
    // 0) 수동 입력 — 브라우저 복호화가 불가한 환경(Windows app-bound v20 등)의 확실한 우회.
    //    두 값 모두 지정돼 있으면 그대로 사용(authToken은 URL 인코딩 허용).
    if let (Some(at), Some(hk)) = (
        env_nonempty("INNO_CREED_AUTH_TOKEN"),
        env_nonempty("INNO_CREED_SIGN_KEY"),
    ) {
        return Ok(Creds {
            auth_token: url_decode(&at),
            sign_key: hk,
        });
    }
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
         [Chrome] {chrome_err:#}\n\
         [Firefox] {firefox_err:#}\n\
         해결: Chrome 또는 Firefox로 https://gw.innogrid.com 에 로그인한 뒤 다시 실행하세요.\n\
         비표준 경로(snap/flatpak/커스텀 프로필)는 환경변수로 지정할 수 있습니다:\n\
         · INNO_CREED_FIREFOX_COOKIES = <cookies.sqlite 경로>   (또는 INNO_CREED_FIREFOX_DIR = <프로필 디렉토리>)\n\
         · INNO_CREED_CHROME_COOKIES  = <Cookies DB 경로>       (또는 INNO_CREED_CHROME_USER_DATA = <User Data 루트>)\n\
         브라우저에서 못 가져오면 값을 직접 지정할 수도 있습니다(DevTools→Application→Cookies→gw.innogrid.com):\n\
         · INNO_CREED_AUTH_TOKEN = <BIZCUBE_AT 값>  ·  INNO_CREED_SIGN_KEY = <BIZCUBE_HK 값>"
    )
}

// ─────────────────────────────── Chrome ───────────────────────────────

/// Chrome 쿠키에서 크레덴셜 취득(OS별 복호화).
pub fn from_chrome() -> Result<Creds> {
    let db = chrome_cookie_db()?;
    let cookies = read_chrome_cookies(&db)
        .with_context(|| format!("Chrome 쿠키 DB 읽기 실패: {}", db.display()))?;
    let key = chrome_key().context("Chrome 복호화 키 취득 실패(mac 키체인/win DPAPI)")?;

    let mut auth_token = None;
    let mut sign_key = None;
    let mut decrypt_failed = false; // 대상 쿠키는 있으나 복호화만 실패한 경우 구분
    for (name, enc) in cookies {
        if name != "BIZCUBE_AT" && name != "BIZCUBE_HK" {
            continue;
        }
        match decrypt_chrome(&enc, &key) {
            Ok(val) => match name.as_str() {
                "BIZCUBE_AT" => auth_token = Some(url_decode(&val)),
                "BIZCUBE_HK" => sign_key = Some(val),
                _ => {}
            },
            Err(_) => decrypt_failed = true,
        }
    }
    // 쿠키는 있는데 복호화만 실패 → 키 스킴 불일치(키링/app-bound). "없음"과 구분해 안내.
    if auth_token.is_none() && decrypt_failed {
        bail!(
            "BIZCUBE 쿠키는 있으나 복호화 실패. Linux 키링(gnome-keyring/kwallet, v11) 사용 시 `secret-tool`(libsecret-tools)이 설치돼 있어야 합니다 — `sudo apt install libsecret-tools` 후 재시도. \
             Windows app-bound(v20)은 Chrome Elevator COM으로 복호화를 시도하나 Chrome 버전/보안설정에 따라 거부될 수 있습니다. \
             확실한 대안: (1) Firefox로 gw.innogrid.com 로그인, 또는 (2) INNO_CREED_AUTH_TOKEN·INNO_CREED_SIGN_KEY 환경변수로 쿠키 값을 직접 지정."
        );
    }
    Ok(Creds {
        auth_token: auth_token.with_context(|| {
            format!(
                "쿠키 DB({})는 읽었으나 BIZCUBE_AT 없음 — 이 프로필로 gw.innogrid.com 로그인이 안 돼 있습니다(다른 프로필이면 INNO_CREED_CHROME_COOKIES로 지정).",
                db.display()
            )
        })?,
        sign_key: sign_key.context("BIZCUBE_HK 쿠키 없음(로그인 필요)")?,
    })
}

/// Chrome User Data 루트 디렉토리(OS별). `INNO_CREED_CHROME_USER_DATA`로 오버라이드 가능.
fn chrome_user_data_dir() -> Result<PathBuf> {
    if let Some(p) = env_nonempty("INNO_CREED_CHROME_USER_DATA") {
        return Ok(PathBuf::from(p));
    }
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

/// 쿠키 DB 경로. `INNO_CREED_CHROME_COOKIES`로 직접 지정 가능. 미지정 시 User Data 루트 아래
/// 신버전(Default/Network/Cookies) 우선, 없으면 구버전(Default/Cookies).
fn chrome_cookie_db() -> Result<PathBuf> {
    if let Some(p) = env_nonempty("INNO_CREED_CHROME_COOKIES") {
        return Ok(PathBuf::from(p));
    }
    let root = chrome_user_data_dir()?;
    let net = root.join("Default").join("Network").join("Cookies");
    if net.exists() {
        return Ok(net);
    }
    let old = root.join("Default").join("Cookies");
    if old.exists() {
        return Ok(old);
    }
    bail!(
        "Chrome 쿠키 DB를 찾지 못함. 확인한 경로:\n      - {}\n      - {}\n    (User Data 루트: {})\n    비표준 위치(snap/flatpak/커스텀 프로필)면 INNO_CREED_CHROME_COOKIES(파일) 또는 INNO_CREED_CHROME_USER_DATA(루트)로 지정하세요.",
        net.display(),
        old.display(),
        root.display()
    )
}

fn read_chrome_cookies(db: &std::path::Path) -> Result<Vec<(String, Vec<u8>)>> {
    // 잠금 회피: 복사본을 읽음. (Windows에서 Chrome 실행 중이면 배타 잠금이라 copy 실패 → Chrome 종료 필요.)
    let tmp = std::env::temp_dir().join("inno_creed_ck.db");
    std::fs::copy(db, &tmp)?;
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
    // 1) 키링(gnome-keyring/kwallet)에 저장된 "Chrome Safe Storage" 비밀 시도(v11 쿠키).
    // 2) 키링 미사용 Chrome은 고정 비번 "peanuts"(v10). 둘 다 PBKDF2 반복 1회.
    if let Some(secret) = linux_keyring_secret() {
        return Ok(pbkdf2_key(secret.as_bytes(), 1, 16));
    }
    Ok(pbkdf2_key(b"peanuts", 1, 16))
}

/// Secret Service(gnome-keyring/kwallet)에서 Chrome/Chromium 저장소 키 조회. `secret-tool`(libsecret-tools) 필요.
/// 없거나 조회 실패면 None → 호출부가 "peanuts"로 폴백.
#[cfg(target_os = "linux")]
fn linux_keyring_secret() -> Option<String> {
    for app in ["chrome", "chromium"] {
        let out = std::process::Command::new("secret-tool")
            .args(["lookup", "application", app])
            .output()
            .ok()?; // secret-tool 자체가 없으면 None(→ peanuts 폴백)
        if out.status.success() && !out.stdout.is_empty() {
            let s = String::from_utf8_lossy(&out.stdout)
                .trim_end_matches(['\n', '\r'])
                .to_string();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    None
}
/// Windows Chrome 키 두 종류. 쿠키 접두(v10/v20)에 따라 골라 쓴다.
#[cfg(target_os = "windows")]
struct ChromeKey {
    v10: Option<Vec<u8>>, // os_crypt.encrypted_key(DPAPI) → 구형 v10 쿠키
    v20: Option<Vec<u8>>, // os_crypt.app_bound_encrypted_key(IElevator COM) → app-bound v20 쿠키
}

#[cfg(target_os = "windows")]
fn chrome_key() -> Result<ChromeKey> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let root = chrome_user_data_dir()?;
    let ls = root.join("Local State");
    let txt = std::fs::read_to_string(&ls)
        .with_context(|| format!("Local State 읽기 실패: {}", ls.display()))?;
    let v: serde_json::Value = serde_json::from_str(&txt)?;
    let os_crypt = v.get("os_crypt");

    // v10: DPAPI 래핑 키("DPAPI" 접두 제거 → CryptUnprotectData).
    let v10 = os_crypt
        .and_then(|o| o.get("encrypted_key"))
        .and_then(|k| k.as_str())
        .and_then(|b64| STANDARD.decode(b64).ok())
        .and_then(|mut raw| {
            if raw.len() >= 5 && &raw[..5] == b"DPAPI" {
                raw.drain(0..5);
            }
            dpapi_unprotect(&raw).ok()
        });

    // v20: app-bound 키("APPB" 접두 제거 → Chrome Elevator COM). 필드 없거나 거부되면 None(→ v10/Firefox/수동 폴백).
    let v20 = os_crypt
        .and_then(|o| o.get("app_bound_encrypted_key"))
        .and_then(|k| k.as_str())
        .and_then(|b64| STANDARD.decode(b64).ok())
        .and_then(|mut raw| {
            if raw.len() >= 4 && &raw[..4] == b"APPB" {
                raw.drain(0..4);
            }
            app_bound_key(&raw).ok()
        });

    if v10.is_none() && v20.is_none() {
        bail!("Chrome 복호화 키 취득 실패 — Local State에 os_crypt 키가 없거나 복호화 불가");
    }
    Ok(ChromeKey { v10, v20 })
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

/// windows: 접두(3B) + nonce(12B) + ciphertext + tag(16B) → AES-256-GCM.
/// 접두가 "v20"이면 app-bound 키, 그 외("v10")면 DPAPI 키를 사용한다.
#[cfg(target_os = "windows")]
fn decrypt_chrome(enc: &[u8], key: &ChromeKey) -> Result<String> {
    use aes_gcm::aead::{Aead, KeyInit};
    use aes_gcm::{Aes256Gcm, Nonce};

    if enc.len() < 3 + 12 + 16 {
        bail!("gcm cookie too short");
    }
    let k = if &enc[..3] == b"v20" {
        key.v20
            .as_deref()
            .context("v20(app-bound) 쿠키인데 app-bound 키 취득 실패")?
    } else {
        key.v10
            .as_deref()
            .context("v10 쿠키인데 os_crypt DPAPI 키 취득 실패")?
    };
    let nonce = Nonce::try_from(&enc[3..15]).map_err(|_| anyhow::anyhow!("GCM nonce 길이 오류"))?;
    let ct = &enc[15..];
    let cipher =
        Aes256Gcm::new_from_slice(k).map_err(|_| anyhow::anyhow!("GCM 키 길이 오류(32B 필요)"))?;
    let pt = cipher
        .decrypt(&nonce, ct)
        .map_err(|_| anyhow::anyhow!("AES-GCM 복호화 실패"))?;
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

/// Windows app-bound(v20) 키: `os_crypt.app_bound_encrypted_key`("APPB" 제거분)를
/// Chrome Elevation Service(IElevator::DecryptData) COM 호출로 복호화 → 마지막 32B가 AES-256 키.
/// Chrome 버전/보안 정책상 호출자를 거부할 수 있어(best-effort) 실패 시 Err → 상위에서 폴백.
#[cfg(target_os = "windows")]
fn app_bound_key(blob: &[u8]) -> Result<Vec<u8>> {
    use core::ffi::c_void;

    #[repr(C)]
    struct Guid {
        d1: u32,
        d2: u16,
        d3: u16,
        d4: [u8; 8],
    }
    // Google Chrome(stable) Elevator. (Chromium/Edge/Brave는 CLSID/IID가 다름.)
    const CLSID_ELEVATOR: Guid = Guid {
        d1: 0x708860E0,
        d2: 0xF641,
        d3: 0x4611,
        d4: [0x88, 0x95, 0x7D, 0x86, 0x7D, 0xD3, 0x67, 0x5B],
    };
    const IID_IELEVATOR: Guid = Guid {
        d1: 0x463ABECF,
        d2: 0x410D,
        d3: 0x407F,
        d4: [0x8A, 0xF5, 0x0D, 0xF3, 0x5A, 0x00, 0x5C, 0xC8],
    };

    // IElevator vtable(IUnknown 상속). DecryptData는 IUnknown(3) + RunRecovery + EncryptData 다음(6번째) 슬롯.
    #[repr(C)]
    struct IElevatorVtbl {
        query_interface:
            unsafe extern "system" fn(*mut c_void, *const Guid, *mut *mut c_void) -> i32,
        add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
        release: unsafe extern "system" fn(*mut c_void) -> u32,
        run_recovery: *const c_void, // 미사용(슬롯 자리맞춤)
        encrypt_data: *const c_void, // 미사용(슬롯 자리맞춤)
        decrypt_data: unsafe extern "system" fn(
            *mut c_void,
            *mut u16,       // [in] BSTR ciphertext
            *mut *mut u16,  // [out] BSTR* plaintext
            *mut u32,       // [out] DWORD* last_error
        ) -> i32,
    }
    #[repr(C)]
    struct IElevator {
        vtbl: *const IElevatorVtbl,
    }

    #[link(name = "ole32")]
    unsafe extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, co_init: u32) -> i32;
        fn CoUninitialize();
        fn CoCreateInstance(
            rclsid: *const Guid,
            outer: *mut c_void,
            ctx: u32,
            riid: *const Guid,
            ppv: *mut *mut c_void,
        ) -> i32;
        fn CoSetProxyBlanket(
            proxy: *mut c_void,
            authn_svc: u32,
            authz_svc: u32,
            princ: *mut u16,
            authn_level: u32,
            imp_level: u32,
            auth_info: *mut c_void,
            capabilities: u32,
        ) -> i32;
    }
    #[link(name = "oleaut32")]
    unsafe extern "system" {
        fn SysAllocStringByteLen(psz: *const u8, len: u32) -> *mut u16;
        fn SysFreeString(bstr: *mut u16);
        fn SysStringByteLen(bstr: *mut u16) -> u32;
    }

    const COINIT_APARTMENTTHREADED: u32 = 0x2;
    const CLSCTX_LOCAL_SERVER: u32 = 0x4;
    const RPC_C_AUTHN_DEFAULT: u32 = 0xFFFF_FFFF;
    const RPC_C_AUTHZ_DEFAULT: u32 = 0xFFFF_FFFF;
    const RPC_C_AUTHN_LEVEL_PKT_PRIVACY: u32 = 6;
    const RPC_C_IMP_LEVEL_IMPERSONATE: u32 = 3;
    const EOAC_DYNAMIC_CLOAKING: u32 = 0x40;

    unsafe {
        let hr = CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
        // S_OK(0)/S_FALSE(1)만 우리가 초기화한 것 → 나중에 CoUninitialize 대상.
        let did_init = hr == 0 || hr == 1;

        let mut elevator: *mut c_void = std::ptr::null_mut();
        let hr = CoCreateInstance(
            &CLSID_ELEVATOR,
            std::ptr::null_mut(),
            CLSCTX_LOCAL_SERVER,
            &IID_IELEVATOR,
            &mut elevator,
        );
        if hr < 0 || elevator.is_null() {
            if did_init {
                CoUninitialize();
            }
            bail!("Chrome Elevator COM 생성 실패(hr=0x{:08X})", hr as u32);
        }
        let vt = (*(elevator as *mut IElevator)).vtbl;

        let hr = CoSetProxyBlanket(
            elevator,
            RPC_C_AUTHN_DEFAULT,
            RPC_C_AUTHZ_DEFAULT,
            std::ptr::null_mut(),
            RPC_C_AUTHN_LEVEL_PKT_PRIVACY,
            RPC_C_IMP_LEVEL_IMPERSONATE,
            std::ptr::null_mut(),
            EOAC_DYNAMIC_CLOAKING,
        );
        if hr < 0 {
            ((*vt).release)(elevator);
            if did_init {
                CoUninitialize();
            }
            bail!("CoSetProxyBlanket 실패(hr=0x{:08X})", hr as u32);
        }

        let in_bstr = SysAllocStringByteLen(blob.as_ptr(), blob.len() as u32);
        let mut out_bstr: *mut u16 = std::ptr::null_mut();
        let mut last_error: u32 = 0;
        let hr = ((*vt).decrypt_data)(elevator, in_bstr, &mut out_bstr, &mut last_error);

        let result = if hr >= 0 && !out_bstr.is_null() {
            let len = SysStringByteLen(out_bstr) as usize;
            Ok(std::slice::from_raw_parts(out_bstr as *const u8, len).to_vec())
        } else {
            Err(anyhow::anyhow!(
                "IElevator::DecryptData 실패(hr=0x{:08X}, last_error={}) — Chrome이 호출자를 신뢰하지 않거나 버전 불일치",
                hr as u32,
                last_error
            ))
        };

        if !in_bstr.is_null() {
            SysFreeString(in_bstr);
        }
        if !out_bstr.is_null() {
            SysFreeString(out_bstr);
        }
        ((*vt).release)(elevator);
        if did_init {
            CoUninitialize();
        }

        let bytes = result?;
        if bytes.len() < 32 {
            bail!("app-bound 복호화 결과가 32B 미만({}B)", bytes.len());
        }
        // 반환 blob의 마지막 32B가 AES-256 키.
        Ok(bytes[bytes.len() - 32..].to_vec())
    }
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

/// Firefox 프로필 루트 디렉토리(OS별). `INNO_CREED_FIREFOX_DIR`로 오버라이드 가능
/// (snap `~/snap/firefox/common/.mozilla/firefox`, flatpak 등 비표준 경로 대응).
fn firefox_profiles_dir() -> Result<PathBuf> {
    if let Some(p) = env_nonempty("INNO_CREED_FIREFOX_DIR") {
        return Ok(PathBuf::from(p));
    }
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
    // 1) cookies.sqlite 직접 지정(최우선) 2) 프로필 디렉토리 스캔.
    let src = if let Some(p) = env_nonempty("INNO_CREED_FIREFOX_COOKIES") {
        let pb = PathBuf::from(&p);
        if !pb.exists() {
            bail!("INNO_CREED_FIREFOX_COOKIES가 가리키는 파일 없음: {p}");
        }
        pb
    } else {
        let profiles = firefox_profiles_dir()?;
        let entries = std::fs::read_dir(&profiles).with_context(|| {
            format!(
                "Firefox 프로필 디렉토리 없음: {} — snap이면 ~/snap/firefox/common/.mozilla/firefox, flatpak이면 ~/.var/app/org.mozilla.firefox/.mozilla/firefox. INNO_CREED_FIREFOX_DIR(디렉토리) 또는 INNO_CREED_FIREFOX_COOKIES(파일)로 지정 가능.",
                profiles.display()
            )
        })?;
        let mut db_path = None;
        for entry in entries {
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
        db_path.with_context(|| {
            format!(
                "Firefox 프로필 디렉토리({})에 cookies.sqlite가 있는 프로필이 없음",
                profiles.display()
            )
        })?
    };

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
        auth_token: auth_token.with_context(|| {
            format!(
                "Firefox 쿠키({})에 BIZCUBE_AT 없음 — 이 프로필로 gw.innogrid.com 로그인이 안 돼 있습니다.",
                src.display()
            )
        })?,
        sign_key: sign_key.context("BIZCUBE_HK 쿠키 없음(Firefox 미로그인)")?,
    })
}

/// 환경변수가 설정되어 있고 비어있지 않으면 그 값을 반환.
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

/// authToken은 `%7C`(=`|`)로 URL 인코딩되어 저장됨.
fn url_decode(s: &str) -> String {
    s.replace("%7C", "|").replace("%7c", "|")
}
