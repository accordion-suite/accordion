//! The API behind the web app's Account and Apps and devices settings:
//! app passwords, so standard clients such as File Explorer, Finder, and
//! phone apps can reach Drive (decision 3 in `docs/decisions`), changing a
//! password, and changing a name.
//!
//! Every user can use it from anywhere, unlike the admin APIs.

use accordion_core::identity::{self, AppPassword, AppPasswordUse};
use accordion_module::{ApiError, Authenticated};
use accordion_types::AppPasswordId;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::network::Connection;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/app-passwords",
            get(app_passwords).post(create_app_password),
        )
        .route("/api/app-passwords/{id}", delete(delete_app_password))
        .route("/api/account/password", post(change_password))
        .route("/api/account/name", put(rename))
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum UseFor {
    /// WebDAV, CalDAV, and CardDAV: files, calendars, and contacts.
    Dav,
    Mail,
}

impl From<UseFor> for AppPasswordUse {
    fn from(use_for: UseFor) -> Self {
        match use_for {
            UseFor::Dav => Self::Dav,
            UseFor::Mail => Self::Mail,
        }
    }
}

#[derive(Serialize)]
struct AppPasswordView {
    id: AppPasswordId,
    device_name: String,
    use_for: UseFor,
    created_at: String,
    last_used_at: Option<String>,
}

impl From<AppPassword> for AppPasswordView {
    fn from(password: AppPassword) -> Self {
        Self {
            id: password.id,
            device_name: password.device_name,
            use_for: match password.use_for {
                AppPasswordUse::Dav => UseFor::Dav,
                AppPasswordUse::Mail => UseFor::Mail,
            },
            created_at: password.created_at,
            last_used_at: password.last_used_at,
        }
    }
}

/// `GET /api/app-passwords`: the signed-in user's app passwords, newest
/// first.
async fn app_passwords(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<Vec<AppPasswordView>>, ApiError> {
    let passwords = state.identity.app_passwords(me.user.id).await?;
    Ok(Json(passwords.into_iter().map(Into::into).collect()))
}

#[derive(Deserialize)]
struct NewAppPassword {
    device_name: String,
    use_for: UseFor,
}

#[derive(Serialize)]
struct CreatedAppPassword {
    app_password: AppPasswordView,
    /// Shown once, in groups of four letters.
    password: String,
}

/// `POST /api/app-passwords`: creates an app password for one device. It's
/// a sensitive action, so it needs a sign-in from the last 15 minutes.
/// Otherwise the answer is `403 Forbidden`, and the user confirms their
/// password with `POST /api/session/confirm` first.
async fn create_app_password(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<NewAppPassword>,
) -> Result<(StatusCode, Json<CreatedAppPassword>), ApiError> {
    if !me.recently_authenticated {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "For your security, enter your password again before you create an app password.",
        ));
    }
    let (secret, created) = state
        .identity
        .create_app_password(me.user.id, request.use_for.into(), &request.device_name)
        .await?;
    tracing::info!(
        user = %me.user.id,
        app_password = %created.id,
        "created an app password"
    );
    Ok((
        StatusCode::CREATED,
        Json(CreatedAppPassword {
            app_password: created.into(),
            password: secret.grouped(),
        }),
    ))
}

/// `DELETE /api/app-passwords/{id}`: deletes one of the signed-in user's app
/// passwords, so its device can't sign in with it again.
async fn delete_app_password(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<AppPasswordId>,
) -> Result<StatusCode, ApiError> {
    if !state.identity.delete_app_password(me.user.id, id).await? {
        return Err(ApiError::not_found());
    }
    tracing::info!(user = %me.user.id, app_password = %id, "deleted an app password");
    Ok(StatusCode::NO_CONTENT)
}

/// Checks the signed-in user's password, for changes that need it.
async fn check_current_password(
    state: &AppState,
    me: &identity::SignedIn,
    password: &str,
    connection: &Connection,
) -> Result<(), ApiError> {
    match state
        .identity
        .check_password(&me.user.username, password, connection.sign_in_from())
        .await
    {
        Ok(_) => Ok(()),
        Err(identity::Error::WrongCredentials) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "Your current password isn't right. Check it, then try again.",
        )),
        Err(err) => Err(err.into()),
    }
}

#[derive(Deserialize)]
struct NewPassword {
    current_password: String,
    new_password: String,
}

/// `POST /api/account/password`: changes the signed-in user's password, and
/// signs out their other browsers and apps, in case someone else knew the
/// old one. App passwords keep working.
async fn change_password(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    connection: Connection,
    Json(request): Json<NewPassword>,
) -> Result<StatusCode, ApiError> {
    check_current_password(&state, &me, &request.current_password, &connection).await?;
    state
        .identity
        .set_password(me.user.id, request.new_password, Some(me.session.id))
        .await?;
    tracing::info!(user = %me.user.id, "changed their password");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct NewName {
    username: String,
    display_name: String,
    /// Needed only for a new username, which is what everyone signs in with.
    #[serde(default)]
    current_password: String,
}

/// `PUT /api/account/name`: changes the signed-in user's name, and their
/// username if they give their password.
async fn rename(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    connection: Connection,
    Json(request): Json<NewName>,
) -> Result<StatusCode, ApiError> {
    let new_username = request.username.trim().to_lowercase() != me.user.username;
    if new_username {
        check_current_password(&state, &me, &request.current_password, &connection).await?;
    }
    let user = state
        .identity
        .rename_member(me.user.id, &request.username, &request.display_name)
        .await?;
    if user.username != me.user.username {
        crate::people::follow_new_username(&state).await;
    }
    tracing::info!(user = %me.user.id, "changed their name");
    Ok(StatusCode::NO_CONTENT)
}
