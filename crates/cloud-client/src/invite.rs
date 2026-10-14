//! Landing on a keyed link, in a browser.
//!
//! Somebody with no account followed a link in their mail. The service
//! redeemed the key, spent one use and set a guest session cookie, then sent
//! the browser to one of the twelve app builds. This module is what that build
//! asks on startup, and it answers one of three things: this is an ordinary
//! visit, this is a keyed link that opened, or this is a keyed link the
//! service refused and here is the sentence to show.
//!
//! # Why it lives here and not in the interface crate
//!
//! The flow is three calls to this service and one socket. Nothing in it
//! draws. [`Client`] already has the three calls and [`craft_collab`] already
//! has the socket, so putting the flow beside them leaves an application with
//! nothing to restate: no route, no cookie, no status code, no version
//! tracking and no ordering.
//!
//! A refusal also has to be readable before any canvas exists. An application
//! that never started has drawn nothing, so the one page a recipient must
//! never miss cannot be drawn by the application's own interface layer.
//! [`refuse`] writes it into the document instead.
//!
//! # What an application still owns
//!
//! Turning bytes into one of its own documents, and pumping its own relay
//! session. Both need the application's document model, and no library can
//! hold that.
//!
//! # The order matters
//!
//! The token comes first, because its answer says whether any of this applies
//! and carries the permission, the name and the version. The bytes come
//! second, because an application that has them must be able to open them. The
//! relay comes last, through [`Invitation::join`], because a document nobody
//! can see is not improved by being shared live.
//!
//! # Nothing here holds a credential
//!
//! A recipient never has one. The session is a cookie this service set, the
//! browser attaches it, and the service reads the document as the sharer. So
//! every call below is made same-origin: a cookie does not cross an origin,
//! and a client pointed anywhere else would be a client with no session.

use std::cell::RefCell;
use std::rc::Rc;

use craft_cloud::{Entry, Permission};
use craft_collab::web::WebHub;
use wasm_bindgen::JsCast;

use crate::{Client, Error, InviteJoin, Result};

/// The code the service refuses a key with. See its `recipient::refuse_json`.
///
/// The one answer that means "this browser followed a keyed link and the link
/// is no good". Every other failure means something else, which is why the
/// code is matched rather than the status.
const LINK_INVALID: &str = "link_invalid";

/// What the landing page puts on the query when it hands a browser to an app
/// build. See the service's `recipient::handoff`.
///
/// It proves nothing and grants nothing. The cookie is the whole of the
/// authority. This only says that a keyed link is why this page was loaded, so
/// a service that cannot be reached is reported to the recipient instead of
/// being mistaken for an ordinary visit. Without it a network blip would show
/// somebody an empty document and no reason, which is the one outcome worse
/// than a plain refusal.
const MARKER: &str = "invite";

/// What this page load turned out to be.
pub enum Arrival {
    /// Not a keyed link. The application carries on exactly as it does
    /// otherwise, and says nothing about this. Most loads are these.
    Ordinary,
    /// A keyed link that is no good, with the sentence the recipient reads.
    /// Hand it to [`refuse`].
    Refused(String),
    /// A keyed link that opened.
    Invited(Invitation),
}

/// A document a recipient was sent, and what they may do with it.
pub struct Invitation {
    /// The bytes, as the service read them for this recipient. Hand them to
    /// whatever the application opens a file with.
    pub document: Vec<u8>,
    /// What to call it. From the service, because a recipient has no file list
    /// to have picked it from.
    pub name: String,
    /// The address the link was sent to. Say this where an account would
    /// otherwise be named. A recipient has no account, so there is nothing to
    /// sign out of and no account menu to draw.
    pub recipient: String,
    /// What the share still grants, decided by the service on this request.
    pub permission: Permission,
    join: InviteJoin,
    client: Client,
    /// The version the next save is conditional on. Held here rather than in
    /// the saver, so two savers taken from one invitation cannot each think
    /// they are saving over what was read at the start.
    etag: Rc<RefCell<Option<String>>>,
}

impl Invitation {
    /// Whether the document opens for reading only.
    pub fn read_only(&self) -> bool {
        self.permission != Permission::Edit
    }

    /// The room this document is shared in, for a relay session.
    pub fn room(&self) -> &str {
        &self.join.room
    }

    /// Where the relay is.
    pub fn relay(&self) -> &str {
        &self.join.relay
    }

