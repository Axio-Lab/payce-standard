use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use rand::RngCore;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum CryptoError {
    #[error("Encryption failed")]
    EncryptionFailed,
    #[error("Decryption failed")]
    DecryptionFailed,
    #[error("Invalid ciphertext format")]
    InvalidFormat,
    #[error("Invalid key: {0}")]
    InvalidKey(String),
}

pub fn encrypt(plaintext: &str, key_hex: &str) -> Result<String, CryptoError> {
    let key_bytes = hex::decode(key_hex).map_err(|e| CryptoError::InvalidKey(e.to_string()))?;
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|e| CryptoError::InvalidKey(e.to_string()))?;

    let mut iv = [0u8; 12];
    OsRng.fill_bytes(&mut iv);
    let nonce = Nonce::from_slice(&iv);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|_| CryptoError::EncryptionFailed)?;

    Ok(format!("{}:{}", hex::encode(iv), hex::encode(ciphertext)))
}

pub fn decrypt(ciphertext: &str, key_hex: &str) -> Result<String, CryptoError> {
    let parts: Vec<&str> = ciphertext.split(':').collect();
    if parts.len() != 2 {
        return Err(CryptoError::InvalidFormat);
    }

    let iv = hex::decode(parts[0]).map_err(|_| CryptoError::InvalidFormat)?;
    let encrypted = hex::decode(parts[1]).map_err(|_| CryptoError::InvalidFormat)?;

    let key_bytes = hex::decode(key_hex).map_err(|e| CryptoError::InvalidKey(e.to_string()))?;
    let cipher = Aes256Gcm::new_from_slice(&key_bytes)
        .map_err(|e| CryptoError::InvalidKey(e.to_string()))?;

    let nonce = Nonce::from_slice(&iv);

    let plaintext = cipher
        .decrypt(nonce, encrypted.as_ref())
        .map_err(|_| CryptoError::DecryptionFailed)?;

    String::from_utf8(plaintext).map_err(|_| CryptoError::DecryptionFailed)
}
