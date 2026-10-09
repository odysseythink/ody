//! Gateway authentication: argon2 password verification, in-memory session
//! tokens, and a minimal login rate limiter.

use argon2::Argon2;
use argon2::PasswordHash;
use argon2::PasswordHasher;
use argon2::PasswordVerifier;
use argon2::password_hash::SaltString;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

pub const SESSION_COOKIE: &str = "ody_remote_session";

const MAX_LOGIN_FAILURES: u32 = 5;

/// Hash a password for storage in the gateway config file.
pub fn hash_password(password: &str) -> String {
    let mut salt_bytes = [0u8; 16];
    rand::fill(&mut salt_bytes);
    let salt = match SaltString::encode_b64(&salt_bytes) {
        Ok(salt) => salt,
        // A 16-byte salt always fits; this is unreachable.
        Err(err) => panic!("argon2 salt encoding failed: {err}"),
    };
    match Argon2::default().hash_password(password.as_bytes(), &salt) {
        Ok(hash) => hash.to_string(),
        // Unreachable with default params and a fresh valid salt.
        Err(err) => panic!("argon2 hashing with default params failed: {err}"),
    }
}

/// Verify a password against an argon2 encoded hash. Never panics.
pub fn verify_password(encoded_hash: &str, password: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(encoded_hash) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Structural check that a string is a parseable argon2 hash (config-time
/// validation; does not verify any password).
pub fn verify_password_probe(encoded_hash: &str) -> bool {
    PasswordHash::new(encoded_hash).is_ok()
}

/// In-memory session registry. Tokens are 256-bit random, base64url encoded.
// ody: 重启 gateway 会清空全部会话（用户需重新登录）；升级触发条件=会话需要跨重启保留时改为持久化存储。
pub struct Sessions {
    ttl: Duration,
    inner: Mutex<HashMap<String, Instant>>,
}

impl Sessions {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn issue(&self) -> String {
        let mut bytes = [0u8; 32];
        rand::fill(&mut bytes);
        let token = URL_SAFE_NO_PAD.encode(bytes);
        crate::lock(&self.inner).insert(token.clone(), Instant::now() + self.ttl);
        token
    }

    pub fn validate(&self, token: &str) -> bool {
        let mut inner = crate::lock(&self.inner);
        match inner.get(token) {
            Some(expiry) if *expiry > Instant::now() => true,
            Some(_) => {
                inner.remove(token);
                false
            }
            None => false,
        }
    }

    pub fn revoke(&self, token: &str) {
        crate::lock(&self.inner).remove(token);
    }
}

/// Per-username login failure limiter: locks after [`MAX_LOGIN_FAILURES`]
/// consecutive failures; a success resets the counter.
// ody: 不按来源 IP 限流，锁定期间会同时挡住 legitimate 用户本人；升级触发条件=暴露到公网后出现撞库迹象时改为按 IP+账号二维限流。
#[derive(Default)]
pub struct LoginLimiter {
    failures: HashMap<String, u32>,
}

impl LoginLimiter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn check(&self, username: &str) -> bool {
        self.failures.get(username).copied().unwrap_or(0) < MAX_LOGIN_FAILURES
    }

    pub fn record_failure(&mut self, username: &str) {
        *self.failures.entry(username.to_string()).or_insert(0) += 1;
    }

    pub fn record_success(&mut self, username: &str) {
        self.failures.remove(username);
    }
}

/// Build the `Set-Cookie` value for a newly issued session.
pub fn session_set_cookie(token: &str, max_age: Duration) -> String {
    format!(
        "{SESSION_COOKIE}={token}; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age={}",
        max_age.as_secs()
    )
}

/// Build the `Set-Cookie` value that clears the session cookie.
pub fn clear_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; Secure; SameSite=Strict; Path=/; Max-Age=0")
}

/// Extract the session token from a raw `Cookie` header value.
pub fn extract_session_cookie(cookie_header: &str) -> Option<String> {
    cookie_header.split(';').map(str::trim).find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == SESSION_COOKIE).then(|| value.to_string())
    })
}
