//! The API behind the web app's **Import from Google** page (decision 34 in
//! `docs/decisions`): the archives a person put in Drive, what they hold,
//! and imports to start, follow, and cancel. Core runs the imports, and
//! each module stores its own products.
//!
//! Every user can use it from anywhere, for their own archives.

use accordion_core::storage::{Item, ItemKind};
use accordion_core::takeout::imports::{self, Import};
use accordion_core::takeout::{self, Imports, Take};
use accordion_module::{ApiError, Authenticated};
use accordion_types::{ItemId, UserId};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/takeout/archives", get(archives))
        .route("/api/takeout/folder", post(folder))
        .route("/api/takeout/plan", post(plan))
        .route("/api/takeout/imports", get(list).post(start))
        .route("/api/takeout/imports/{id}/cancel", post(cancel))
}

/// How many of a person's imports the page lists.
const LISTED: usize = 20;

fn imports(state: &AppState) -> Result<&Imports, ApiError> {
    state.takeout.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Importing isn't available until the server finishes starting. Try again in a \
             minute.",
        )
    })
}

fn storage(state: &AppState) -> Result<&accordion_core::storage::Storage, ApiError> {
    state.storage.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "Drive isn't available until the server finishes starting. Try again in a minute.",
        )
    })
}

/// An import's errors, as the page shows them.
fn api_error(err: imports::Error) -> ApiError {
    use imports::Error;
    match err {
        Error::NoParts
        | Error::TooManyParts
        | Error::PartNotFound
        | Error::PartNotAFile
        | Error::NothingChosen
        | Error::UnknownProduct(_)
        | Error::Read(takeout::Error::Unreadable(_)) => {
            ApiError::new(StatusCode::BAD_REQUEST, err.to_string())
        }
        Error::Busy | Error::NotRunning => ApiError::new(StatusCode::CONFLICT, err.to_string()),
        Error::Read(takeout::Error::Failed(why)) => {
            tracing::warn!("couldn't read Takeout archives: {why}");
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Accordion couldn't read the archives. Check that they're the export's .zip or \
                 .tgz files, then try again.",
            )
        }
        Error::Storage(err) => err.into(),
        Error::Database(err) => {
            tracing::error!("import: {err}");
            ApiError::server_problem()
        }
    }
}

/// An archive in Drive that could be part of an export.
#[derive(Serialize)]
struct ArchiveView {
    id: ItemId,
    name: String,
    size: u64,
    modified_at: String,
    /// Where it is: `takeout` for **Google Takeout**, `top` for the top of
    /// My Drive.
    place: &'static str,
}

#[derive(Serialize)]
struct ArchivesView {
    archives: Vec<ArchiveView>,
    /// **Google Takeout** in My Drive, if it's there, where the page
    /// uploads archives.
    folder: Option<ItemId>,
}

/// Whether `name` looks like one of Takeout's archives.
fn is_archive(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.ends_with(".zip") || name.ends_with(".tgz") || name.ends_with(".tar.gz")
}

fn archive(item: Item, place: &'static str) -> Option<ArchiveView> {
    let ItemKind::File { blob, .. } = &item.kind else {
        return None;
    };
    is_archive(&item.name).then_some(ArchiveView {
        id: item.id,
        size: blob.size,
        name: item.name,
        modified_at: item.modified_at,
        place,
    })
}

