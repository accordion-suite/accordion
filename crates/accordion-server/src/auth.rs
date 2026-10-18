//! Finds the signed-in user for a request (decision 3 in `docs/decisions`).
//!
//! The web app sends its session token in a cookie. The desktop and mobile
//! apps send it as a bearer token.

use accordion_core::identity::{SessionToken, SignedIn};
use axum::extract::FromRequestParts;
use axum::http::header::{AUTHORIZATION, COOKIE};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue};

use crate::AppState;
use crate::error::ApiError;

/// The `__Host-` prefix makes browsers refuse the cookie unless it's
/// `Secure`, has `Path=/`, and has no `Domain`, so no other site or
/// subdomain can set or overwrite it.
pub const COOKIE_NAME: &str = "__Host-accordion-session";

/// The server ends idle sessions itself. The cookie only has to outlast them,
/// and 400 days is the longest that browsers keep a cookie.
const COOKIE_MAX_AGE_SECS: u32 = 400 * 24 * 60 * 60;

/// A request from a signed-in user. Add it as a handler argument to require
/// a sign-in. Requests without a live session get a 401 response.
pub struct Authenticated(pub SignedIn);

impl FromRequestParts<AppState> for Authenticated {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let token = token_from(&parts.headers).ok_or_else(ApiError::not_signed_in)?;
        let signed_in = state
            .identity
            .check_session(token)
            .await?
            .ok_or_else(ApiError::not_signed_in)?;
        Ok(Self(signed_in))
    }
}

fn token_from(headers: &HeaderMap) -> Option<&str> {
    if let Some(value) = headers.get(AUTHORIZATION) {
        return value.to_str().ok()?.strip_prefix("Bearer ");
    }
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| {
            let (name, value) = pair.trim().split_once('=')?;
            (name == COOKIE_NAME).then_some(value)
        })
}

pub fn session_cookie(token: &SessionToken) -> HeaderValue {
    HeaderValue::try_from(format!(
        "{COOKIE_NAME}={}; Path=/; Max-Age={COOKIE_MAX_AGE_SECS}; Secure; HttpOnly; SameSite=Lax",
        token.as_str()
    ))
    .expect("session tokens are always valid header text")
}

pub fn removal_cookie() -> HeaderValue {
    HeaderValue::from_static(
        "__Host-accordion-session=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Lax",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for &(name, value) in pairs {
            map.append(name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn finds_the_token_in_a_cookie_or_bearer_header() {
        let cookie = headers(&[(
            "cookie",
            "theme=dark; __Host-accordion-session=acs_abc; x=1",
        )]);
        assert_eq!(token_from(&cookie), Some("acs_abc"));

        let bearer = headers(&[("authorization", "Bearer acs_xyz")]);
        assert_eq!(token_from(&bearer), Some("acs_xyz"));

        let neither = headers(&[("cookie", "accordion-session=acs_abc")]);
        assert_eq!(token_from(&neither), None);

        let basic = headers(&[("authorization", "Basic YWxleDpwYXNz")]);
        assert_eq!(token_from(&basic), None);
    }

    #[test]
    fn the_removal_cookie_matches_the_session_cookie() {
        let removal = removal_cookie();
        assert!(
            removal
                .to_str()
                .unwrap()
                .starts_with(&format!("{COOKIE_NAME}=;"))
        );
    }
}
