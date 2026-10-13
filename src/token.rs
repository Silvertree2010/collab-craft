//! The join token: signed by the CollabCraft API, checked by the relay.
//!
//! The relay makes no access decisions. The API asked the provider whether
//! this user may open this document, signed the answer, and this module checks
//! the signature and the expiry. That is the whole trust boundary, so it is
//! deliberately small and makes no network call.
//!
//! Format is `payload.signature`, both base64url without padding, the
//! signature being HMAC-SHA256 over the payload bytes. Not a JWT: six fields
//! with no negotiation buy nothing from the extra machinery, and a JWT library
//! on each side is two dependencies for one HMAC.
//!
//! Nothing here names a provider. The room is an opaque string the API chose,
//! so a relay serving a Nextcloud deployment and one serving something else
//! run the same code.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
// new_from_slice lives on digest::KeyInit in hmac 0.13, not on Mac.
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};

/// Permission bits. The numbering follows the OCS Share API because one
/// provider already speaks it, and nothing is lost by matching.
pub const PERMISSION_READ: u32 = 1;
pub const PERMISSION_UPDATE: u32 = 2;

/// How long a freshly minted token is good for.
///
/// Short, because a client asks for one per document and getting another costs
/// a round trip it was making anyway. Long enough that opening a large file
/// over a slow link does not expire the token before the socket opens.
pub const LIFETIME_SECS: i64 = 15 * 60;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TokenError {
    #[error("token is not payload.signature")]
    Shape,
    #[error("token is not base64url")]
    Encoding,
    #[error("signature does not match")]
    Signature,
    #[error("payload is not the expected JSON: {0}")]
    Payload(String),
    #[error("token expired at {exp}, now {now}")]
    Expired { exp: i64, now: i64 },
    #[error("token grants no read permission")]
    NoRead,
    #[error("room {0:?} is not usable as a key")]
    Room(String),
}

/// What the API asserted. Field names are the short ones on the wire.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Claims {
    /// The room. Opaque here: whatever the provider calls this document, as
    /// long as it survives a rename.
    pub rm: String,
    /// Who this is, as the identity provider names them.
    pub uid: String,
    /// Display name for the roster. Set here rather than by the client, so a
    /// peer cannot present itself as somebody else.
    pub dsp: String,
    /// Permission bits.
    pub perm: u32,
    /// Unix expiry.
    pub exp: i64,
    /// Peer id for this one connection.
    pub pid: String,
}

impl Claims {
    /// Whether this peer may submit edits.
    ///
    /// A peer without it still joins and watches, which is what makes a
    /// read-only share usable rather than merely visible.
    pub fn may_edit(&self) -> bool {
        self.perm & PERMISSION_UPDATE != 0
    }

    /// The room name, which is also the Valkey key suffix.
    pub fn room(&self) -> String {
        format!("doc:{}", self.rm)
    }
}

/// Whether a room string can be used as part of a key and a Socket.IO room.
///
/// Rejected rather than escaped. A room arriving with whitespace or a control
/// character is a bug upstream, and quietly rewriting it would put two peers
/// in rooms they both believe are the same one.
fn room_is_sane(room: &str) -> bool {
    !room.is_empty() && room.len() <= 256 && room.chars().all(|c| !c.is_whitespace() && !c.is_control())
}

/// Sign a token for one room.
///
/// `now` is passed in rather than read from the clock, so this is testable and
/// so a caller that already knows the time does not read it twice.
pub fn sign_at(secret: &[u8], room: &str, user: &str, display: &str, may_edit: bool, peer: &str, now: i64) -> Result<String, TokenError> {
    if !room_is_sane(room) {
        return Err(TokenError::Room(room.to_owned()));
    }
    let claims = Claims {
        rm: room.to_owned(),
        uid: user.to_owned(),
        dsp: display.to_owned(),
        perm: if may_edit { PERMISSION_READ | PERMISSION_UPDATE } else { PERMISSION_READ },
        exp: now + LIFETIME_SECS,
        pid: peer.to_owned(),
    };
    let payload = serde_json::to_vec(&claims).map_err(|e| TokenError::Payload(e.to_string()))?;
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload);

    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret).map_err(|_| TokenError::Signature)?;
    mac.update(payload_b64.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    Ok(format!("{payload_b64}.{sig}"))
}

use sha2::Sha256;

/// Check a token against the shared secret at time `now`.
///
/// The comparison is constant time, because `Mac::verify_slice` is. A byte
/// comparison here would leak the signature a byte at a time to anyone able to
/// retry, which a join endpoint certainly is.
pub fn verify(token: &str, secret: &[u8], now: i64) -> Result<Claims, TokenError> {
    let (payload_b64, sig_b64) = token.split_once('.').ok_or(TokenError::Shape)?;
    if payload_b64.is_empty() || sig_b64.is_empty() {
        return Err(TokenError::Shape);
    }

    let sig = URL_SAFE_NO_PAD.decode(sig_b64).map_err(|_| TokenError::Encoding)?;

    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret).map_err(|_| TokenError::Signature)?;
    mac.update(payload_b64.as_bytes());
    mac.verify_slice(&sig).map_err(|_| TokenError::Signature)?;

    let raw = URL_SAFE_NO_PAD.decode(payload_b64).map_err(|_| TokenError::Encoding)?;
    let claims: Claims = serde_json::from_slice(&raw).map_err(|e| TokenError::Payload(e.to_string()))?;

    // Expiry after signature, so an unsigned token never reaches the clock and
    // a caller learns nothing from which check failed first.
    if claims.exp <= now {
        return Err(TokenError::Expired { exp: claims.exp, now });
    }
    if claims.perm & PERMISSION_READ == 0 {
        return Err(TokenError::NoRead);
    }
    if !room_is_sane(&claims.rm) {
        return Err(TokenError::Room(claims.rm));
    }

    Ok(claims)
}
