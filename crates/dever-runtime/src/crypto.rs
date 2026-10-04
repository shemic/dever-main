use argon2::password_hash::{Error, SaltString};
use argon2::{Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version};
use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

use crate::{bytes::Bytes, secret::Secret};

const MAX_TOKEN_BYTES: i64 = 1024;
const INVALID_HASH: &str = "invalid or unsupported password hash";

pub fn token(length: i64) -> Result<Secret, String> {
    if !(16..=MAX_TOKEN_BYTES).contains(&length) {
        return Err("token length must be between 16 and 1024 bytes".into());
    }
    let mut bytes = Zeroizing::new(vec![0; length as usize]);
    getrandom::getrandom(&mut bytes).map_err(|_| "secure random source unavailable")?;
    Ok(Secret(bytes))
}

pub fn password_hash(password: &Secret) -> Result<String, String> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).map_err(|_| "secure random source unavailable")?;
    let salt = SaltString::encode_b64(&salt).map_err(|_| "password hashing failed")?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, Params::default())
        .hash_password(&password.0, &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| "password hashing failed".into())
}

pub fn password_verify(password: &Secret, encoded: &str) -> Result<bool, String> {
    // Bound parsing and resource costs before invoking the password verifier:
    // that verifier otherwise accepts PHC-controlled Argon2 parameters.
    if encoded.len() > 256 {
        return Err(INVALID_HASH.into());
    }
    let hash = PasswordHash::new(encoded).map_err(|_| INVALID_HASH)?;
    let params = Params::try_from(&hash).map_err(|_| INVALID_HASH)?;
    let limits = Params::default();
    if hash.algorithm.as_str() != "argon2id"
        || hash.version != Some(19)
        || hash.salt.is_none()
        || hash.hash.is_none()
        || params.m_cost() > limits.m_cost()
        || params.t_cost() > limits.t_cost()
        || params.p_cost() > limits.p_cost()
        || params.output_len().unwrap_or(Params::DEFAULT_OUTPUT_LEN) > Params::DEFAULT_OUTPUT_LEN
        || !params.keyid().is_empty()
        || !params.data().is_empty()
    {
        return Err(INVALID_HASH.into());
    }
    match Argon2::default().verify_password(&password.0, &hash) {
        Ok(()) => Ok(true),
        Err(Error::Password) => Ok(false),
        Err(_) => Err(INVALID_HASH.into()),
    }
}

pub fn sha256(bytes: &Bytes) -> Bytes {
    Bytes::new(Sha256::digest(bytes.values()).to_vec())
}

pub fn hmac_sha256(key: &Secret, bytes: &Bytes) -> Bytes {
    let mut mac = Hmac::<Sha256>::new_from_slice(&key.0).expect("HMAC accepts any key length");
    mac.update(bytes.values());
    Bytes::new(mac.finalize().into_bytes().to_vec())
}

/// Content comparison is constant-time for equal lengths; lengths are public.
pub fn constant_time_eq(left: &Bytes, right: &Bytes) -> bool {
    left.values().ct_eq(right.values()).into()
}
