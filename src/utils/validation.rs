use std::net::IpAddr;

pub fn assert_wallet_encryption_key(key: &str) {
    let key = key.trim();
    if key.len() != 64 || !key.chars().all(|c| c.is_ascii_hexdigit()) {
        panic!("WALLET_ENCRYPTION_KEY must be exactly 64 hexadecimal characters (32 bytes)");
    }
    let decoded = hex::decode(key).expect("WALLET_ENCRYPTION_KEY hex decode");
    assert_eq!(
        decoded.len(),
        32,
        "WALLET_ENCRYPTION_KEY must decode to 32 bytes"
    );
}

pub fn assert_wallet_master_seed(seed: &str) {
    let seed = seed.trim();
    let decoded = hex::decode(seed).unwrap_or_else(|_| {
        panic!("WALLET_MASTER_SEED must be valid hexadecimal");
    });
    assert!(
        decoded.len() >= 32,
        "WALLET_MASTER_SEED must decode to at least 32 bytes (64 hex chars minimum)"
    );
}

pub fn assert_public_https_url(name: &str, url: &str) {
    let url = url.trim();
    let parsed = reqwest::Url::parse(url).unwrap_or_else(|_| panic!("{name} must be a valid URL"));
    assert_eq!(parsed.scheme(), "https", "{name} must use https scheme");
    let host = parsed
        .host_str()
        .unwrap_or_else(|| panic!("{name} must include a hostname"));
    assert!(!host.is_empty(), "{name} hostname must not be empty");
    if let Ok(ip) = host.parse::<IpAddr>() {
        assert!(
            !ip.is_loopback() && !ip.is_unspecified(),
            "{name} must not use loopback or unspecified IP as host"
        );
    }
}

pub fn is_valid_merchant_code_param(code: &str) -> bool {
    let c = code.trim();
    c.len() == 6 && c.chars().all(|ch| ch.is_ascii_digit())
}
