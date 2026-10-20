//! `/api/session`: sign in, check who's signed in, and sign out.

use accordion_core::identity::{AuthMethod, Client, Session, SignedIn, User};
use accordion_types::{Role, SessionId, UserId};
use axum::Json;
use axum::extract::State;
use axum::http::header::{SET_COOKIE, USER_AGENT};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::auth::{self, Authenticated};
use crate::device;
use crate::error::ApiError;
use crate::network::Connection;

#[derive(Deserialize)]
pub struct SignInRequest {
    username: String,
    password: String,
}

#[derive(Serialize)]
pub struct SessionResponse {
    user: UserView,
    session: SessionView,
}

#[derive(Serialize)]
struct UserView {
    id: UserId,
    username: String,
    display_name: String,
    role: Role,
}

#[derive(Serialize)]
struct SessionView {
    id: SessionId,
    device_name: String,
    created_at: String,
    last_used_at: String,
}

impl SessionResponse {
    pub(crate) fn new(user: User, session: Session) -> Self {
        Self {
            user: UserView {
                id: user.id,
                username: user.username,
                display_name: user.display_name,
                role: user.role,
            },
            session: SessionView {
                id: session.id,
                device_name: session.device_name,
                created_at: session.created_at,
                last_used_at: session.last_used_at,
            },
        }
    }
}

/// `POST /api/session`: signs in with a username and password, and sets the
/// session cookie.
pub async fn sign_in(
    State(state): State<AppState>,
    connection: Connection,
    headers: HeaderMap,
    Json(request): Json<SignInRequest>,
) -> Result<Response, ApiError> {
    let user = state
        .identity
        .check_password(
            &request.username,
            &request.password,
            connection.sign_in_from(),
        )
        .await?;
    let user_agent = headers.get(USER_AGENT).and_then(|ua| ua.to_str().ok());
    let (token, session) = state
        .identity
        .create_session(
            user.id,
            Client::Browser,
            &device::describe_browser(user_agent),
            AuthMethod::Password,
        )
        .await?;
    tracing::info!(user = %user.id, session = %session.id, "signed in");
    Ok((
        [(SET_COOKIE, auth::session_cookie(&token))],
        Json(SessionResponse::new(user, session)),
    )
        .into_response())
}

/// `GET /api/session`: returns the signed-in user and their session.
pub async fn current(
    Authenticated(SignedIn { user, session, .. }): Authenticated,
) -> Json<SessionResponse> {
    Json(SessionResponse::new(user, session))
}

/// `DELETE /api/session`: signs out this browser or app.
pub async fn sign_out(
    State(state): State<AppState>,
    Authenticated(SignedIn { user, session, .. }): Authenticated,
) -> Result<Response, ApiError> {
    state.identity.end_session(user.id, session.id).await?;
    tracing::info!(user = %user.id, session = %session.id, "signed out");
    Ok((
        StatusCode::NO_CONTENT,
        [(SET_COOKIE, auth::removal_cookie())],
    )
        .into_response())
}
