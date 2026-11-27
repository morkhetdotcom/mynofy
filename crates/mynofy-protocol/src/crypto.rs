//! Only established provider primitives; no home-grown signature or encryption algorithms.
use crate::{ApiError, Result};
use aws_lc_rs::{
    aead, digest, hmac,
    rand::{SecureRandom, SystemRandom},
    signature::{self, Ed25519KeyPair, KeyPair},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde::{Serialize, de::DeserializeOwned};

pub fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| ApiError::internal())?;
    Ok(bytes)
}
pub fn encode(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}
pub fn decode<const N: usize>(text: &str) -> Result<[u8; N]> {
    let bytes = STANDARD.decode(text).map_err(|_| ApiError::invalid())?;
    if STANDARD.encode(&bytes) != text {
        return Err(ApiError::invalid());
    }
    bytes.try_into().map_err(|_| ApiError::invalid())
}
/// Decode the canonical padded rotation secret directly into its owning buffer.
pub fn decode_rotation_secret(text: &str) -> Result<zeroize::Zeroizing<[u8; 32]>> {
    let mut secret = zeroize::Zeroizing::new([0; 32]);
    if text.len() != 44 || STANDARD.decode_slice(text, secret.as_mut()).ok() != Some(32) {
        return Err(ApiError::invalid());
    }
    Ok(secret)
}

/// SHA-256 checksum for framed local storage; not an authenticity proof.
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let hash = digest::digest(&digest::SHA256, bytes);
    let mut result = [0; 32];
    result.copy_from_slice(hash.as_ref());
    result
}
pub fn token_id(public: &[u8]) -> String {
    format!(
        "ed25519:{}",
        STANDARD.encode(digest::digest(&digest::SHA256, public).as_ref())
    )
}
pub fn initial_client_id(public: &[u8]) -> String {
    format!("client:{}", token_id(public))
}
pub fn valid_client_id(id: &str) -> bool {
    id.strip_prefix("client:ed25519:")
        .is_some_and(|s| decode::<32>(s).is_ok())
}
pub fn auth_bytes(nonce: &[u8; 32], public: &[u8; 32], owner: &str) -> Vec<u8> {
    let mut bytes = b"mynofy-client-auth-v1\0".to_vec();
    bytes.extend(nonce);
    bytes.extend(public);
    bytes.extend((owner.len() as u32).to_be_bytes());
    bytes.extend(owner.as_bytes());
    bytes
}
pub fn verify(public: &[u8; 32], bytes: &[u8], signature: &[u8; 64]) -> Result<()> {
    signature::UnparsedPublicKey::new(&signature::ED25519, public)
        .verify(bytes, signature)
        .map_err(|_| ApiError::unauthorized())
}
pub fn generate_key() -> Result<Vec<u8>> {
    Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map(|d| d.as_ref().to_vec())
        .map_err(|_| ApiError::internal())
}
pub fn public_key(pkcs8: &[u8]) -> Result<[u8; 32]> {
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| ApiError::invalid())?;
    pair.public_key()
        .as_ref()
        .try_into()
        .map_err(|_| ApiError::invalid())
}
pub fn sign(pkcs8: &[u8], bytes: &[u8]) -> Result<String> {
    let pair = Ed25519KeyPair::from_pkcs8(pkcs8).map_err(|_| ApiError::invalid())?;
    Ok(encode(pair.sign(bytes).as_ref()))
}
pub struct CursorCodec(hmac::Key);
impl CursorCodec {
    pub fn new(secret: [u8; 32]) -> Self {
        Self(hmac::Key::new(hmac::HMAC_SHA256, &secret))
    }
    pub fn seal<T: Serialize>(&self, value: &T) -> Result<String> {
        let bytes = serde_json::to_vec(value).map_err(|_| ApiError::internal())?;
        Ok(format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(&bytes),
            URL_SAFE_NO_PAD.encode(hmac::sign(&self.0, &bytes).as_ref())
        ))
    }
    pub fn open<T: DeserializeOwned>(&self, text: &str) -> Result<T> {
        let bad = || {
            ApiError::new(
                "invalid_cursor",
                "Cursor is invalid or belongs to an old owner generation.",
            )
        };
        if text.len() > 4096 {
            return Err(bad());
        }
        let (body, mac) = text.split_once('.').ok_or_else(bad)?;
        let bytes = URL_SAFE_NO_PAD.decode(body).map_err(|_| bad())?;
        let tag = URL_SAFE_NO_PAD.decode(mac).map_err(|_| bad())?;
        hmac::verify(&self.0, &bytes, &tag).map_err(|_| bad())?;
        serde_json::from_slice(&bytes).map_err(|_| bad())
    }
}
/// AES-256-GCM: one random secret and nonce per bounded rotation transaction.
pub fn decrypt_rotation(
    secret: &[u8; 32],
    nonce: [u8; 12],
    aad: &[u8],
    encrypted: &str,
) -> Result<[u8; 32]> {
    let mut bytes = STANDARD
        .decode(encrypted)
        .map_err(|_| ApiError::invalid())?;
    if bytes.len() != 48 || STANDARD.encode(&bytes) != encrypted {
        return Err(ApiError::invalid());
    }
    let key =
        aead::UnboundKey::new(&aead::AES_256_GCM, secret).map_err(|_| ApiError::internal())?;
    let key = aead::LessSafeKey::new(key);
    key.open_in_place(
        aead::Nonce::assume_unique_for_key(nonce),
        aead::Aad::from(aad),
        &mut bytes,
    )
    .map_err(|_| ApiError::unauthorized())?
    .try_into()
    .map_err(|_| ApiError::invalid())
}
pub fn encrypt_rotation(
    secret: &[u8; 32],
    nonce: [u8; 12],
    aad: &[u8],
    public: &[u8; 32],
) -> Result<String> {
    let key =
        aead::UnboundKey::new(&aead::AES_256_GCM, secret).map_err(|_| ApiError::internal())?;
    let mut bytes = public.to_vec();
    aead::LessSafeKey::new(key)
        .seal_in_place_append_tag(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(aad),
            &mut bytes,
        )
        .map_err(|_| ApiError::internal())?;
    Ok(encode(&bytes))
}

