//! Tests for the two parts that must be right: token verification and
//! arbitration. Neither needs a socket or a Valkey.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::digest::KeyInit;
use hmac::{Hmac, Mac};
use serde_json::json;
use sha2::Sha256;

use crate::room::{Peer, Refusal, Room, Submission};
use crate::token::{self, PERMISSION_READ, PERMISSION_UPDATE, TokenError};

const SECRET: &[u8] = b"a-shared-secret-at-least-32-chars-long";
const NOW: i64 = 1_760_000_000;

/// Mint a token the way the PHP side does, so the test exercises the real
/// format rather than a convenient one.
fn mint(payload: serde_json::Value, secret: &[u8]) -> String {
    let encoded = URL_SAFE_NO_PAD.encode(payload.to_string());
    let mut mac = <Hmac<Sha256> as KeyInit>::new_from_slice(secret).expect("key");
    mac.update(encoded.as_bytes());
    let sig = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{encoded}.{sig}")
}

fn claims_json(perm: u32, exp: i64) -> serde_json::Value {
    json!({ "fid": 1234, "uid": "alice", "dsp": "Alice", "perm": perm, "exp": exp, "pid": "peer-a" })
}

// ---- token ----------------------------------------------------------------

#[test]
fn a_well_formed_token_verifies() {
    let t = mint(claims_json(PERMISSION_READ | PERMISSION_UPDATE, NOW + 300), SECRET);
    let c = token::verify(&t, SECRET, NOW).expect("verifies");
    assert_eq!(c.fid, 1234);
    assert_eq!(c.uid, "alice");
    assert_eq!(c.dsp, "Alice");
    assert_eq!(c.pid, "peer-a");
    assert!(c.may_edit());
    assert_eq!(c.room(), "doc:1234");
}

#[test]
fn a_read_only_token_verifies_but_cannot_edit() {
    let t = mint(claims_json(PERMISSION_READ, NOW + 300), SECRET);
    let c = token::verify(&t, SECRET, NOW).expect("a reader may still join");
    assert!(!c.may_edit());
}

#[test]
fn a_token_with_no_read_is_refused() {
    let t = mint(claims_json(0, NOW + 300), SECRET);
    assert_eq!(token::verify(&t, SECRET, NOW), Err(TokenError::NoRead));
}

#[test]
fn a_tampered_payload_fails_the_signature() {
    let t = mint(claims_json(PERMISSION_READ, NOW + 300), SECRET);
    let (payload, sig) = t.split_once('.').expect("shape");
    // Re-encode the payload with elevated permissions, keeping the old signature.
    let forged = URL_SAFE_NO_PAD.encode(claims_json(PERMISSION_READ | PERMISSION_UPDATE, NOW + 300).to_string());
    assert_ne!(forged, payload);
    assert_eq!(token::verify(&format!("{forged}.{sig}"), SECRET, NOW), Err(TokenError::Signature));
}

#[test]
fn another_secret_fails() {
    let t = mint(claims_json(PERMISSION_READ, NOW + 300), b"a-different-secret-also-long-enough-x");
    assert_eq!(token::verify(&t, SECRET, NOW), Err(TokenError::Signature));
}

#[test]
fn an_expired_token_is_refused() {
    let t = mint(claims_json(PERMISSION_READ, NOW - 1), SECRET);
    match token::verify(&t, SECRET, NOW) {
        Err(TokenError::Expired { exp, now }) => {
            assert_eq!(exp, NOW - 1);
            assert_eq!(now, NOW);
        }
        other => panic!("expected Expired, got {other:?}"),
    }
}

#[test]
fn expiry_is_checked_after_the_signature() {
    // An expired token signed with the wrong key reports Signature, not
    // Expired. A caller must not learn anything about a token it did not sign.
    let t = mint(claims_json(PERMISSION_READ, NOW - 1), b"wrong-secret-but-long-enough-to-use-x");
    assert_eq!(token::verify(&t, SECRET, NOW), Err(TokenError::Signature));
}

#[test]
fn malformed_tokens_are_refused_without_panicking() {
    for bad in ["", ".", "a.", ".b", "no-dot", "!!!.!!!", "a.b.c"] {
        assert!(token::verify(bad, SECRET, NOW).is_err(), "{bad:?} must be refused");
    }
}

// ---- arbitration ----------------------------------------------------------

fn peer(id: &str, may_edit: bool) -> Peer {
    Peer { peer: id.to_owned(), display: id.to_uppercase(), may_edit }
}

/// A submission from `peer`, editing `scope` with `command`.
fn sub(peer: &str, may_edit: bool, command: &str, params: serde_json::Value, scope: Option<&str>) -> Submission {
    Submission { peer_id: peer.to_owned(), may_edit, command: command.to_owned(), params, scope: scope.map(str::to_owned), local: None }
}

fn room_with(ids: &[(&str, bool)]) -> Room {
    let mut r = Room::default();
    for (id, may_edit) in ids {
        r.join(peer(id, *may_edit));
    }
    r
}

#[test]
fn a_reader_cannot_submit() {
    let mut r = room_with(&[("bob", false)]);
    assert_eq!(r.submit(1, sub("bob", false, "cell.set", json!({}), None)), Err(Refusal::ReadOnly));
}

