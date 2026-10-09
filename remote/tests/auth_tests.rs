use ody_remote::gateway::auth::*;
use std::time::Duration;

#[test]
fn password_hash_roundtrip_accepts_correct_password() {
    let hash = hash_password("correct horse battery staple");
    assert!(verify_password(&hash, "correct horse battery staple"));
}

#[test]
fn password_verify_rejects_wrong_password() {
    let hash = hash_password("right");
    assert!(!verify_password(&hash, "wrong"));
}

#[test]
fn password_verify_handles_garbage_hash_without_panicking() {
    assert!(!verify_password("not-an-argon2-hash", "whatever"));
    assert!(!verify_password("", ""));
}

#[test]
fn issued_session_validates_until_revoked() {
    let sessions = Sessions::new(Duration::from_secs(3600));
    let token = sessions.issue();
    assert!(sessions.validate(&token));
    sessions.revoke(&token);
    assert!(!sessions.validate(&token));
}

#[test]
fn unknown_session_token_is_invalid() {
    let sessions = Sessions::new(Duration::from_secs(3600));
    assert!(!sessions.validate("no-such-token"));
}

#[test]
fn expired_session_is_invalid() {
    let sessions = Sessions::new(Duration::from_millis(1));
    let token = sessions.issue();
    std::thread::sleep(Duration::from_millis(10));
    assert!(!sessions.validate(&token));
}

#[test]
fn session_tokens_are_unique() {
    let sessions = Sessions::new(Duration::from_secs(60));
    assert_ne!(sessions.issue(), sessions.issue());
}

#[test]
fn login_limiter_locks_after_five_failures() {
    let mut limiter = LoginLimiter::new();
    for _ in 0..5 {
        assert!(limiter.check("alice"));
        limiter.record_failure("alice");
    }
    assert!(!limiter.check("alice"));
}

#[test]
fn login_limiter_success_resets_failures() {
    let mut limiter = LoginLimiter::new();
    for _ in 0..4 {
        limiter.record_failure("alice");
    }
    limiter.record_success("alice");
    for _ in 0..4 {
        assert!(limiter.check("alice"));
        limiter.record_failure("alice");
    }
    assert!(limiter.check("alice"));
}

#[test]
fn login_limiter_is_per_user() {
    let mut limiter = LoginLimiter::new();
    for _ in 0..5 {
        limiter.record_failure("alice");
    }
    assert!(limiter.check("bob"));
}

#[test]
fn session_cookie_roundtrip_via_header_parse() {
    let cookie = session_set_cookie("tok123", Duration::from_secs(3600));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("Secure"));
    assert!(cookie.contains("SameSite=Strict"));
    assert!(cookie.contains("Max-Age=3600"));
    let header = format!("other=1; {SESSION_COOKIE}=tok123; another=2");
    assert_eq!(extract_session_cookie(&header).as_deref(), Some("tok123"));
}

#[test]
fn extract_session_cookie_missing_returns_none() {
    assert_eq!(extract_session_cookie("a=1; b=2"), None);
}

#[test]
fn clear_cookie_expires_immediately() {
    let cookie = clear_session_cookie();
    assert!(cookie.contains("Max-Age=0"));
}
