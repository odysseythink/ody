use super::*;

#[test]
fn classifies_personal_access_tokens_by_prefix() {
    assert!(matches!(
        classify_ody_access_token("at-example"),
        OdyAccessToken::PersonalAccessToken("at-example")
    ));
    assert!(matches!(
        classify_ody_access_token("header.payload.signature"),
        OdyAccessToken::AgentIdentityJwt("header.payload.signature")
    ));
}