/// `GET /api/takeout/archives`: the `.zip` and `.tgz` files in the
/// person's **Google Takeout** folder and at the top of their My Drive,
/// where people put downloads, by name, so an export's parts come in
/// order.
async fn archives(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<ArchivesView>, ApiError> {
    let storage = storage(&state)?;
    let top = storage.top_folder(me.user.id).await?;
    let folder = storage
        .find(top, vec![takeout::imports::FOLDER.to_owned()])
        .await?
        .filter(|item| item.kind == ItemKind::Folder);
    let mut archives = Vec::new();
    if let Some(folder) = &folder {
        archives.extend(
            storage
                .children(folder.id)
                .await?
                .into_iter()
                .filter_map(|item| archive(item, "takeout")),
        );
    }
    archives.extend(
        storage
            .children(top)
            .await?
            .into_iter()
            .filter_map(|item| archive(item, "top")),
    );
    archives.sort_by(|a, b| a.name.cmp(&b.name).then(a.place.cmp(b.place)));
    Ok(Json(ArchivesView {
        archives,
        folder: folder.map(|folder| folder.id),
    }))
}

#[derive(Serialize)]
struct FolderView {
    id: ItemId,
}

/// `POST /api/takeout/folder`: **Google Takeout** in the person's My
/// Drive, made if it isn't there, for the page to upload archives to.
async fn folder(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<FolderView>, ApiError> {
    let storage = storage(&state)?;
    let top = storage.top_folder(me.user.id).await?;
    let name = takeout::imports::FOLDER;
    let id = match storage.find(top, vec![name.to_owned()]).await? {
        Some(item) if item.kind == ItemKind::Folder => item.id,
        _ => {
            let by = storage.acting_as(accordion_module::auth::actor(&me));
            by.create_folder(top, name).await?.id
        }
    };
    Ok(Json(FolderView { id }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanRequest {
    parts: Vec<ItemId>,
}

#[derive(Serialize)]
struct PlanView {
    parts: Vec<PartView>,
    folders: Vec<FolderFoundView>,
    skipped: Vec<SkippedView>,
    skipped_count: u64,
    /// How much more the person can store, or null with no limit.
    room: Option<u64>,
}

#[derive(Serialize)]
struct PartView {
    format: String,
    entries: u64,
}

#[derive(Serialize)]
struct FolderFoundView {
    /// Its name in the archive, such as `Keep`, which is how the page asks
    /// for a folder no app converts.
    folder: String,
    /// The product that claimed it, such as `keep`, or null.
    product: Option<String>,
    /// The Google product and the app it goes to, for a product.
    google: Option<&'static str>,
    app: Option<&'static str>,
    items: u64,
    bytes: u64,
}

#[derive(Serialize)]
struct SkippedView {
    path: String,
    why: String,
}

/// The Google product and Accordion app for a product ID.
fn names(product: &str) -> (Option<&'static str>, Option<&'static str>) {
    match product {
        "keep" => (Some("Google Keep"), Some("Notes")),
        "drive" => (Some("Google Drive"), Some("Drive")),
        _ => (None, None),
    }
}

/// `POST /api/takeout/plan`: what the archives hold, which takes a while
/// for big ones, since the sandbox reads each part from its start.
async fn plan(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<PlanRequest>,
) -> Result<Json<PlanView>, ApiError> {
    let imports = imports(&state)?;
    let plan = imports
        .plan(me.user.id, &request.parts)
        .await
        .map_err(api_error)?;
    let room = imports.room(me.user.id).await.map_err(api_error)?;
    Ok(Json(PlanView {
        parts: plan
            .parts
            .into_iter()
            .map(|part| PartView {
                format: part.format,
                entries: part.entries,
            })
            .collect(),
        folders: plan
            .folders
            .into_iter()
            .map(|folder| {
                let (google, app) = folder.product.as_deref().map_or((None, None), names);
                FolderFoundView {
                    folder: folder.folder,
                    product: folder.product,
                    google,
                    app,
                    items: folder.items,
                    bytes: folder.bytes,
                }
            })
            .collect(),
        skipped: plan
            .skipped
            .into_iter()
            .map(|skipped| SkippedView {
                path: skipped.path,
                why: skipped.why,
            })
            .collect(),
        skipped_count: plan.skipped_count,
        room,
    }))
}

/// What to import: a product, or a folder no app converts, by name.
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "snake_case")]
enum TakeRequest {
    Product(String),
    Other(String),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRequest {
    parts: Vec<ItemId>,
    take: Vec<TakeRequest>,
}

#[derive(Serialize)]
struct Started {
    id: Uuid,
}

/// `POST /api/takeout/imports`: starts importing in the background. Its
/// first pass counts the items and checks there's room for them.
async fn start(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Json(request): Json<StartRequest>,
) -> Result<(StatusCode, Json<Started>), ApiError> {
    let take: Vec<Take> = request
        .take
        .into_iter()
        .map(|take| match take {
            TakeRequest::Product(product) => Take::Product(product),
            TakeRequest::Other(folder) => Take::Other(folder),
        })
        .collect();
    let id = imports(&state)?
        .start(me.user.id, &request.parts, &take)
        .await
        .map_err(api_error)?;
    Ok((StatusCode::CREATED, Json(Started { id })))
}

#[derive(Serialize)]
struct ImportView {
    id: Uuid,
    /// `running`, `canceling`, `done`, `canceled`, or `failed`.
    state: &'static str,
    /// How many items it takes, once it has counted them.
    items: Option<u64>,
    done: u64,
    started_at: String,
    finished_at: Option<String>,
    error: Option<String>,
    /// The report, once it's saved, if it's still there.
    report: Option<ReportView>,
}

#[derive(Serialize)]
struct ReportView {
    id: ItemId,
    name: String,
    /// The folder it's in, for opening it in Drive.
    folder: Option<ItemId>,
}

#[derive(Serialize)]
struct ImportsView {
    imports: Vec<ImportView>,
}

async fn view(state: &AppState, me: UserId, import: Import) -> Result<ImportView, ApiError> {
    let report = match import.report {
        Some(id) => storage(state)?
            .item(id)
            .await?
            .filter(|item| item.owner == me && item.trashed_at.is_none())
            .map(|item| ReportView {
                id: item.id,
                name: item.name,
                folder: item.parent,
            }),
        None => None,
    };
    Ok(ImportView {
        id: import.id,
        state: import.state.as_str(),
        items: import.items,
        done: import.done,
        started_at: import.started_at,
        finished_at: import.finished_at,
        error: import.error,
        report,
    })
}

/// `GET /api/takeout/imports`: the person's imports, newest first.
async fn list(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
) -> Result<Json<ImportsView>, ApiError> {
    let found = imports(&state)?
        .list(me.user.id, LISTED)
        .await
        .map_err(api_error)?;
    let mut views = Vec::with_capacity(found.len());
    for import in found {
        views.push(view(&state, me.user.id, import).await?);
    }
    Ok(Json(ImportsView { imports: views }))
}

/// `POST /api/takeout/imports/{id}/cancel`: stops the import after the
/// item it's on. It still writes its report.
async fn cancel(
    State(state): State<AppState>,
    Authenticated(me): Authenticated,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    imports(&state)?
        .cancel(me.user.id, id)
        .await
        .map_err(api_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::is_archive;

    #[test]
    fn knows_takeouts_archives_by_name() {
        assert!(is_archive("takeout-20261124T065651Z-1-001.zip"));
        assert!(is_archive("takeout-20261124T065747Z-2-002.TGZ"));
        assert!(is_archive("takeout.tar.gz"));
        assert!(!is_archive("Report.pdf"));
    }
}
