//! Requests and answers, with no transport.
//!
//! Everything here is the same on every target. A request is a method, a URL
//! and a body. An answer is a status and bytes. Nothing opens a socket, which
//! is what lets one description of the API serve a desktop binary and a
//! WebAssembly build rather than two that drift apart.
//!
//! The split follows the one `craft-nextcloud` already uses, where the wire
//! format and the socket live in separate modules and the tests drive a canned
//! transport.

use std::collections::BTreeMap;
use std::fmt;

use serde::Deserialize;

use crate::{Error, Result};

/// An HTTP method. Four, because the service answers no others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
}

impl Method {
    /// The token that goes on the request line.
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Delete => "DELETE",
        }
    }

    /// Whether this method may carry a body.
    ///
    /// A browser refuses outright to send a GET with one, so a transport has
    /// to know before it builds the request rather than after.
    pub fn takes_a_body(self) -> bool {
        matches!(self, Method::Post | Method::Put)
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A request to send.
///
/// A builder in [`crate::req`] leaves the service's own path and query in
/// `url`. [`Target::address`] turns that into the absolute URL a transport
/// sends, and adds the session. So no builder knows where the service lives
/// and no transport knows what the routes are called.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub method: Method,
    pub url: String,
    /// Header names are lowercase, which keeps lookups and test assertions
    /// predictable.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn new(method: Method, url: impl Into<String>) -> Request {
        Request { method, url: url.into(), headers: BTreeMap::new(), body: Vec::new() }
    }

    /// Set a header. The name is lowercased.
    #[must_use]
    pub fn header(mut self, name: &str, value: impl Into<String>) -> Request {
        self.headers.insert(name.to_ascii_lowercase(), value.into());
        self
    }

    #[must_use]
    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Request {
        self.body = body.into();
        self
    }

    /// Read a header back, by any casing.
    pub fn get_header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }
}

/// An answer as the transport saw it.
///
/// A non-2xx is an answer carrying a reason, never a transport failure.
/// Treating one as an error would throw the body away and leave every refusal
/// looking identical, so a transport hands the status over untouched and the
/// decoders below read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Response {
        Response { status, body: body.into() }
    }

    /// 2xx.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// Which deployment to talk to, and what to present when asking.
///
/// The session rides two ways and both name the same record on the server. A
/// desktop app collected a bearer token through a browser and presents it
/// here. A recipient who followed a keyed link has a cookie the service set,
/// which the browser attaches on its own, so there is no token to hold. That
/// is why `token` is optional rather than required.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Target {
    base: String,
    token: Option<String>,
}

impl Target {
    /// `base` is the service root, such as `https://craft.example.com`. An
    /// empty one means the service that served this page, which is the
    /// ordinary case in a browser.
    pub fn new(base: impl Into<String>) -> Target {
        Target { base: base.into().trim_end_matches('/').to_owned(), token: None }
    }

    #[must_use]
    pub fn with_token(mut self, token: impl Into<String>) -> Target {
        self.token = Some(token.into());
        self
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    pub fn is_signed_in(&self) -> bool {
        self.token.is_some()
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// The request as a transport should send it.
    #[must_use]
    pub fn address(&self, mut req: Request) -> Request {
        req.url = format!("{}{}", self.base, req.url);
        match &self.token {
            Some(token) => req.header("authorization", format!("Bearer {token}")),
            None => req,
        }
    }
}

/// One call: the request to send, and how to read what comes back.
///
/// Pairing the two is the point of the type. A caller cannot send one route's
/// request and read the answer as another route's type, because the answer's
/// type arrives with the request and neither transport chooses it.
pub struct Call<T> {
    pub(crate) request: Request,
    pub(crate) decode: fn(Response) -> Result<T>,
}

impl<T> Call<T> {
    /// A call whose answer is JSON.
    pub(crate) fn json(request: Request) -> Call<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        Call { request, decode: json }
    }

    /// What will be sent, before [`Target::address`] has seen it.
    pub fn request(&self) -> &Request {
        &self.request
    }

    /// Read one answer.
    ///
    /// For a host driving these over an HTTP stack of its own, which is the
    /// whole reason the request and the answer are separable.
    pub fn answer(self, response: Response) -> Result<T> {
        (self.decode)(response)
    }
}

impl Call<Vec<u8>> {
    /// A call whose answer is a document rather than a message.
    pub(crate) fn raw(request: Request) -> Call<Vec<u8>> {
        Call { request, decode: bytes }
    }
}

impl Call<()> {
    /// A call that answers with nothing, such as a withdrawal.
    pub(crate) fn nothing(request: Request) -> Call<()> {
        Call { request, decode: nothing }
    }
}

impl Call<String> {
    /// A call answering `{"token": "…"}`, which is a session rather than a
    /// message worth a type of its own.
    pub(crate) fn collected(request: Request) -> Call<String> {
        Call { request, decode: collected }
    }
}

impl<T> fmt::Debug for Call<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Call").field("request", &self.request).finish()
    }
}

/// What a failed call looks like on the wire.
///
/// The shape is the service's and it is stable, so a client acts on `code`
/// without reading `detail`, which is for a human.
#[derive(Deserialize)]
struct ApiError {
    code: String,
    detail: String,
}

/// A 2xx body, or the error the status and body describe.
pub fn bytes(response: Response) -> Result<Vec<u8>> {
    if response.is_success() {
        return Ok(response.body);
    }
    Err(status(response.status, &response.body))
}

/// A 2xx body, read as JSON.
pub fn json<T: for<'de> Deserialize<'de>>(response: Response) -> Result<T> {
    let body = bytes(response)?;
    serde_json::from_slice(&body).map_err(|e| Error::Malformed(e.to_string()))
}

/// Success, with nothing to read.
pub fn nothing(response: Response) -> Result<()> {
    bytes(response).map(|_| ())
}

/// The session out of `{"token": "…"}`.
fn collected(response: Response) -> Result<String> {
    #[derive(Deserialize)]
    struct Collected {
        token: String,
    }
    json::<Collected>(response).map(|c| c.token)
}

/// Turn a status and body into the error a caller can act on.
pub fn status(code: u16, body: &[u8]) -> Error {
    if let Ok(parsed) = serde_json::from_slice::<ApiError>(body) {
        return match parsed.code.as_str() {
            "unauthenticated" => Error::Unauthenticated,
            "provider_not_linked" => Error::NotLinked,
            _ => Error::Refused { code: parsed.code, detail: parsed.detail },
        };
    }
    match code {
        401 => Error::Unauthenticated,
        428 => Error::NotLinked,
        _ => Error::Refused { code: code.to_string(), detail: String::new() },
    }
}

/// Treat "the user has not finished in the browser yet" as a state.
///
/// Both polls need this. It is not a failure, so a caller loops on `None`
/// instead of giving up on the sign-in.
pub fn pending<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(Error::Refused { code, .. }) if code == "authorization_pending" => Ok(None),
        Err(e) => Err(e),
    }
}

/// Percent-encode a query value. Stricter than needed and never wrong.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => out.push(*b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// A selector matching one named `meta` element.
///
/// The browser client reads its configuration from the page this way. Here
/// rather than beside the fetch, so a test on any target can show that a name
/// carrying a quote cannot end the attribute and start something else.
pub fn meta_selector(name: &str) -> String {
    format!("meta[name=\"{}\"]", name.replace('\\', "\\\\").replace('"', "\\\""))
}
