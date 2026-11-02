//! `/api/directory`: the names and usernames of everyone who uses the
//! server, so people can choose whom to share with (decision 16).
//!
//! Unlike People in Settings, anyone signed in can read it, from anywhere.
//! It leaves out accounts that are turned off or still waiting for their
//! password, and says nothing about roles.

use accordion_module::{ApiError, Authenticated};
use accordion_types::UserId;
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new().route("/api/directory", get(list))
}

#[derive(Serialize)]
struct Entry {
    id: UserId,
    username: String,
    display_name: String,
    /// Whether it's the signed-in person.
    you: bool,
}

/// `GET /api/directory`: everyone who can sign in, by name.
async fn list(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<Vec<Entry>>, ApiError> {
    let mut people: Vec<Entry> = state
        .identity
        .members()
        .await?
        .into_iter()
        .filter(|member| member.has_password && !member.turned_off)
        .map(|member| Entry {
            you: member.user.id == me.user.id,
            id: member.user.id,
            username: member.user.username,
            display_name: member.user.display_name,
        })
        .collect();
    // People look for each other by name. The admin list puts roles first.
    people.sort_by_cached_key(|entry| (entry.display_name.to_lowercase(), entry.username.clone()));
    Ok(Json(people))
}