#[test]
fn an_unlocked_scope_accepts_an_edit() {
    let mut r = room_with(&[("alice", true)]);
    let op = r.submit(1, Submission { local: Some(7), ..sub("alice", true, "cell.set", json!({"cell": "B2"}), Some("B2")) }).expect("ok");
    assert_eq!(op.seq, 1);
    assert_eq!(op.origin, "alice");
    assert_eq!(op.local, Some(7));
}

#[test]
fn a_scope_another_peer_holds_refuses() {
    let mut r = room_with(&[("alice", true), ("bob", true)]);
    r.record_lock("bob", "B2".into());
    let out = r.submit(1, sub("alice", true, "cell.set", json!({}), Some("B2")));
    assert_eq!(out, Err(Refusal::Locked { holder: "bob".into() }));
}

#[test]
fn a_scope_this_peer_holds_proceeds() {
    let mut r = room_with(&[("alice", true)]);
    r.record_lock("alice", "B2".into());
    assert!(r.submit(1, sub("alice", true, "cell.set", json!({}), Some("B2"))).is_ok());
}

#[test]
fn an_edit_with_no_scope_is_never_blocked() {
    let mut r = room_with(&[("alice", true), ("bob", true)]);
    r.record_lock("bob", "B2".into());
    assert!(r.submit(1, sub("alice", true, "doc.title", json!({}), None)).is_ok());
}

#[test]
fn leaving_drops_every_scope_the_peer_held() {
    let mut r = room_with(&[("alice", true), ("bob", true)]);
    r.record_lock("bob", "B2".into());
    r.record_lock("bob", "C3".into());
    r.record_lock("alice", "A1".into());

    let dropped = r.leave("bob");
    let mut dropped = dropped;
    dropped.sort();
    assert_eq!(dropped, vec!["B2".to_owned(), "C3".to_owned()]);
    assert_eq!(r.holder_of("A1"), Some("alice"), "alice keeps hers");
    assert_eq!(r.holder_of("B2"), None, "bob's went with him");
    assert_eq!(r.peers().len(), 1);
}

#[test]
fn unlock_only_releases_your_own() {
    let mut r = room_with(&[("alice", true), ("bob", true)]);
    r.record_lock("bob", "B2".into());
    assert!(!r.unlock("alice", "B2"), "alice must not release bob's lock");
    assert!(r.unlock("bob", "B2"));
    assert!(!r.unlock("bob", "B2"), "a second release changes nothing");
}

#[test]
fn rejoining_replaces_the_earlier_connection() {
    let mut r = room_with(&[("alice", true)]);
    r.join(peer("alice", true));
    assert_eq!(r.peers().len(), 1, "a reconnect must not duplicate the peer");
}

#[test]
fn the_room_tracks_the_highest_sequence_it_has_seen() {
    let mut r = room_with(&[("alice", true)]);
    r.submit(5, sub("alice", true, "a", json!({}), None)).expect("ok");
    assert_eq!(r.seq(), 5, "the sequence comes from the bus, so it may not start at 1");
    r.submit(6, sub("alice", true, "b", json!({}), None)).expect("ok");
    assert_eq!(r.seq(), 6);
}

#[test]
fn a_resync_within_history_replays() {
    let mut r = room_with(&[("alice", true)]);
    for seq in 1..=5 {
        r.submit(seq, sub("alice", true, &format!("c{seq}"), json!({}), None)).expect("ok");
    }
    let ops = r.since(2).expect("history reaches back");
    assert_eq!(ops.len(), 3);
    assert_eq!(ops.iter().map(|o| o.seq).collect::<Vec<_>>(), vec![3, 4, 5]);
}

#[test]
fn a_resync_already_current_replays_nothing() {
    let mut r = room_with(&[("alice", true)]);
    r.submit(1, sub("alice", true, "a", json!({}), None)).expect("ok");
    assert_eq!(r.since(1).expect("current"), Vec::new());
    assert_eq!(r.since(99).expect("ahead of us"), Vec::new());
}

#[test]
fn a_resync_past_the_history_asks_for_a_snapshot() {
    let mut r = room_with(&[("alice", true)]);
    // Fill past the cap so the oldest ops are evicted.
    for seq in 1..=(crate::room::HISTORY as u64 + 10) {
        r.submit(seq, sub("alice", true, "c", json!({}), None)).expect("ok");
    }
    assert!(r.since(0).is_none(), "a gap the log cannot fill must answer None");
    assert!(r.since(crate::room::HISTORY as u64 + 5).is_some(), "a recent gap still replays");
}

#[test]
fn an_empty_room_is_reported_empty() {
    let mut r = room_with(&[("alice", true)]);
    assert!(!r.is_empty());
    r.leave("alice");
    assert!(r.is_empty());
}

#[test]
fn the_mirror_reports_who_holds_a_scope() {
    let mut r = room_with(&[("alice", true), ("bob", true)]);
    assert_eq!(r.holder_of("B2"), None, "an unclaimed scope has no holder");
    r.record_lock("bob", "B2".into());
    assert_eq!(r.holder_of("B2"), Some("bob"));
    r.record_holder("C3".into(), "carol".into());
    assert_eq!(r.holder_of("C3"), Some("carol"), "a holder learned from a refusal is recorded too");
}
