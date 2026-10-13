//! The CollabCraft API.
//!
//! This is the only thing a Craft app talks to. Apps hold no provider
//! credential, know no provider URL and implement no provider protocol. They
//! ask here, and here decides which provider answers.
//!
//! That split is the whole design. A desktop binary carrying a cloud app
//! password leaks one the moment somebody copies the file, and every app would
//! reimplement WebDAV, OCS and CardDAV to use it. Both problems go away when
//! one service holds the credential and speaks the protocols.
//!
//! Every route is scoped to a signed-in session. Nothing here acts without
//! knowing who is asking.
//!
//! The contracts are synchronous, because a provider crate that opens no
//! socket has no reason to be async. Handlers therefore push provider work
//! onto a blocking thread with [`run`]. Calling it inline would stall the
//! runtime, and building a blocking HTTP client inside a runtime panics
//! outright.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use craft_cloud::{Permission, Provider, ShareTarget};
use serde::{Deserialize, Serialize};

use crate::session::Session;
use crate::state::App;

type Shared = Arc<dyn Provider + Send + Sync>;

/// What a failed call looks like on the wire.
///
/// The shape is stable so a client acts on `code` without reading `detail`,
/// which is for a human.
#[derive(Serialize)]
pub struct ApiError {
    pub code: &'static str,
    pub detail: String,
}

/// Map a contract error onto a status and a stable code.
///
/// The distinctions matter to a UI. `unauthenticated` offers a sign-in,
/// `forbidden` must not, `conflict` offers to reload, `unsupported` hides the
/// feature rather than explaining it.
fn fail(e: craft_cloud::Error) -> Response {
    use craft_cloud::Error as E;
    let (status, code) = match &e {
        E::Unreachable(_) => (StatusCode::BAD_GATEWAY, "unreachable"),
        E::Unauthenticated => (StatusCode::UNAUTHORIZED, "unauthenticated"),
        E::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
        E::NotFound { .. } => (StatusCode::NOT_FOUND, "not_found"),
        E::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
        E::Unsupported(_) => (StatusCode::NOT_IMPLEMENTED, "unsupported"),
        E::Malformed { .. } => (StatusCode::BAD_GATEWAY, "malformed"),
        E::Invalid { .. } => (StatusCode::BAD_REQUEST, "invalid"),
    };
    (status, Json(ApiError { code, detail: e.to_string() })).into_response()
}

/// Run one blocking provider call and render whatever it returns as JSON.
async fn run<T, F>(f: F) -> Response
where
    T: Serialize + Send + 'static,
    F: FnOnce() -> craft_cloud::Result<T> + Send + 'static,
{
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err(e)) => fail(e),
        // The thread panicked. Reported rather than swallowed, because a
        // provider that panics is a defect somebody has to see.
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { code: "panic", detail: e.to_string() }),
        )
            .into_response(),
    }
}

/// No provider account linked yet.
///
/// Its own code, because the fix is specific: the user links an account. It is
/// neither a failed sign-in nor a permission problem, and offering either
/// would send them somewhere useless.
fn not_linked() -> Response {
    (
        StatusCode::PRECONDITION_REQUIRED,
        Json(ApiError {
            code: "provider_not_linked",
            detail: "link a cloud account before opening or saving documents".to_owned(),
        }),
    )
        .into_response()
}

// ---- who am I -------------------------------------------------------------

#[derive(Serialize)]
pub struct Me {
    pub id: String,
    pub display: String,
    pub email: Option<String>,
    pub avatar: Option<String>,
    /// Which Craft apps this user may open. Decided here, not by the client:
    /// a client asking itself would be no check at all.
    pub apps: Vec<String>,
    /// What the provider can actually do, so a UI hides what it cannot
    /// deliver rather than failing on use.
    pub capabilities: Vec<craft_cloud::Capability>,
    /// Until an account is linked, Open and Save have nowhere to go.
    pub provider_linked: bool,
}

