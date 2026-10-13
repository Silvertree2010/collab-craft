//! Sending mail, through the Nextcloud Mail app.
//!
//! [`Mail`] documents that API and how it was established.

use crate::error::{Error, Result};
use crate::http::{Http, Method};
use crate::{Account, send};

/// One of the signed-in user's mail accounts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MailAccount {
    /// The handle the outbox wants back as `accountId`.
    pub id: i64,
    /// The address this account sends from.
    pub email: String,
    /// The display name for the `From` header.
    pub name: String,
    /// The outbound server. Empty where the account has none. That is how the app describes an
    /// account that only receives.
    pub smtp_host: String,
    /// Where a sent copy is filed. Absent until the user picks a mailbox.
    pub sent_mailbox: Option<i64>,
}

impl MailAccount {
    /// Whether this account can actually send.
    ///
    /// Both conditions come off the app's own send path. It builds its SMTP transport from the
    /// outbound host. An account without one has nothing to send through. Its send chain then
    /// opens by checking the sent mailbox. The chain stops there when none is set. The message
    /// stays in the outbox and the send reports a failure.
    ///
    /// An SMTP server can still refuse a message. The chain also needs a working IMAP connection
    /// to file the sent copy. Only a send finds either of those out.
    pub fn can_send(&self) -> bool {
        !self.smtp_host.is_empty() && self.sent_mailbox.is_some()
    }
}

/// A message to hand to the outbox.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NewMessage {
    /// Recipient addresses.
    pub to: Vec<String>,
    pub subject: String,
    pub body: String,
    /// Whether `body` is HTML. Plain text otherwise.
    pub html: bool,
}

/// The Mail app, as reached by one account.
///
/// Mail is a separate app carrying its own REST API under `/apps/mail/api`. The core OCS surface
/// does not reach it. The app contributes three OCS routes of its own. All three only read. One
/// fetches a message. One fetches its source. One fetches an attachment. A send therefore goes to
/// the app's own routes. That is what this type speaks.
///
/// A send is two calls. The first puts a message in the outbox. It answers with the message id.
/// The second posts that id. The server then hands the message to SMTP. A message that goes out is
/// deleted. A finished send leaves nothing behind on the server. A message that fails stays in the
/// outbox, where the user can see it in the Mail app.
///
/// Every call here carries `OCS-APIRequest: true`. Every request this crate builds already sets
/// it. Nextcloud's CSRF guard passes any request carrying that header. That is what lets an app
/// password reach these routes with no session cookie and no request token.
///
/// The Mail app publishes no specification. Everything here was read off the deployed app itself,
/// version 5.5.17 on Nextcloud 30. `appinfo/routes.php` gave the routes and their verbs.
/// `AccountsController` and `OutboxController` gave the parameters and the answers.
/// `MailTransmission` and `Send\Chain` gave what a send actually requires. The core `Request`
/// class settled the CSRF question. No Nextcloud code is used.
pub struct Mail<'a> {
    account: &'a Account,
}

impl<'a> Mail<'a> {
    pub fn new(account: &'a Account) -> Mail<'a> {
        Mail { account }
    }

    /// Every mail account this user has, in the order the server lists them.
    ///
    /// A bare list arrives here. The accounts controller answers with the array itself. Every
    /// other call in this app wraps its answer in a `status` and `data` envelope.
    pub fn accounts<H: Http>(&self, http: &H) -> Result<Vec<MailAccount>> {
        let url = format!("{}/accounts", self.account.mail_api_root());
        let req = self.account.request(Method::Get, url).header("accept", "application/json");
        let resp = send(http, req)?;
        let body: serde_json::Value = serde_json::from_slice(&resp.body).map_err(|e| Error::malformed("mail accounts", e.to_string()))?;
        let rows = body.as_array().ok_or_else(|| Error::malformed("mail accounts", "the answer was not a list of accounts"))?;
        Ok(rows.iter().filter_map(read_account).collect())
    }

    /// Send one message as `from`. That is a [`MailAccount::id`].
    pub fn send<H: Http>(&self, http: &H, from: i64, message: &NewMessage) -> Result<()> {
        if message.to.iter().all(|address| address.trim().is_empty()) {
            return Err(Error::invalid("mail recipients", "a message needs at least one address"));
        }
        let id = self.compose(http, from, message)?;
        let url = format!("{}/outbox/{id}", self.account.mail_api_root());
        let req = self.account.request(Method::Post, url).header("accept", "application/json");
        send(http, req)?;
        Ok(())
    }

    /// Put a message in the outbox. The answer carries the id the send needs.
    fn compose<H: Http>(&self, http: &H, from: i64, message: &NewMessage) -> Result<i64> {
        let url = format!("{}/outbox", self.account.mail_api_root());
        let req = self
            .account
            .request(Method::Post, url)
            .header("accept", "application/json")
            .header("content-type", "application/json")
            .body(outbox_body(from, message).to_string().into_bytes());
        let resp = send(http, req)?;
        let body: serde_json::Value = serde_json::from_slice(&resp.body).map_err(|e| Error::malformed("outbox message", e.to_string()))?;
        // The id is the whole point of this call. An envelope without one leaves the send nothing
        // to post. That makes the id the only success test worth running.
        body.pointer("/data/id")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| Error::malformed("outbox message", "the answer carried no message id"))
    }
}

