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

#[cfg(test)]
mod tests {
    use super::*;

    /// 골든 테스트 — 서명 규격(docs/architecture.md §5)이 바뀌면 즉시 깨진다.
    /// 기대값은 이 구현이 아니라 **독립 계산**(python `hmac.new(key, token+tid+ts+path, sha256)`
    /// → base64)으로 뽑았다. 자기 자신을 정답으로 삼으면 골든 테스트가 아니다.
    #[test]
    fn wehago_sign은_고정입력에_고정서명을_낸다() {
        let sig = wehago_sign(
            "gcmsAmaranth31433|3166|test",
            "0123456789abcdef0123456789abcdef",
            "1700000000",
            "/gw/gw050A02",
            "SIGNKEY-abc",
        );
        assert_eq!(sig, "IIJvpAZ5u3uKLH5mGGgNoEtcnXVwplKL2pNErNz/PXc=");
    }

    /// 결합 순서(token‖tid‖ts‖path)가 규격이다 — 순서를 섞으면 서명이 달라져야 한다.
    #[test]
    fn 입력_순서가_서명에_반영된다() {
        let a = wehago_sign("A", "B", "C", "D", "k");
        let b = wehago_sign("B", "A", "C", "D", "k");
        assert_ne!(a, b, "입력 순서가 뒤바뀌어도 같은 서명이면 규격이 깨진 것");
    }

    #[test]
    fn transaction_id는_32자리_hex다() {
        let t = transaction_id();
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(t, transaction_id(), "매 요청 달라야 한다");
    }
}
