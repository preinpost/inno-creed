//! 요청 서명: wehago-sign = Base64(HMAC_SHA256(authToken + transactionId + timestamp + urlPath, signKey))

use base64::{engine::general_purpose::STANDARD, Engine};
use hmac::{Hmac, KeyInit, Mac};
use rand::Rng;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// 요청별 32 hex 랜덤 transaction-id
pub fn transaction_id() -> String {
    let mut b = [0u8; 16];
    rand::rng().fill_bytes(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// wehago-sign 계산. 입력을 구분자 없이 결합해 HMAC-SHA256 후 Base64.
pub fn wehago_sign(auth_token: &str, tid: &str, ts: &str, path: &str, sign_key: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(sign_key.as_bytes()).expect("HMAC key");
    mac.update(auth_token.as_bytes());
    mac.update(tid.as_bytes());
    mac.update(ts.as_bytes());
    mac.update(path.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}