pub async fn me(State(app): State<Arc<App>>, session: Session) -> Response {
    let apps = app.permissions.apps_for_session(&session);
    let Some(provider) = app.provider_for(&session).await else {
        return Json(Me {
            id: session.user,
            display: session.display,
            email: session.email,
            avatar: None,
            apps,
            capabilities: Vec::new(),
            provider_linked: false,
        })
        .into_response();
    };

    let capabilities = provider.capabilities();
    let p = provider.clone();
    let identity = tokio::task::spawn_blocking(move || craft_cloud::Accounts::me(p.as_ref())).await;
    match identity {
        Ok(Ok(id)) => Json(Me {
            id: session.user,
            display: id.display,
            email: id.email.or(session.email),
            avatar: id.avatar,
            apps,
            capabilities,
            provider_linked: true,
        })
        .into_response(),
        Ok(Err(e)) => fail(e),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { code: "panic", detail: e.to_string() }))
            .into_response(),
    }
}

// ---- files ----------------------------------------------------------------

#[derive(Deserialize)]
pub struct ListQuery {
    /// Folder to list. Absent means the user's root.
    #[serde(default)]
    pub path: String,
}

pub async fn list(State(app): State<Arc<App>>, session: Session, Query(q): Query<ListQuery>) -> Response {
    let Some(p): Option<Shared> = app.provider_for(&session).await else { return not_linked() };
    run(move || p.list(&q.path)).await
}

/// Everything other people have shared with this user.
///
/// A separate route because it is a separate question. These entries have no
/// common parent folder, so no amount of listing would find them.
pub async fn shared_with_me(State(app): State<Arc<App>>, session: Session) -> Response {
    let Some(p): Option<Shared> = app.provider_for(&session).await else { return not_linked() };
    run(move || p.shared_with_me()).await
}

#[derive(Deserialize)]
pub struct IdQuery {
    pub id: String,
}

