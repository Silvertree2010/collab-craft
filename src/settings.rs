//! What a deployment configures about its cloud provider, and who may change
//! it.
//!
//! The environment is the default. A value stored on the bus overrides it. An
//! existing deployment therefore keeps working with nothing touched, and a new
//! one can be pointed at its provider from a page in a browser.
//!
//! Who may change it comes from the identity provider. Membership of a named
//! group is the whole of the permission. That is how every other entitlement
//! in this service is decided, so this one is decided the same way.
//!
//! Nothing here holds a credential. The address of a provider is configuration
//! and the account that acts against it is linked per user, which keeps those
//! two questions apart on purpose.

use serde::{Deserialize, Serialize};

use crate::session::Session;

/// The provider implementations this binary speaks.
///
/// One today. The list lives here so the setup page can offer a choice out of
/// what the binary actually implements. Anything else is refused at the point
/// of saving.
pub const KINDS: [&str; 1] = ["nextcloud"];

/// Which provider answers, and where it lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// A stable identifier such as `nextcloud`. Used in configuration and in
    /// logs, never as a label for a user.
    pub kind: String,
    /// The server root, as `https://cloud.example.com`.
    pub base: String,
}

impl ProviderConfig {
    /// Check and normalise what somebody typed into the setup form.
    ///
    /// The refusals are deliberately specific, because an administrator who
    /// pasted the wrong thing can act on each one. A trailing slash is
    /// accepted and removed, since every call site trims one anyway and
    /// storing the difference would be storing a trap.
    pub fn parse(kind: &str, base: &str) -> Result<ProviderConfig, &'static str> {
        let kind = kind.trim().to_ascii_lowercase();
        if !KINDS.contains(&kind.as_str()) {
            return Err("this service does not implement that kind of provider");
        }
        let base = base.trim().trim_end_matches('/').to_owned();
        let rest =
            base.strip_prefix("https://").or_else(|| base.strip_prefix("http://")).ok_or("the address has to begin with https:// or http://")?;
        if rest.is_empty() || rest.starts_with('/') {
            return Err("the address names no host");
        }
        // An address carrying a credential would put one in configuration
        // every administrator can read. A query or a fragment means somebody
        // pasted a page out of the provider's own interface.
        if rest.contains(['@', '?', '#', ' ']) {
            return Err("the address has to be a plain server address, with no credential, no query and no fragment");
        }
        Ok(ProviderConfig { kind, base })
    }

    /// What to call this provider in a heading.
    pub fn label(&self) -> String {
        crate::pages::capitalise(&self.kind)
    }
}

/// Where the provider in use came from.
///
/// Reported so the setup page can say which of the two a deployment is
/// running on. An administrator looking at an address wants to know whether
/// changing the environment would change anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Environment,
    Stored,
}

/// What this deployment was started with, and who may change it afterwards.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The environment's provider. The default, overridden by anything stored.
    env: Option<ProviderConfig>,
    /// Groups from the identity provider whose members may change the stored
    /// provider, lower-cased for comparison.
    admins: Vec<String>,
}

impl Settings {
    /// Read `COLLABCRAFT_PROVIDER_URL`, `COLLABCRAFT_PROVIDER_KIND` and
    /// `COLLABCRAFT_ADMIN_GROUPS`.
    pub fn from_env() -> Settings {
        Settings::decide(
            var("COLLABCRAFT_PROVIDER_URL").as_deref(),
            var("COLLABCRAFT_PROVIDER_KIND").as_deref(),
            var("COLLABCRAFT_ADMIN_GROUPS").as_deref(),
        )
    }

    /// The same decision over values.
    ///
    /// Separated so the override and the gate are testable. Reading the
    /// environment inside a test would race every other test in the binary.
    pub fn decide(url: Option<&str>, kind: Option<&str>, groups: Option<&str>) -> Settings {
        let kind = kind.map(str::trim).filter(|k| !k.is_empty()).unwrap_or(KINDS[0]);
        let env = url.map(str::trim).filter(|u| !u.is_empty()).and_then(|u| ProviderConfig::parse(kind, u).ok());
        let admins =
            groups.unwrap_or_default().split(',').map(str::trim).filter(|g| !g.is_empty()).map(str::to_ascii_lowercase).collect::<Vec<String>>();
        Settings { env, admins }
    }

    /// The environment's provider, which is the default.
    pub fn env_default(&self) -> Option<&ProviderConfig> {
        self.env.as_ref()
    }

    /// Which provider this deployment uses, and where that came from.
    ///
    /// A stored value wins. That ordering is the whole of the override: a
    /// deployment storing nothing behaves exactly as it did before this page
    /// existed, and one that stores a provider can be pointed somewhere new
    /// without an environment change.
    ///
    /// A stored record naming no address is ignored, so a half-written record
    /// cannot take a working deployment off its provider.
    pub fn resolve(&self, stored: Option<ProviderConfig>) -> Option<(ProviderConfig, Source)> {
        match stored {
            Some(stored) if !stored.base.trim().is_empty() => Some((stored, Source::Stored)),
            _ => self.env.clone().map(|c| (c, Source::Environment)),
        }
    }

    /// Groups whose members may change the stored provider.
    pub fn admin_groups(&self) -> &[String] {
        &self.admins
    }

    /// Whether this session may change the provider.
    ///
    /// A deployment naming no group has nobody who may, so the page refuses
    /// everybody and the control appears for nobody. A recipient holding a
    /// keyed link carries no group and is refused by the same rule, which is
    /// what keeps this closed to one without a second check anywhere.
    pub fn may_configure(&self, session: &Session) -> bool {
        if session.is_guest() || self.admins.is_empty() {
            return false;
        }
        session.groups.iter().any(|g| self.admins.contains(&g.to_ascii_lowercase()))
    }
}

/// Whether a provider answers at an address.
///
/// Asks for the beginning of the provider's own desktop login flow. That needs
/// no credential and no account, and a server answering it is reachable,
/// speaks the protocol and is willing to issue a credential. Proving all three
/// before the address is saved is the point: an address that only fails later
/// fails for every user at once.
///
/// What the provider hands back is dropped here. The flow is abandoned, and
/// the poll token it carries is never logged and never rendered.
pub async fn probe(config: &ProviderConfig) -> Result<(), String> {
    let config = config.clone();
    let probing = tokio::task::spawn_blocking(move || match config.kind.as_str() {
        "nextcloud" => craft_nextcloud::begin(&crate::transport::ReqwestHttp::default(), &config.base).map(|_| ()).map_err(|e| e.to_string()),
        other => Err(format!("this service does not implement a {other} provider")),
    })
    .await;
    match probing {
        Ok(answer) => answer,
        // The thread panicked. Reported, because a provider crate that panics
        // is a defect somebody has to see.
        Err(e) => Err(e.to_string()),
    }
}

fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}
