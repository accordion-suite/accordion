//! The activity logs in **Settings** (decision 18): each person's own, at
//! `/api/account/activity`, and the admin log and how long events are kept,
//! at `/api/admin/log`.
//!
//! Core keeps the event log. This fills in names as they are now: people's
//! display names, and items only for someone who can still open them.

use std::collections::HashMap;

use accordion_core::events::{Action, Event, Events, NewEvent};
use accordion_core::identity::SignedIn;
use accordion_module::auth::actor;
use accordion_module::{ApiError, Authenticated};
use accordion_types::{ItemId, UserId};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;

/// Events on one page of a log.
const PAGE: usize = 50;

/// Routes anyone signed in can use, from anywhere.
pub fn account_routes() -> Router<AppState> {
    Router::new().route("/api/account/activity", get(account_log))
}

/// Routes for admins, on the home network.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/api/admin/log", get(admin_log))
        .route("/api/admin/log/settings", get(settings).put(set_settings))
}

/// Records an admin's change from the server, after it's made. A failure
/// only loses the event, so it's logged rather than undoing the change.
pub(crate) async fn record(state: &AppState, me: &SignedIn, event: NewEvent) {
    let Some(events) = &state.events else {
        return;
    };
    if let Err(err) = events.record(actor(me), event).await {
        tracing::warn!(%err, "couldn't record an admin action in the event log");
    }
}

fn events(state: &AppState) -> Result<&Events, ApiError> {
    state.events.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "The activity log isn't available on this server.",
        )
    })
}

#[derive(Deserialize)]
struct PageQuery {
    /// The lowest event ID on the previous page.
    before: Option<i64>,
}

#[derive(Serialize)]
struct LogPage {
    events: Vec<EventView>,
    more: bool,
}

#[derive(Serialize)]
struct EventView {
    id: i64,
    at: String,
    action: &'static str,
    /// `None` for Accordion itself, or someone who wasn't signed in.
    actor: Option<PersonView>,
    via: &'static str,
    /// The item, if the reader can still open it.
    item: Option<ItemView>,
    /// Whether the event is about an item, even one the reader can't open.
    about_item: bool,
    other: Option<PersonView>,
    details: Value,
}

#[derive(Clone, Serialize)]
struct PersonView {
    id: UserId,
    /// `None` for an account that was deleted.
    name: Option<String>,
}

#[derive(Clone, Serialize)]
struct ItemView {
    id: ItemId,
    name: String,
}

async fn views(state: &AppState, reader: UserId, events: Vec<Event>) -> Result<LogPage, ApiError> {
    let more = events.len() == PAGE;
    let mut people: HashMap<UserId, Option<String>> = HashMap::new();
    let mut person = async |id: Option<UserId>| -> Result<Option<PersonView>, ApiError> {
        let Some(id) = id else {
            return Ok(None);
        };
        let name = match people.get(&id) {
            Some(name) => name.clone(),
            None => {
                let name = state.identity.user(id).await?.map(|user| user.display_name);
                people.insert(id, name.clone());
                name
            }
        };
        Ok(Some(PersonView { id, name }))
    };
    let mut views = Vec::with_capacity(events.len());
    for event in events {
        let item = match (&state.storage, event.item) {
            (Some(storage), Some(id)) if storage.access(reader, id).await?.is_some() => {
                storage.item(id).await?.map(|item| ItemView {
                    id,
                    name: item.name,
                })
            }
            _ => None,
        };
        views.push(EventView {
            id: event.id,
            at: event.at,
            action: event.action.as_str(),
            actor: person(event.actor).await?,
            via: event.via.as_str(),
            item,
            about_item: event.item.is_some(),
            other: person(event.other).await?,
            details: event.details,
        });
    }
    Ok(LogPage {
        events: views,
        more,
    })
}

/// `GET /api/account/activity`: the signed-in person's own activity log,
/// newest first: sign-ins, failed sign-ins, changes to their password and
/// app passwords, and sharing to and from them.
async fn account_log(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Query(query): Query<PageQuery>,
) -> Result<Json<LogPage>, ApiError> {
    let events = events(&state)?
        .account_log(me.user.id, query.before, PAGE)
        .await?;
    Ok(Json(views(&state, me.user.id, events).await?))
}

fn check_admin(me: &SignedIn) -> Result<(), ApiError> {
    if me.user.role.is_admin() {
        return Ok(());
    }
    Err(ApiError::new(
        StatusCode::FORBIDDEN,
        "Only admins can see the admin log.",
    ))
}

/// `GET /api/admin/log`: admin actions and failed sign-ins for every
/// account, newest first. Nobody's file activity is in it.
async fn admin_log(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Query(query): Query<PageQuery>,
) -> Result<Json<LogPage>, ApiError> {
    check_admin(&me)?;
    let events = events(&state)?.admin_log(query.before, PAGE).await?;
    Ok(Json(views(&state, me.user.id, events).await?))
}

#[derive(Serialize, Deserialize)]
struct LogSettings {
    /// How many days events are kept: 90, 365, or 730, or `null` for
    /// forever.
    keep_days: Option<u32>,
}

/// `GET /api/admin/log/settings`: how long events are kept.
async fn settings(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<LogSettings>, ApiError> {
    check_admin(&me)?;
    let keep_days = events(&state)?.keep_days().await?;
    Ok(Json(LogSettings { keep_days }))
}

/// `PUT /api/admin/log/settings`: changes how long events are kept. Older
/// events go in the next nightly upkeep.
async fn set_settings(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<LogSettings>,
) -> Result<Json<LogSettings>, ApiError> {
    check_admin(&me)?;
    events(&state)?
        .set_keep_days(actor(&me), request.keep_days)
        .await?;
    tracing::info!(user = %me.user.id, keep_days = ?request.keep_days, "changed how long activity is kept");
    Ok(Json(request))
}

/// An admin's change to backups, such as `"target_added"`.
pub(crate) fn backup_changed(change: &str) -> NewEvent {
    NewEvent::new(Action::BackupChanged).details(serde_json::json!({ "change": change }))
}
