//! The API behind People in the web app's Settings, where admins add the
//! people in the household, make password links, change roles and names,
//! turn accounts off, and delete them.
//!
//! Admins never choose or see anyone's password. A new account gets a
//! one-time link, which the admin passes on, and the person sets their own
//! password on `/welcome`. A forgotten password works the same way.
//!
//! Admins manage standard accounts. Only the owner manages admins, so an
//! admin can't lock the owner out or give themselves more power. Nobody
//! changes their own role or the owner's account here.
//!
//! Like the other admin APIs, it answers only on the home network.

use accordion_core::identity::{
    AppPasswords, LinkPurpose, LinkToken, Member, PasswordLink, SignedIn, User,
};
use accordion_core::storage::LeaveFiles;
use accordion_module::{ApiError, Authenticated};
use accordion_types::{Role, UserId};
use axum::extract::{Path, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/people", get(list).post(add))
        .route("/api/people/{id}", delete(delete_person))
        .route("/api/people/{id}/name", put(rename))
        .route("/api/people/{id}/password-link", post(password_link))
        .route("/api/people/{id}/role", put(set_role))
        .route("/api/people/{id}/turn-off", post(turn_off))
        .route("/api/people/{id}/turn-on", post(turn_on))
}

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
    Err(ApiError::password_needed(
        "For your security, enter your password again before you change someone's account.",
    ))
}

/// Why `me` can't manage an account with `role`, or `None` if they can.
fn refusal(me: &User, target: &User) -> Option<&'static str> {
    if target.id == me.id {
        Some(
            "You can't change your own account here. To change your name or password, go to \
             Account in Settings.",
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
    /// The signed-in admin, whom the page suggests for a deleted account's
    /// files.
    you: UserId,
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
        you: me.user.id,
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

#[derive(Deserialize)]
struct NewName {
    username: String,
    display_name: String,
}

/// `PUT /api/people/{id}/name`: changes someone's name and username.
async fn rename(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<UserId>,
    Json(request): Json<NewName>,
) -> Result<StatusCode, ApiError> {
    let before = manageable(&state, &me, id).await?;
    let user = state
        .identity
        .rename_member(id, &request.username, &request.display_name)
        .await?;
    if user.username != before.username {
        follow_new_username(&state).await;
    }
    tracing::info!(user = %me.user.id, target = %id, "renamed an account");
    Ok(StatusCode::NO_CONTENT)
}

/// Storage's plain view names each person's folder after their username, so
/// it's rebuilt after a new one. A failure only leaves the view behind until
/// the next rebuild, so it's logged rather than shown.
pub(crate) async fn follow_new_username(state: &AppState) {
    if let Some(storage) = &state.storage
        && let Err(err) = storage.rebuild_view().await
    {
        tracing::error!(%err, "couldn't rebuild the plain view after a new username");
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Files {
    /// Move them into a folder in this person's Drive.
    GiveTo(UserId),
    Delete,
}

#[derive(Deserialize)]
struct Deletion {
    /// The account's username, typed by the admin to show they mean it.
    username: String,
    /// Required, so a request that leaves it out never deletes files.
    files: Files,
}

#[derive(Serialize)]
struct Deleted {
    /// The folder the files went into, if they were given to someone and
    /// there were any.
    folder: Option<String>,
}

/// `DELETE /api/people/{id}`: deletes an account, after giving its files to
/// someone else or deleting them.
async fn delete_person(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<UserId>,
    Json(request): Json<Deletion>,
) -> Result<Json<Deleted>, ApiError> {
    let target = manageable(&state, &me, id).await?;
    if request.username.trim().to_lowercase() != target.username {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            format!(
                "To confirm, type {}'s username exactly: {}",
                target.display_name, target.username
            ),
        ));
    }
    let storage = state
        .storage
        .as_ref()
        .ok_or_else(ApiError::server_problem)?;
    let (files, given_to) = match request.files {
        Files::GiveTo(to) => (
            LeaveFiles::GiveTo {
                owner: to,
                // Display names can hold any character, but a folder name
                // can't hold a slash.
                folder_name: format!("{}'s files", target.display_name.replace('/', "-")),
            },
            Some(to),
        ),
        Files::Delete => (LeaveFiles::Delete, None),
    };
    // Off first: they're signed out everywhere, and nothing new arrives
    // while their files move. If a step fails, the account stays off, and
    // trying again picks up where it stopped.
    state.identity.turn_off_member(id).await?;
    let folder = storage.remove_owner(id, files).await?;
    state.identity.delete_member(id).await?;
    match given_to {
        Some(to) => tracing::info!(
            user = %me.user.id,
            target = %id,
            files_to = %to,
            "deleted an account and gave its files away"
        ),
        None => tracing::info!(
            user = %me.user.id,
            target = %id,
            "deleted an account and its files"
        ),
    }
    Ok(Json(Deleted {
        folder: folder.map(|folder| folder.name),
    }))
}
