const PERSONAL_ACCESS_TOKEN_PREFIX: &str = "at-";

pub(super) enum OdyAccessToken<'a> {
    PersonalAccessToken(&'a str),
    AgentIdentityJwt(&'a str),
}

pub(super) fn classify_ody_access_token(access_token: &str) -> OdyAccessToken<'_> {
    if access_token.starts_with(PERSONAL_ACCESS_TOKEN_PREFIX) {
        OdyAccessToken::PersonalAccessToken(access_token)
    } else {
        OdyAccessToken::AgentIdentityJwt(access_token)
    }
}

#[cfg(test)]
#[path = "access_token_tests.rs"]
mod tests;