pub fn rotation_aad(owner: &str, client: &str, rid: &str, old: &str, until: u64) -> Vec<u8> {
    // Length-delimited JSON tuple: clients construct it independently, never sign arbitrary server bytes.
    serde_json::to_vec(&("mynofy-rotation-v1", owner, client, rid, old, until)).unwrap_or_default()
}
pub fn rotation_proof_bytes(aad: &[u8], public: &[u8; 32], challenge: &[u8; 32]) -> Vec<u8> {
    let mut b = b"mynofy-rotation-proof-v1\0".to_vec();
    b.extend(aad);
    b.extend(public);
    b.extend(challenge);
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn verifies_existing_public_protocol_fixture_and_rejects_changed_owner() {
        let v: serde_json::Value = serde_json::from_str(include_str!(
            "../../../protocol/v1/client-auth-examples.json"
        ))
        .unwrap();
        let frames = &v["frames"];
        let public = decode(v["public_key"].as_str().unwrap()).unwrap();
        let nonce = decode(frames[1]["frame"]["nonce"].as_str().unwrap()).unwrap();
        let signature = decode(frames[2]["frame"]["signature"].as_str().unwrap()).unwrap();
        let owner = frames[1]["frame"]["service_id"].as_str().unwrap();
        verify(&public, &auth_bytes(&nonce, &public, owner), &signature).unwrap();
        assert!(
            verify(
                &public,
                &auth_bytes(&nonce, &public, "other-owner"),
                &signature
            )
            .is_err()
        );
    }
    #[test]
    fn aead_rejects_modified_ciphertext_wrong_key_and_changed_context() {
        use base64::Engine;
        let key = random().unwrap();
        let nonce = random().unwrap();
        let public = random().unwrap();
        let encrypted = encrypt_rotation(&key, nonce, b"context", &public).unwrap();
        assert_eq!(
            decrypt_rotation(&key, nonce, b"context", &encrypted).unwrap(),
            public
        );
        assert!(decrypt_rotation(&random().unwrap(), nonce, b"context", &encrypted).is_err());
        assert!(decrypt_rotation(&key, nonce, b"changed", &encrypted).is_err());
        let mut bytes = STANDARD.decode(&encrypted).unwrap();
        bytes[0] ^= 1;
        assert!(decrypt_rotation(&key, nonce, b"context", &encode(&bytes)).is_err());
    }
    #[test]
    fn rejects_noncanonical_base64_padding_alias() {
        let canonical = encode(&[0u8; 32]);
        let mut alias = canonical.clone().into_bytes();
        alias[42] = b'B';
        assert_ne!(String::from_utf8(alias.clone()).unwrap(), canonical);
        assert!(decode::<32>(&String::from_utf8(alias).unwrap()).is_err());
    }
}