/// The body the outbox takes.
///
/// JSON here, where every other call in this crate posts a form. The recipients are a list of
/// objects. The app's typed parameters want real booleans. Form encoding mangles both.
///
/// Every key the controller declares without a default is present. One left out arrives as null.
/// It then fails a typed parameter before the handler runs. The caller sees a 500 carrying nothing
/// it can act on.
fn outbox_body(from: i64, message: &NewMessage) -> serde_json::Value {
    // Only these two reach the wire. The app builds the MIME part from them. `editorBody` feeds
    // the app's own composer. It is sent to match the other two and never reaches a recipient.
    let (plain, html) = if message.html {
        (serde_json::Value::Null, serde_json::Value::from(message.body.clone()))
    } else {
        (serde_json::Value::from(message.body.clone()), serde_json::Value::Null)
    };

    let recipients: Vec<serde_json::Value> = message
        .to
        .iter()
        .map(|address| address.trim())
        .filter(|address| !address.is_empty())
        .map(|address| {
            let mut who = serde_json::Map::new();
            who.insert("email".to_owned(), serde_json::Value::from(address));
            // The app carries a label beside each address and shows it to the recipient. The
            // address is the best label available here.
            who.insert("label".to_owned(), serde_json::Value::from(address));
            serde_json::Value::Object(who)
        })
        .collect();

    let empty = Vec::<serde_json::Value>::new();
    let mut out = serde_json::Map::new();
    out.insert("accountId".to_owned(), serde_json::Value::from(from));
    out.insert("subject".to_owned(), serde_json::Value::from(message.subject.clone()));
    out.insert("bodyPlain".to_owned(), plain);
    out.insert("bodyHtml".to_owned(), html);
    out.insert("editorBody".to_owned(), serde_json::Value::from(message.body.clone()));
    out.insert("isHtml".to_owned(), serde_json::Value::from(message.html));
    out.insert("smimeSign".to_owned(), serde_json::Value::from(false));
    out.insert("smimeEncrypt".to_owned(), serde_json::Value::from(false));
    out.insert("to".to_owned(), serde_json::Value::from(recipients));
    out.insert("cc".to_owned(), serde_json::Value::from(empty.clone()));
    out.insert("bcc".to_owned(), serde_json::Value::from(empty.clone()));
    out.insert("attachments".to_owned(), serde_json::Value::from(empty));
    // No send time. The app's own cron picks up any outbox message carrying one. This send is
    // explicit. A null here is what keeps a message from going out twice.
    out.insert("sendAt".to_owned(), serde_json::Value::Null);
    serde_json::Value::Object(out)
}

/// One account out of the app's list.
///
/// A row missing its id or its address describes nothing that can be sent from. The reader skips
/// it. Half an account would fail later with nothing to explain it.
fn read_account(row: &serde_json::Value) -> Option<MailAccount> {
    let id = row.get("accountId").or_else(|| row.get("id")).and_then(serde_json::Value::as_i64)?;
    let email = row.get("emailAddress").and_then(serde_json::Value::as_str).filter(|address| !address.is_empty())?;
    Some(MailAccount {
        id,
        email: email.to_owned(),
        name: row.get("name").and_then(serde_json::Value::as_str).filter(|n| !n.is_empty()).unwrap_or(email).to_owned(),
        // The app omits this key for an account with no outbound server. It never nulls it. An
        // absent key and an empty string therefore mean the same thing.
        smtp_host: row.get("smtpHost").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned(),
        sent_mailbox: row.get("sentMailboxId").and_then(serde_json::Value::as_i64),
    })
}
