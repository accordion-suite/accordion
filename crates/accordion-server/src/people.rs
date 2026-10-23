//! `/people`: where admins add the people in the household, make password
//! links, change roles, and turn accounts off.
//!
//! Admins never choose or see anyone's password. A new account gets a
//! one-time link, which the admin passes on, and the person sets their own
//! password on `/welcome`. A forgotten password works the same way.
//!
//! Admins manage standard accounts. Only the owner manages admins, so an
//! admin can't lock the owner out or give themselves more power. Nobody
//! changes their own role or the owner's account here.
//!
//! Like the other admin pages, it opens only from the home network.

use accordion_core::identity::{
    AppPasswords, LinkPurpose, LinkToken, Member, PasswordLink, SignedIn, User,
};
use accordion_module::{ApiError, Authenticated};
use accordion_types::{Role, UserId};
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;
use crate::setup::asset;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/people",
            get(|| async { asset(PEOPLE_HTML, "text/html; charset=utf-8") }),
        )
        .route(
            "/people.js",
            get(|| async { asset(PEOPLE_JS, "text/javascript") }),
        )
        .route("/api/people", get(list).post(add))
        .route("/api/people/{id}/password-link", post(password_link))
        .route("/api/people/{id}/role", put(set_role))
        .route("/api/people/{id}/turn-off", post(turn_off))
        .route("/api/people/{id}/turn-on", post(turn_on))
}

const PEOPLE_HTML: &str = include_str!("people/people.html");
const PEOPLE_JS: &str = include_str!("people/people.js");

fn check_admin(me: &SignedIn) -> Result<(), ApiError> {
    if me.user.role.is_admin() {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "Only admins can manage people. Ask your admin to add someone.",
    ))
}

/// Changes to accounts need a sign-in from the last 15 minutes, like other
/// sensitive actions.
fn check_recent(me: &SignedIn) -> Result<(), ApiError> {
    if me.recently_authenticated {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "For your security, enter your password again before you change someone's account.",
    ))
}

/// Why `me` can't manage an account with `role`, or `None` if they can.
fn refusal(me: &User, target: &User) -> Option<&'static str> {
    if target.id == me.id {
        Some(
            "You can't change your own account here. To change your password, go to your \
             account page.",
        )
    } else if target.role == Role::Owner {
        Some("The owner's account can't be changed here.")
    } else if target.role == Role::Admin && me.role != Role::Owner {
        Some("Only the owner can change an admin's account.")
    } else {
        None
    }
}

/// The account with `id`, if `me` may manage it.
async fn manageable(state: &AppState, me: &SignedIn, id: UserId) -> Result<User, ApiError> {
    check_admin(me)?;
    check_recent(me)?;
    let target = state
        .identity
        .user(id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    match refusal(&me.user, &target) {
        Some(reason) => Err(ApiError::new(StatusCode::FORBIDDEN, reason)),
        None => Ok(target),
    }
}

fn check_role_for(me: &User, role: Role) -> Result<(), ApiError> {
    if role == Role::Admin && me.role != Role::Owner {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Only the owner can make someone an admin.",
        ));
    }
    Ok(())
}

#[derive(Serialize)]
struct PeopleView {
    people: Vec<PersonView>,
    /// Whether the signed-in admin may make admins, so the page offers it.
    can_add_admins: bool,
}

#[derive(Serialize)]
struct PersonView {
    id: UserId,
    username: String,
    display_name: String,
    role: Role,
    created_at: String,
    /// `active`, `waiting` (for the person to set a password), or
    /// `turned_off`.
    state: &'static str,
    password_link: Option<LinkView>,
    /// Whether the signed-in admin may change this account.
    can_manage: bool,
}

#[derive(Serialize)]
struct LinkView {
    purpose: &'static str,
    expires_at: String,
}

impl From<&PasswordLink> for LinkView {
    fn from(link: &PasswordLink) -> Self {
        Self {
            purpose: match link.purpose {
                LinkPurpose::NewAccount => "new_account",
                LinkPurpose::Reset => "reset",
            },
            expires_at: link.expires_at.clone(),
        }
    }
}

