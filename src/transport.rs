//! The HTTP client the providers use.
//!
//! `craft-cloud` and `craft-nextcloud` open no sockets, so something has to.
//! Keeping it here means a provider crate stays testable with no server and
//! the service decides timeouts, redirects and TLS in one place.

use std::collections::BTreeMap;
use std::time::Duration;

use craft_nextcloud::{Http, Request, Response};

pub struct ReqwestHttp {
    client: reqwest::blocking::Client,
}

impl Default for ReqwestHttp {
    fn default() -> ReqwestHttp {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(45))
            // The contract is to hand back what came off the wire. Following a
            // redirect would turn a 302 into something the parsers never see,
            // and WebDAV's 207 is a real answer rather than a failure.
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("CollabCraft")
            .build();
        ReqwestHttp { client: client.unwrap_or_default() }
    }
}

impl Http for ReqwestHttp {
    type Error = String;

    fn send(&self, req: Request) -> Result<Response, String> {
        let method = reqwest::Method::from_bytes(req.method.as_str().as_bytes()).map_err(|e| e.to_string())?;
        let mut call = self.client.request(method, &req.url);
        for (name, value) in &req.headers {
            call = call.header(name, value);
        }
        if !req.body.is_empty() {
            call = call.body(req.body);
        }
        let raw = call.send().map_err(|e| e.to_string())?;
        let status = raw.status().as_u16();
        let mut headers = BTreeMap::new();
        for (name, value) in raw.headers() {
            if let Ok(v) = value.to_str() {
                headers.insert(name.as_str().to_ascii_lowercase(), v.to_owned());
            }
        }
        let body = raw.bytes().map_err(|e| e.to_string())?.to_vec();
        Ok(Response { status, headers, body })
    }
}
