//! What the service holds, and how it reaches a provider for one user.

use std::collections::BTreeMap;
use std::sync::Arc;

use craft_cloud::{Permission, Provider};
use craft_nextcloud::{Account, Nextcloud};

use crate::session::Session;
use crate::transport::ReqwestHttp;

/// Which Craft apps a user may open.
///
/// Decided here rather than by the provider, because this is CollabCraft's own
/// question. A user with a perfectly good cloud account may still not be
/// entitled to run every app.
///
/// Configuration is a map of group name to app list, plus a default applied to
/// everyone who signs in. Groups arrive from the identity provider, so adding
/// somebody to a group in Authentik is the whole of granting access.
pub struct Permissions {
    default: Vec<String>,
    by_group: BTreeMap<String, Vec<String>>,
}

impl Permissions {
    /// Read from `COLLABCRAFT_APPS_DEFAULT` and `COLLABCRAFT_APPS_<GROUP>`.
    ///
    /// Nothing is compiled in, so a deployment decides its own catalogue and
    /// its own entitlements without a rebuild.
    pub fn from_env() -> Permissions {
        let default = split(&std::env::var("COLLABCRAFT_APPS_DEFAULT").unwrap_or_default());
        let mut by_group = BTreeMap::new();
        for (key, value) in std::env::vars() {
            if let Some(group) = key.strip_prefix("COLLABCRAFT_APPS_")
                && group != "DEFAULT"
            {
                by_group.insert(group.to_ascii_lowercase(), split(&value));
            }
        }
        Permissions { default, by_group }
    }

    /// Every app this session may open, deduplicated and sorted.
    pub fn apps_for_session(&self, session: &Session) -> Vec<String> {
        let mut out = self.default.clone();
        for group in &session.groups {
            if let Some(extra) = self.by_group.get(&group.to_ascii_lowercase()) {
                out.extend(extra.iter().cloned());
            }
        }
        out.sort();
        out.dedup();
        out
    }

    pub fn may_open(&self, session: &Session, app: &str) -> bool {
        self.apps_for_session(session).iter().any(|a| a == app)
    }
}

fn split(raw: &str) -> Vec<String> {
    raw.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// Everything the service needs at runtime.
pub struct App {
    /// Signs relay tokens. Shared with the relay and with nothing else.
    pub secret: Vec<u8>,
    /// Handed to clients so the relay can move without reissuing apps.
    pub relay_url: String,
    /// Where the provider lives. One per deployment for now; the shape allows
    /// more than one later, because nothing above here names a provider.
    pub provider_base: String,
    pub permissions: Permissions,
    /// Session and link storage, on the Valkey the rest of the estate uses.
    pub store: crate::store::Store,
    /// Absent when this deployment runs without sign-in, which is a local
    /// convenience and never right in a deployment.
    pub oidc: Option<crate::session::Oidc>,
    /// Shared client for talking to the identity provider. One, because each
    /// new one costs a connection pool.
    pub http: reqwest::Client,
    /// Serves the browser builds, behind the per-app permission check.
    pub web: tower_http::services::ServeDir,
    /// Directory names under the web root, which are the apps this deployment
    /// actually hosts. Read once at startup rather than from disk per request.
    pub hosted: Vec<String>,
}

impl App {
    /// Whether a path segment names an app this deployment hosts.
    pub fn is_app(&self, name: &str) -> bool {
        !name.is_empty() && self.hosted.iter().any(|a| a == name)
    }

    /// A provider acting as this user, or `None` when no account is linked.
    ///
    /// The credential never leaves this process. A client gets answers, never
    /// the means to produce them itself.
    pub async fn provider_for(&self, session: &Session) -> Option<Arc<dyn Provider + Send + Sync>> {
        let link = self.store.provider_link(&session.user).await?;
        let account = Account::new(self.provider_base.clone(), link.user, link.password);
        Some(Arc::new(Nextcloud::new(account, ReqwestHttp::default())))
    }

    /// Sign a relay token for one room.
    ///
    /// The claims are the smallest set the relay needs to decide everything by
    /// itself: which room, who, what they may do, and when it stops being
    /// true. The relay never calls back here.
    pub fn mint_token(&self, session: &Session, room: &str, permission: Permission) -> Result<String, String> {
        // A peer id per token, not per user. Two windows open on the same
        // document are two peers, and the roster has to tell them apart.
        let peer = crate::session::random_id();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        crate::token::sign_at(
            &self.secret,
            room,
            &session.user,
            &session.display,
            matches!(permission, Permission::Edit),
            &peer,
            now,
        )
        .map_err(|e| e.to_string())
    }
}