    /// Connect to the relay, so edits reach whoever else has it open.
    ///
    /// The token, the address and the room all come from the one answer that
    /// admitted this recipient, so there is nothing for an application to
    /// assemble and nothing it can get wrong.
    ///
    /// One call per session. A second one opens a second socket into the same
    /// room, and the peer would then see its own edits arrive as somebody
    /// else's.
    ///
    /// An error here is not a reason to refuse the document. The recipient
    /// came to read one document and they have it. A caller says so in
    /// passing and carries on.
    pub fn join(&self) -> craft_collab::Result<WebHub> {
        WebHub::connect(&self.join.relay, &self.join.token)
    }

    /// How to save back, or `None` when the share is read only.
    ///
    /// `None` is the whole of honouring a read-only share: there is no saver
    /// to wire, so an application cannot offer a save the service would
    /// refuse.
    pub fn saver(&self) -> Option<Saver> {
        if self.read_only() {
            return None;
        }
        Some(Saver { client: self.client.clone(), etag: Rc::clone(&self.etag) })
    }
}

/// Saving a recipient's document back.
///
/// Cheap to clone, so the one an application wires into its Save can be kept
/// beside whatever else it holds. Every clone shares the version, which is
/// what makes two saves in a row behave: the second is conditional on what the
/// first stored rather than on what was read at the start.
#[derive(Clone)]
pub struct Saver {
    client: Client,
    /// The version the next save is conditional on. The service answers each
    /// save with the version it wrote, and that becomes this.
    etag: Rc<RefCell<Option<String>>>,
}

impl Saver {
    /// Save, refusing to overwrite work saved since this recipient last read
    /// or wrote.
    ///
    /// A refusal arrives as `Error::Refused` with the code `conflict`, which
    /// means somebody else's work is in the stored copy. That one is never
    /// retried and never forced.
    pub async fn save(&self, bytes: &[u8]) -> Result<Entry> {
        let condition = self.etag.borrow().clone();
        let entry = self.client.invite_write(bytes, condition.as_deref()).await?;
        *self.etag.borrow_mut() = entry.etag.clone();
        Ok(entry)
    }

    /// The same, started and left to finish.
    ///
    /// For the ordinary case: an application's Save is a plain call that
    /// returns nothing, and a browser has no thread to block. `done` is called
    /// with the outcome once the service has answered.
    pub fn save_in_background(&self, bytes: Vec<u8>, done: impl FnOnce(Result<Entry>) + 'static) {
        let saver = self.clone();
        wasm_bindgen_futures::spawn_local(async move {
            done(saver.save(&bytes).await);
        });
    }

    /// The version the next save is conditional on.
    pub fn version(&self) -> Option<String> {
        self.etag.borrow().clone()
    }
}

/// Ask the service whether a keyed link is why this page was loaded.
///
/// Call it once on startup, before anything is drawn. [`Arrival::Ordinary`] is
/// the usual answer and means carry on. Nothing is said to the user about it.
pub async fn arrive() -> Arrival {
    // Same-origin and never configurable. A cookie does not cross an origin,
    // so the service that served this page is the only one a recipient has a
    // session with.
    arrive_through(Client::same_origin()).await
}

/// The same, against a client a caller built.
///
/// For a host that reaches the service some other way. The ordinary case is
/// [`arrive`].
pub async fn arrive_through(client: Client) -> Arrival {
    let followed = followed_a_link();
    let join = match client.invite_token().await {
        Ok(join) => join,
        Err(e) => return not_a_link(e, followed),
    };
    // The token said yes, so this is a keyed link and every later failure is
    // the recipient's to read. Carrying on as an ordinary visit from here
    // would leave somebody looking at an empty document with no reason for it.
    match client.invite_read().await {
        Ok(document) => Arrival::Invited(Invitation {
            document,
            name: join.name.clone(),
            recipient: join.recipient.clone(),
            permission: join.permission,
            etag: Rc::new(RefCell::new(join.etag.clone())),
            join,
            client,
        }),
        Err(e) => Arrival::Refused(sentence(&e)),
    }
}

/// What a failed token call means.
///
/// The service answers a dead key with its own code, and that is a refusal
/// whoever followed the link has to read. Anything else is somebody who is not
/// a recipient: no session at all, or a signed-in account, which has the
/// ordinary routes instead. Those are ordinary visits and nothing is said
/// about them.
///
/// `followed` flips the unknown cases. A browser the landing page sent here is
/// known to be a recipient, so a service that cannot be reached is reported
/// rather than passed over in silence.
fn not_a_link(error: Error, followed: bool) -> Arrival {
    match &error {
        Error::Refused { code, detail } if code == LINK_INVALID => Arrival::Refused(public(detail)),
        _ if followed => Arrival::Refused(sentence(&error)),
        _ => Arrival::Ordinary,
    }
}