fn person(member: Member, me: &User) -> PersonView {
    let state = if member.turned_off {
        "turned_off"
    } else if member.has_password {
        "active"
    } else {
        "waiting"
    };
    PersonView {
        can_manage: refusal(me, &member.user).is_none(),
        id: member.user.id,
        username: member.user.username,
        display_name: member.user.display_name,
        role: member.user.role,
        created_at: member.created_at,
        state,
        password_link: member.password_link.as_ref().map(Into::into),
    }
}

/// `GET /api/people`: everyone with an account.
async fn list(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<PeopleView>, ApiError> {
    check_admin(&me)?;
    let members = state.identity.members().await?;
    Ok(Json(PeopleView {
        people: members
            .into_iter()
            .map(|member| person(member, &me.user))
            .collect(),
        can_add_admins: me.user.role == Role::Owner,
    }))
}

#[derive(Serialize)]
struct CreatedLink {
    /// The secret for the link's `#` part, which the page turns into the
    /// full address. Shown once.
    token: String,
    #[serde(flatten)]
    link: LinkView,
}

/// A response the browser mustn't keep, because it holds a password link.
fn secret(token: LinkToken, link: &PasswordLink, status: StatusCode) -> Response {
    let body = CreatedLink {
        token: token.as_str().to_owned(),
        link: link.into(),
    };
    let mut response = (status, Json(body)).into_response();
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Deserialize)]
struct NewPerson {
    username: String,
    display_name: String,
    role: Role,
}

/// `POST /api/people`: adds an account without a password, and returns the
/// link for the person to set one.
async fn add(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<NewPerson>,
) -> Result<Response, ApiError> {
    check_admin(&me)?;
    check_recent(&me)?;
    check_role_for(&me.user, request.role)?;
    let (user, token, link) = state
        .identity
        .add_member(&request.username, &request.display_name, request.role)
        .await?;
    tracing::info!(user = %me.user.id, added = %user.id, role = %user.role, "added an account");
    Ok(secret(token, &link, StatusCode::CREATED))
}

#[derive(Deserialize)]
struct LinkRequest {
    /// For when someone else might know the password: also deletes the
    /// account's app passwords, so every device has to connect again.
    #[serde(default)]
    delete_app_passwords: bool,
}

/// `POST /api/people/{id}/password-link`: makes a new link to set the
/// account's password. For an account that has one, it's a reset, which
/// signs the account out everywhere.
async fn password_link(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<UserId>,
    Json(request): Json<LinkRequest>,
) -> Result<Response, ApiError> {
    manageable(&state, &me, id).await?;
    let app_passwords = if request.delete_app_passwords {
        AppPasswords::Delete
    } else {
        AppPasswords::Keep
    };
    let (token, link) = state.identity.make_password_link(id, app_passwords).await?;
    match link.purpose {
        LinkPurpose::Reset => {
            tracing::info!(
                user = %me.user.id,
                target = %id,
                deleted_app_passwords = request.delete_app_passwords,
                "reset a password"
            );
        }
        LinkPurpose::NewAccount => {
            tracing::info!(user = %me.user.id, target = %id, "made a new password link");
        }
    }
    Ok(secret(token, &link, StatusCode::OK))
}

#[derive(Deserialize)]
struct NewRole {
    role: Role,
}

/// `PUT /api/people/{id}/role`: makes an account an admin or a standard
/// account.
async fn set_role(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<UserId>,
    Json(request): Json<NewRole>,
) -> Result<StatusCode, ApiError> {
    manageable(&state, &me, id).await?;
    check_role_for(&me.user, request.role)?;
    let user = state.identity.set_role(id, request.role).await?;
    tracing::info!(user = %me.user.id, target = %id, role = %user.role, "changed a role");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/people/{id}/turn-off`: stops an account from signing in. Its
/// files stay.
async fn turn_off(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<UserId>,
) -> Result<StatusCode, ApiError> {
    manageable(&state, &me, id).await?;
    state.identity.turn_off_member(id).await?;
    tracing::info!(user = %me.user.id, target = %id, "turned off an account");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/people/{id}/turn-on`: lets a turned-off account sign in again.
async fn turn_on(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<UserId>,
) -> Result<StatusCode, ApiError> {
    manageable(&state, &me, id).await?;
    state.identity.turn_on_member(id).await?;
    tracing::info!(user = %me.user.id, target = %id, "turned on an account");
    Ok(StatusCode::NO_CONTENT)
}
