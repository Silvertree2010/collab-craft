//! The share dialog's backend, reaching the CollabCraft service.
//!
//! `craft-ui` draws the dialog and states each piece of work as a
//! `share::Request`. Nothing in that crate implements `share::ShareBackend`,
//! which is what keeps it free of a transport and buildable for the web. This
//! is the half that opens a socket.
//!
//! It lives beside the client rather than beside the dialog. Twelve
//! applications bind this one crate instead of each writing the same adapter,
//! and craft-ui never learns that a service exists.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::sync::Arc;

use craft_cloud_client::{Client, ShareRequest};

use craft_ui::share::{Answer, Destination, Failure, Replier, Request, ShareBackend};

/// Where each request's client comes from.
#[derive(Clone)]
enum Source {
    /// One client for the life of the dialog.
    Fixed(Client),
    /// A client built for each request.
    ///
    /// A desktop application collects its session through a browser,
    /// possibly long after the window opened, and keeps it somewhere only
    /// the application knows. Asking for a client per request means a share
    /// opened after that sign-in finds the session without a restart.
    Made(Arc<dyn Fn() -> Option<Client> + Send + Sync>),
}

/// A [`ShareBackend`] that reaches the CollabCraft service.
///
/// This is the only part of the dialog that opens a socket, and it opens it
/// to CollabCraft. Which provider sits behind that, and what credential
/// reaches it, are the service's business and are configured in one place.
#[derive(Clone)]
pub struct CollabCraft {
    source: Source,
}

impl CollabCraft {
    /// A backend on one client, for a host that already holds a session.
    pub fn new(client: Client) -> CollabCraft {
        CollabCraft { source: Source::Fixed(client) }
    }

    /// A backend that asks the host for a client each time, so a session
    /// collected later is picked up without a restart.
    ///
    /// `None` from the factory means there is no service configured, which
    /// leaves the dialog unavailable rather than broken.
    pub fn from_fn(make: impl Fn() -> Option<Client> + Send + Sync + 'static) -> CollabCraft {
        CollabCraft { source: Source::Made(Arc::new(make)) }
    }

    /// A backend from the environment: `CRAFT_CLOUD_URL`, and
    /// `CRAFT_CLOUD_TOKEN` when a session has already been collected.
    ///
    /// Neither is compiled in, which is what lets one build talk to any
    /// deployment. `None` means there is no service to talk to.
    pub fn from_env() -> Option<CollabCraft> {
        let mut client = Client::new(var("CRAFT_CLOUD_URL")?);
        if let Some(token) = var("CRAFT_CLOUD_TOKEN") {
            client = client.with_token(token);
        }
        Some(CollabCraft::new(client))
    }
}

fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

fn client(source: &Source) -> Option<Client> {
    match source {
        Source::Fixed(client) => Some(client.clone()),
        Source::Made(make) => make(),
    }
}

impl ShareBackend for CollabCraft {
    fn submit(&self, request: Request, reply: Replier) {
        let source = self.source.clone();
        // One thread per request. The dialog debounces its autocomplete,
        // so this is a handful at a time, and a lookup that is still
        // waiting cannot hold up a share the user has already pressed.
        // The client is built on this thread too: reading a saved session
        // off disk is not work for a frame.
        let spare = reply.dup();
        let started = std::thread::Builder::new().name("craft-share".to_owned()).spawn(move || match client(&source) {
            None => reply.refuse("no cloud service is configured"),
            Some(client) => match run(&client, request) {
                Ok(answer) => reply.answer(answer),
                Err(reason) => reply.refuse(reason),
            },
        });
        if let Err(e) = started {
            // The moved handle went with the closure that never ran, so the
            // spare is the only way to stop the dialog waiting forever.
            spare.refuse(format!("no thread to run this on: {e}"));
        }
    }
}

fn run(client: &Client, request: Request) -> Result<Answer, String> {
    match request {
        Request::Capabilities => {
            let me = client.me().map_err(say)?;
            Ok(Answer::Capabilities { capabilities: me.capabilities, linked: me.provider_linked })
        }
        Request::Lookup { query, limit } => client.contacts(&query, limit).map(Answer::Lookup).map_err(say),
        Request::Access { id } => client.shares(&id).map(Answer::Access).map_err(say),
        Request::Grant(grant) => {
            let request = ShareRequest {
                id: grant.id,
                targets: grant.targets,
                permission: grant.permission,
                note: grant.note,
                notify: grant.notify,
                password: grant.password,
                expires: None,
            };
            let outcome = client.share(&request).map_err(say)?;
            let failed = outcome.failed.into_iter().map(|f| Failure { target: f.target, code: f.code, detail: f.detail }).collect();
            Ok(Answer::Granted { shared: outcome.shared, failed })
        }
        Request::Withdraw { share_id } => {
            client.unshare(&share_id).map_err(say)?;
            Ok(Answer::Withdrawn { share_id })
        }
        // A contact's avatar URL is the provider's own, and an app reaching
        // it would be an app talking to the provider, which is the thing
        // this whole arrangement exists to prevent. Until CollabCraft
        // relays pictures, the dialog paints initials. A host that has
        // decided it may fetch them wraps this backend and answers this
        // one request itself; nothing else changes.
        Request::Avatar { .. } => Err("this build does not fetch contact pictures".to_owned()),

        // One destination today, described by the account behind it.
        // Everything above the contracts treats this as a list, so a
        // second provider is an entry rather than a change.
        Request::Destinations => {
            let me = client.me().map_err(|e| e.to_string())?;
            Ok(Answer::Destinations(vec![Destination {
                id: "cloud".to_owned(),
                name: "Cloud".to_owned(),
                account: me.email.clone().or(Some(me.display.clone())),
                linked: me.provider_linked,
            }]))
        }

        Request::Link { .. } => {
            let start = client.link_start().map_err(|e| e.to_string())?;
            // Opening a browser is the host application's business, so the
            // URL is reported rather than launched from here.
            Err(format!("open {} to connect an account", start.login_url))
        }
    }
}

/// A client error in words a user can read.
fn say(e: craft_cloud_client::Error) -> String {
    e.to_string()
}