pub async fn read(State(app): State<Arc<App>>, session: Session, Query(q): Query<IdQuery>) -> Response {
    let Some(p): Option<Shared> = app.provider_for(&session).await else { return not_linked() };
    match tokio::task::spawn_blocking(move || p.read(&q.id)).await {
        Ok(Ok(bytes)) => ([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Ok(Err(e)) => fail(e),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { code: "panic", detail: e.to_string() }))
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct WriteQuery {
    pub id: String,
    /// The version the client last saw. Sent back so a save refuses to
    /// overwrite work somebody else saved in the meantime.
    #[serde(default)]
    pub if_match: Option<String>,
}

pub async fn write(
    State(app): State<Arc<App>>,
    session: Session,
    Query(q): Query<WriteQuery>,
    body: axum::body::Bytes,
) -> Response {
    let Some(p): Option<Shared> = app.provider_for(&session).await else { return not_linked() };
    run(move || p.write(&q.id, &body, q.if_match.as_deref())).await
}

#[derive(Deserialize)]
pub struct CreateQuery {
    pub folder: String,
    pub name: String,
}

pub async fn create(
    State(app): State<Arc<App>>,
    session: Session,
    Query(q): Query<CreateQuery>,
    body: axum::body::Bytes,
) -> Response {
    let Some(p): Option<Shared> = app.provider_for(&session).await else { return not_linked() };
    run(move || p.create(&q.folder, &q.name, &body)).await
}

// ---- contacts, for the share dialog ---------------------------------------

#[derive(Deserialize)]
pub struct SearchQuery {
    #[serde(default)]
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    20
}

/// People matching what has been typed so far.
///
/// Called on each keystroke, so the limit is enforced here as well as asked of
/// the provider. A dialog that pulls a thousand contacts to show eight only
/// hurts on somebody else's address book.
pub async fn contacts(State(app): State<Arc<App>>, session: Session, Query(q): Query<SearchQuery>) -> Response {
    let Some(p): Option<Shared> = app.provider_for(&session).await else { return not_linked() };
    let limit = q.limit.clamp(1, 50);
    run(move || p.search(&q.q, limit)).await
}

// ---- sharing --------------------------------------------------------------

#[derive(Deserialize)]
pub struct ShareRequest {
    /// The document's provider handle.
    pub id: String,
    /// Everyone to share with, in one call. The dialog is multi-select, so a
    /// request per recipient would be several round trips and a partial
    /// failure the user cannot see.
    pub targets: Vec<ShareTarget>,
    pub permission: Permission,
    #[serde(default)]
    pub note: Option<String>,
    /// Ask the provider to tell the recipient.
    #[serde(default)]
    pub notify: bool,
    /// A key the recipient must present. Only meaningful for a link, and
    /// refused by a server that does not allow one.
    #[serde(default)]
    pub password: Option<String>,
    /// `YYYY-MM-DD`, after which the share stops working.
    #[serde(default)]
    pub expires: Option<String>,
}

#[derive(Serialize)]
pub struct ShareOutcome {
    pub shared: Vec<craft_cloud::Share>,
    /// One entry per target that failed, so the dialog can say which.
    pub failed: Vec<ShareFailure>,
}

#[derive(Serialize)]
pub struct ShareFailure {
    pub target: ShareTarget,
    pub code: &'static str,
    pub detail: String,
}

/// Share with several people at once.
///
/// Partial success is reported rather than hidden. Sharing with five people
/// where one address is wrong should share with four and name the one that
/// failed, not fail the lot and not claim all five worked.
pub async fn share(State(app): State<Arc<App>>, session: Session, Json(req): Json<ShareRequest>) -> Response {
    let Some(p): Option<Shared> = app.provider_for(&session).await else { return not_linked() };
    if !p.supports(craft_cloud::Capability::Sharing) {
        return fail(craft_cloud::Error::Unsupported("sharing"));
    }

    let outcome = tokio::task::spawn_blocking(move || {
        let mut shared = Vec::new();
        let mut failed = Vec::new();
        for target in &req.targets {
            let options = craft_cloud::ShareOptions {
                note: req.note.clone(),
                notify: req.notify,
                password: req.password.clone(),
                expires: req.expires.clone(),
            };
            match p.share(&req.id, target, req.permission, &options) {
                Ok(s) => shared.push(s),
                Err(e) => {
                    let code = match &e {
                        craft_cloud::Error::NotFound { .. } => "not_found",
                        craft_cloud::Error::Forbidden(_) => "forbidden",
                        craft_cloud::Error::Unreachable(_) => "unreachable",
                        _ => "failed",
                    };
                    failed.push(ShareFailure { target: target.clone(), code, detail: e.to_string() });
                }
            }
        }
        ShareOutcome { shared, failed }
    })
    .await;

    match outcome {
        Ok(o) => Json(o).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { code: "panic", detail: e.to_string() }))
            .into_response(),
    }
}

pub async fn shares(State(app): State<Arc<App>>, session: Session, Query(q): Query<IdQuery>) -> Response {
    let Some(p): Option<Shared> = app.provider_for(&session).await else { return not_linked() };
    run(move || p.shares(&q.id)).await
}

pub async fn unshare(State(app): State<Arc<App>>, session: Session, Path(share_id): Path<String>) -> Response {
    let Some(p): Option<Shared> = app.provider_for(&session).await else { return not_linked() };
    match tokio::task::spawn_blocking(move || p.unshare(&share_id)).await {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(e)) => fail(e),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { code: "panic", detail: e.to_string() }))
            .into_response(),
    }
}

// ---- the collaboration token ----------------------------------------------

#[derive(Serialize)]
pub struct TokenResponse {
    pub token: String,
    /// Given out rather than compiled into a client, so the relay can move
    /// without reissuing every app.
    pub relay: String,
    /// The room this token is for, which is the document's stable id.
    pub room: String,
}