/// What a recipient is told when the service gave no sentence of its own.
const UNEXPLAINED: &str = "this link no longer works. Ask whoever sent it to share the document again";

fn public(detail: &str) -> String {
    let trimmed = detail.trim();
    if trimmed.is_empty() { UNEXPLAINED.to_owned() } else { trimmed.to_owned() }
}

/// One failure as a sentence for a person to read.
///
/// A recipient can act on one thing only, which is asking whoever sent the
/// link to send another. An unreachable service is worth telling apart,
/// because that one is worth trying again.
fn sentence(error: &Error) -> String {
    match error {
        Error::Refused { code, detail } if code == LINK_INVALID => public(detail),
        Error::Unreachable(detail) => format!("This service could not be reached. {detail}"),
        Error::Refused { detail, .. } => public(detail),
        _ => UNEXPLAINED.to_owned(),
    }
}

/// Whether the landing page is what sent this browser here.
///
/// Read off the query. `false` on a reload or a bookmark, which is why it only
/// ever widens what is reported and never decides whether a link is good. The
/// cookie decides that, and only the service can read it.
fn followed_a_link() -> bool {
    let Some(search) = web_sys::window().and_then(|w| w.location().search().ok()) else {
        return false;
    };
    search.trim_start_matches('?').split('&').any(|pair| {
        let name = pair.split('=').next().unwrap_or_default();
        name == MARKER
    })
}

/// Show a recipient why their link did not work.
///
/// Written into the document rather than drawn, so it reaches somebody whose
/// application never started. The sentence comes from the service and is set
/// as text rather than as markup, so nothing in it can be read as an element.
///
/// A caller stops after this. There is no document to open.
pub fn refuse(reason: &str) {
    if let Err(detail) = write_refusal(reason) {
        // Nothing left to show it in. The log is all that remains, and a
        // caller has already stopped.
        log::error!("a refused link could not be shown to the recipient: {detail}");
    }
}

/// The heading, the same words the service's own refusal page uses.
///
/// A recipient refused at the landing page and one refused a moment later, in
/// the app build, are the same person reading the same sentence. Two
/// different pages would read as two different faults.
const HEADING: &str = "This link does not work";

fn write_refusal(reason: &str) -> std::result::Result<(), String> {
    let window = web_sys::window().ok_or("there is no window")?;
    let document = window.document().ok_or("there is no document")?;
    document.set_title(HEADING);

    let sheet = document.create_element("div").map_err(describe)?;
    sheet
        .set_attribute(
            "style",
            "position:fixed;inset:0;z-index:2147483647;display:flex;align-items:center;justify-content:center;\
             padding:2rem;background:#ffffff;color:#1b1b1b;font:16px/1.5 system-ui,sans-serif;",
        )
        .map_err(describe)?;

    let column = document.create_element("div").map_err(describe)?;
    column.set_attribute("style", "max-width:32rem;").map_err(describe)?;

    let heading = document.create_element("h1").map_err(describe)?;
    heading.set_attribute("style", "margin:0 0 0.75rem;font-size:1.5rem;font-weight:600;").map_err(describe)?;
    heading.set_text_content(Some(HEADING));

    let sentence = document.create_element("p").map_err(describe)?;
    sentence.set_attribute("style", "margin:0;").map_err(describe)?;
    sentence.set_text_content(Some(reason));

    column.append_child(&heading).map_err(describe)?;
    column.append_child(&sentence).map_err(describe)?;
    sheet.append_child(&column).map_err(describe)?;

    // The body where there is one. A page whose body has not been parsed yet
    // still has a root element, and a refusal nobody can see is the failure
    // this whole module exists to prevent.
    let host: web_sys::Node = match document.body() {
        Some(body) => body.unchecked_into(),
        None => document.document_element().ok_or("there is nothing to put it in")?.unchecked_into(),
    };
    host.append_child(&sheet).map_err(describe)?;
    Ok(())
}

fn describe(error: wasm_bindgen::JsValue) -> String {
    error.as_string().unwrap_or_else(|| format!("{error:?}"))
}