/// Mint a relay token for one document.
///
/// Authorisation happens here, against the provider, which is the only thing
/// that knows whether this user may open this file. The relay then verifies a
/// signature and never asks again, so this is once per document rather than
/// once per edit.
pub async fn token(State(app): State<Arc<App>>, session: Session, Query(q): Query<IdQuery>) -> Response {
    let Some(p): Option<Shared> = app.provider_for(&session).await else { return not_linked() };

    // Stat rather than trust the client. An id arriving in a request proves
    // nothing about whether this user may open it, and the provider is the
    // authority on that.
    let entry = match tokio::task::spawn_blocking(move || p.stat(&q.id)).await {
        Ok(Ok(e)) => e,
        Ok(Err(e)) => return fail(e),
        Err(e) => {
            return (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { code: "panic", detail: e.to_string() }))
                .into_response();
        }
    };
    let permission = if entry.writable { Permission::Edit } else { Permission::View };
    match app.mint_token(&session, &entry.room, permission) {
        Ok(token) => Json(TokenResponse { token, relay: app.relay_url.clone(), room: entry.room }).into_response(),
        Err(detail) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { code: "mint_failed", detail })).into_response()
        }
    }
}

// ---- serving the browser builds -------------------------------------------

/// Serve a browser build, if this user is entitled to that app.
///
/// The first path segment is the app. `/gridcraft/index.html` is GridCraft, and
/// a user without it gets 403 rather than the page. Enforcing it here rather
/// than in the client is the point: a client asking itself whether it may run
/// is no check at all.
///
/// Anything that is not an app directory is served unconditionally. Shared
/// assets and the index have nothing to gate.
pub async fn web(
    State(app): State<Arc<App>>,
    session: Option<Session>,
    req: axum::extract::Request,
) -> Response {
    let path = req.uri().path().trim_start_matches('/').to_owned();
    let first = path.split('/').next().unwrap_or_default().to_owned();

    if app.is_app(&first) {
        let Some(session) = session else {
            // Send them to sign in rather than refusing flatly: an anonymous
            // visitor has not been denied anything yet.
            return axum::response::Redirect::temporary("/auth/login").into_response();
        };
        if !app.permissions.may_open(&session, &first) {
            return (
                StatusCode::FORBIDDEN,
                Json(ApiError {
                    code: "app_not_permitted",
                    detail: format!("you do not have access to {first}"),
                }),
            )
                .into_response();
        }
    }

    match tower::ServiceExt::oneshot(app.web.clone(), req).await {
        Ok(r) => {
            let mut response = r.into_response();
            if let Some(value) = cache_for(&path) {
                response.headers_mut().insert(axum::http::header::CACHE_CONTROL, value);
            }
            response
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiError { code: "static", detail: e.to_string() }),
        )
            .into_response(),
    }
}

/// Which apps this user may open.
pub async fn apps(State(app): State<Arc<App>>, session: Session) -> Response {
    Json(app.permissions.apps_for_session(&session)).into_response()
}

/// How long a static asset may be cached.
///
/// A one-year immutable cache is only sound where the filename carries a
/// content hash, because the hash is then the cache key. Trunk emits those, as
/// in `gridcraft-web-b92841ed094e19aa_bg.wasm`. Several apps are built with
/// wasm-bindgen directly and emit plain names, as do the shared workers.
/// Marking one of those immutable means a redeploy never reaches anybody who
/// has already visited.
fn cache_for(path: &str) -> Option<axum::http::HeaderValue> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let value = if name.is_empty() || name.ends_with(".html") {
        // The entry point carries no hash and must never be cached, or a
        // deploy is invisible.
        "no-cache"
    } else if (name.ends_with(".wasm") || name.ends_with(".js")) && has_content_hash(name) {
        "public, max-age=31536000, immutable"
    } else if name.ends_with(".wasm") || name.ends_with(".js") {
        "public, max-age=300, must-revalidate"
    } else if name.ends_with(".otf") || name.ends_with(".ttf") || name.ends_with(".woff") || name.ends_with(".woff2") {
        // Fonts are large and change rarely. One app ships a 16 MB CJK face.
        "public, max-age=86400"
    } else {
        return None;
    };
    axum::http::HeaderValue::from_str(value).ok()
}

/// Whether a filename carries a content hash, as `-<hex>` before the
/// extension.
fn has_content_hash(name: &str) -> bool {
    let stem = name.trim_end_matches(".wasm").trim_end_matches(".js").trim_end_matches("_bg");
    stem.rsplit('-').next().is_some_and(|tail| tail.len() >= 8 && tail.chars().all(|c| c.is_ascii_hexdigit()))
}
