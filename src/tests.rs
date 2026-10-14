//! Tests for the parts that must be right: token verification, arbitration
//! and the keyed links a recipient with no account follows. None of them
//! needs a socket or a Valkey.

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
    json!({ "rm": "1234", "uid": "alice", "dsp": "Alice", "perm": perm, "exp": exp, "pid": "peer-a" })
}

// ---- token ----------------------------------------------------------------

#[test]
fn a_well_formed_token_verifies() {
    let t = mint(claims_json(PERMISSION_READ | PERMISSION_UPDATE, NOW + 300), SECRET);
    let c = token::verify(&t, SECRET, NOW).expect("verifies");
    assert_eq!(c.rm, "1234");
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

/// Keyed links, for a recipient who holds no account anywhere.
///
/// Its own module because `Refusal` means one thing in arbitration and
/// another here, and importing both into one namespace would need an alias at
/// every call site.
///
/// Every decision under test is a pure function over a record and a clock.
/// Expiry, spending, withdrawal and the off switch are the cases that matter,
/// and none of them needs a datastore or a wall clock to exercise.
mod keyed_links {

    use serde_json::json;

    use craft_cloud::{Permission, Share, ShareTarget};

    use crate::keyed::{self, Key, Link, Policy, Refusal};
    use crate::session::{Guest, Session};
    use crate::state::{App, Permissions, ServiceCapability};

    use super::{NOW, SECRET};

    /// A policy with keyed links on, as a deployment that allows them has.
    fn policy_on() -> Policy {
        Policy::decide(None, "https://craft.example.com", None, None)
    }

    /// A service, as far as anything below needs one.
    ///
    /// Nothing here opens a socket. The store holds a client rather than a
    /// connection, and every assertion is about what this deployment reports or
    /// permits rather than about what it stores.
    fn service(keyed: Policy) -> App {
        App {
            secret: SECRET.to_vec(),
            relay_url: "https://craft.example.com".to_owned(),
            provider_base: "https://cloud.example.com".to_owned(),
            permissions: Permissions::from_env(),
            store: crate::store::Store::new("redis://127.0.0.1:6379/15", "collabcraft:test").expect("a client, not a connection"),
            oidc: None,
            http: reqwest::Client::new(),
            web: tower_http::services::ServeDir::new("."),
            hosted: vec!["gridcraft".to_owned(), "wordcraft".to_owned()],
            keyed,
        }
    }

    fn link_for(key: &Key, exp: i64, max_uses: u32, permission: Permission) -> Link {
        Link {
            verifier: key.verifier(SECRET).expect("a verifier"),
            room: "room-7".to_owned(),
            doc: "/Documents/Budget.xlsx".to_owned(),
            app: "gridcraft".to_owned(),
            permission,
            recipient: "outsider@example.org".to_owned(),
            owner: "alice".to_owned(),
            share: "share-99".to_owned(),
            exp,
            max_uses,
        }
    }

    /// The share the provider still reports for that document.
    fn standing_share(permission: Permission) -> Share {
        Share {
            id: "share-99".to_owned(),
            target: ShareTarget::Email("outsider@example.org".to_owned()),
            display: None,
            permission,
            url: None,
            note: None,
            created: None,
        }
    }

    #[test]
    fn a_minted_key_round_trips_through_a_url() {
        let key = Key::mint();
        let wire = key.wire();
        assert!(!wire.contains('/'), "a key has to survive a path segment");
        let read_back = Key::parse(&wire).expect("parses");
        assert_eq!(read_back, key);
        assert_eq!(policy_on().url(&key), format!("https://craft.example.com/invite/{wire}"));
    }

    #[test]
    fn two_minted_keys_differ() {
        let a = Key::mint();
        let b = Key::mint();
        assert_ne!(a.handle, b.handle);
        assert_ne!(a.wire(), b.wire());
    }

    #[test]
    fn a_key_works_once_and_then_does_not() {
        let key = Key::mint();
        let link = link_for(&key, NOW + 3600, 1, Permission::Edit);

        // Nothing spent yet, so the key opens the document.
        assert_eq!(keyed::redeem(SECRET, &link, &key, 0, NOW), Ok(Permission::Edit));

        // One use taken is the whole allowance.
        assert_eq!(
            keyed::redeem(SECRET, &link, &key, 1, NOW),
            Err(Refusal::Spent { spent: 1, allowed: 1 }),
            "a single-use key must not open the document twice"
        );
    }

    #[test]
    fn a_key_allowed_more_uses_spends_each_one() {
        let key = Key::mint();
        let link = link_for(&key, NOW + 3600, 3, Permission::View);
        for spent in 0..3 {
            assert_eq!(keyed::redeem(SECRET, &link, &key, spent, NOW), Ok(Permission::View));
        }
        assert_eq!(keyed::redeem(SECRET, &link, &key, 3, NOW), Err(Refusal::Spent { spent: 3, allowed: 3 }));
    }

    #[test]
    fn a_key_past_its_expiry_is_refused() {
        let key = Key::mint();
        let link = link_for(&key, NOW - 1, 1, Permission::Edit);
        assert_eq!(keyed::redeem(SECRET, &link, &key, 0, NOW), Err(Refusal::Expired { exp: NOW - 1, now: NOW }));
    }

    #[test]
    fn expiry_lands_exactly_on_the_second_it_names() {
        let key = Key::mint();
        let link = link_for(&key, NOW, 1, Permission::Edit);
        assert!(keyed::redeem(SECRET, &link, &key, 0, NOW).is_err(), "the expiry second is already past");
        assert!(keyed::redeem(SECRET, &link, &key, 0, NOW - 1).is_ok(), "the second before it is not");
    }

    #[test]
    fn another_key_does_not_open_the_link() {
        let key = Key::mint();
        let link = link_for(&key, NOW + 3600, 1, Permission::Edit);
        let other = Key::mint();
        assert_eq!(keyed::redeem(SECRET, &link, &other, 0, NOW), Err(Refusal::Mismatch));
    }

    #[test]
    fn the_verifier_is_checked_before_anything_else() {
        // An expired, exhausted record presented with the wrong key reports a
        // mismatch. Somebody holding no key must not learn that such a record
        // exists, when it expired or how often it has been opened.
        let key = Key::mint();
        let link = link_for(&key, NOW - 10_000, 1, Permission::Edit);
        let other = Key::mint();
        assert_eq!(keyed::redeem(SECRET, &link, &other, 99, NOW), Err(Refusal::Mismatch));
    }

    #[test]
    fn a_verifier_made_under_another_secret_does_not_open_the_link() {
        let key = Key::mint();
        let mut link = link_for(&key, NOW + 3600, 1, Permission::Edit);
        link.verifier = key.verifier(b"a-different-secret-also-long-enough-x").expect("a verifier");
        assert_eq!(keyed::redeem(SECRET, &link, &key, 0, NOW), Err(Refusal::Mismatch));
    }

    #[test]
    fn malformed_keys_are_refused_without_a_lookup() {
        for bad in ["", ".", "no-dot", "a.b", "!!!.!!!", "a.b.c", "AAAAAAAAAAAAAAAAAAAAAA"] {
            assert_eq!(Key::parse(bad), Err(Refusal::Shape), "{bad:?} must be refused");
        }
        // The right shape with the wrong lengths is still not a key.
        let key = Key::mint();
        assert_eq!(Key::parse(&format!("{}.{}", key.handle, "c2hvcnQ")), Err(Refusal::Shape));
    }

    #[test]
    fn the_stored_record_carries_no_key_material() {
        let key = Key::mint();
        let link = link_for(&key, NOW + 3600, 1, Permission::Edit);
        let stored = serde_json::to_string(&link).expect("serialises");
        assert!(!stored.contains(&key.wire()), "the record must not hold the key");
        // What it holds instead is an HMAC under the service secret. A copy of the
        // keyspace therefore opens no document without this process.
        let verifier = key.verifier(SECRET).expect("a verifier");
        assert!(stored.contains(&verifier));
        assert_ne!(verifier, key.wire());
    }

    #[test]
    fn a_withdrawn_share_withdraws_the_key() {
        let key = Key::mint();
        let link = link_for(&key, NOW + 3600, 1, Permission::Edit);
        // The provider reports no share for that document any more.
        assert_eq!(keyed::regrant(&link, "room-7", &[]), Err(Refusal::Withdrawn));
    }

    #[test]
    fn another_share_on_the_same_document_does_not_stand_in() {
        let key = Key::mint();
        let link = link_for(&key, NOW + 3600, 1, Permission::Edit);
        let someone_else = Share { id: "share-100".to_owned(), ..standing_share(Permission::Edit) };
        assert_eq!(
            keyed::regrant(&link, "room-7", &[someone_else]),
            Err(Refusal::Withdrawn),
            "the key names one share and only that one may keep it alive"
        );
    }

    #[test]
    fn a_key_for_a_room_the_share_no_longer_grants_is_refused() {
        let key = Key::mint();
        let link = link_for(&key, NOW + 3600, 1, Permission::Edit);
        // The share stands, but the document the provider reports is a different
        // room now. A key must not follow a document it was never minted for.
        assert_eq!(keyed::regrant(&link, "room-8", &[standing_share(Permission::Edit)]), Err(Refusal::Moved));
    }

    #[test]
    fn the_grant_follows_the_share_down() {
        let key = Key::mint();
        let link = link_for(&key, NOW + 3600, 1, Permission::Edit);
        assert_eq!(
            keyed::regrant(&link, "room-7", &[standing_share(Permission::View)]),
            Ok(Permission::View),
            "a share downgraded to reading downgrades the recipient"
        );
    }

    #[test]
    fn the_grant_never_rises_above_what_was_minted() {
        let key = Key::mint();
        let link = link_for(&key, NOW + 3600, 1, Permission::View);
        assert_eq!(
            keyed::regrant(&link, "room-7", &[standing_share(Permission::Edit)]),
            Ok(Permission::View),
            "a key minted for reading stays a key for reading"
        );
    }

    #[test]
    fn a_standing_share_keeps_the_key_working() {
        let key = Key::mint();
        let link = link_for(&key, NOW + 3600, 1, Permission::Edit);
        assert_eq!(keyed::regrant(&link, "room-7", &[standing_share(Permission::Edit)]), Ok(Permission::Edit));
    }

    #[test]
    fn every_refusal_tells_a_recipient_the_same_thing() {
        // Which check failed says something about the record, and anybody can
        // follow a link. Only the deployment-wide refusal reads differently,
        // because that one is not about any particular link.
        let refusals = [
            Refusal::Shape,
            Refusal::Unknown,
            Refusal::Mismatch,
            Refusal::Expired { exp: NOW, now: NOW },
            Refusal::Spent { spent: 1, allowed: 1 },
            Refusal::Withdrawn,
            Refusal::Moved,
        ];
        let first = refusals[0].public();
        for refusal in &refusals {
            assert_eq!(refusal.public(), first, "{refusal} must not be distinguishable");
        }
        assert_ne!(Refusal::Disabled.public(), first);
    }

    // ---- the off switch -------------------------------------------------------

    #[test]
    fn a_deployment_can_switch_keyed_links_off() {
        for off in ["off", "OFF", "0", "false", "no", " off "] {
            let policy = Policy::decide(Some(off), "https://craft.example.com", None, None);
            assert!(!policy.enabled(), "{off:?} must turn keyed links off");
            assert_eq!(service(policy).service_capabilities(), Vec::new(), "with them off the capability is reported absent, so no button appears");
        }
    }

    #[test]
    fn keyed_links_are_on_unless_a_deployment_says_otherwise() {
        for on in [None, Some("on"), Some("true"), Some("1"), Some("")] {
            let policy = Policy::decide(on, "https://craft.example.com", None, None);
            assert!(policy.enabled(), "{on:?} leaves keyed links on");
            assert_eq!(service(policy).service_capabilities(), vec![ServiceCapability::KeyedLinks]);
        }
    }

    #[test]
    fn a_deployment_that_does_not_know_its_own_address_mints_nothing() {
        // A link has to point somewhere. Reporting the capability absent is the
        // honest answer, and the alternative is a button that produces a link to
        // nowhere.
        assert!(!Policy::decide(None, "", None, None).enabled());
        assert!(!Policy::decide(None, "   ", None, None).enabled());
        assert_eq!(service(Policy::decide(None, "", None, None)).service_capabilities(), Vec::new());
    }

    #[test]
    fn a_trailing_slash_on_the_public_address_does_not_double_up() {
        let policy = Policy::decide(None, "https://craft.example.com/", None, None);
        let key = Key::mint();
        assert_eq!(policy.url(&key), format!("https://craft.example.com/invite/{}", key.wire()));
    }

    #[test]
    fn the_lifetime_and_the_use_count_are_clamped() {
        let huge = Policy::decide(None, "https://craft.example.com", Some("99999"), Some("9999"));
        assert_eq!(huge.ttl_secs, keyed::MAX_TTL_HOURS * 3600);
        assert_eq!(huge.max_uses, keyed::MAX_USES);

        let zero = Policy::decide(None, "https://craft.example.com", Some("0"), Some("0"));
        assert_eq!(zero.ttl_secs, 3600, "an hour is the shortest a key may last");
        assert_eq!(zero.max_uses, 1, "a key nobody may spend is not a key");

        let nonsense = Policy::decide(None, "https://craft.example.com", Some("soon"), Some("lots"));
        assert_eq!(nonsense.ttl_secs, keyed::DEFAULT_TTL_HOURS * 3600);
        assert_eq!(nonsense.max_uses, keyed::DEFAULT_USES);
    }

    // ---- what a recipient is entitled to --------------------------------------

    fn guest_session() -> Session {
        Session {
            user: Guest::user_id("handle-1"),
            display: "outsider@example.org".to_owned(),
            email: Some("outsider@example.org".to_owned()),
            groups: Vec::new(),
            guest: Some(Guest {
                link: "handle-1".to_owned(),
                room: "room-7".to_owned(),
                doc: "/Documents/Budget.xlsx".to_owned(),
                app: "gridcraft".to_owned(),
                permission: Permission::Edit,
            }),
        }
    }

    #[test]
    fn a_recipient_may_open_one_app_and_no_other() {
        let permissions = Permissions::from_env();
        let session = guest_session();
        assert_eq!(permissions.apps_for_session(&session), vec!["gridcraft".to_owned()]);
        assert!(permissions.may_open(&session, "gridcraft"));
        for other in ["wordcraft", "deckcraft", "pdfcraft", "cadcraft"] {
            assert!(!permissions.may_open(&session, other), "a recipient must not reach {other}");
        }
    }

    #[test]
    fn a_recipient_carries_no_group() {
        // Entitlement here is granted by group membership. A recipient holding a
        // group would inherit whatever that group may open.
        let session = guest_session();
        assert!(session.groups.is_empty());
        assert!(session.is_guest());
    }

    #[tokio::test]
    async fn a_recipient_is_never_given_a_provider() {
        // The sharer's credential reads the document. A recipient acting as an
        // account holder is the one thing none of this may allow, so the lookup
        // refuses before it reaches the store.
        let app = service(policy_on());
        assert!(app.provider_for(&guest_session()).await.is_none());
    }

    #[test]
    fn a_session_written_before_keyed_links_existed_still_loads() {
        let old = json!({ "user": "alice", "display": "Alice", "email": null, "groups": ["editors"] });
        let session: Session = serde_json::from_value(old).expect("an older record still reads");
        assert!(!session.is_guest());
        assert!(session.guest.is_none());
    }

    // ---- the invitation -------------------------------------------------------

    #[test]
    fn the_invitation_names_no_provider_and_carries_no_credential() {
        let message = keyed::invitation("outsider@example.org", "Alice", "Budget.xlsx", "https://craft.example.com/invite/handle.secret");
        assert_eq!(message.to, vec!["outsider@example.org".to_owned()]);
        assert!(!message.html, "plain text, so nothing in it can be dressed up as something else");
        assert!(message.subject.contains("Alice"));
        assert!(message.subject.contains("Budget.xlsx"));
        assert!(message.body.contains("https://craft.example.com/invite/handle.secret"));
        let haystack = format!("{} {}", message.subject.to_lowercase(), message.body.to_lowercase());
        for leak in ["nextcloud", "webdav", "ocs", "password", "valkey", "redis", "cloud.example.com"] {
            assert!(!haystack.contains(leak), "an invitation must not mention {leak}");
        }
    }

    // ---- against a live bus -----------------------------------------------

    /// The store half, which no pure function can cover.
    ///
    /// Ignored by default because it needs a Valkey, and the gate that runs on
    /// every push has none. Run it with a bus in reach:
    ///
    /// ```text
    /// COLLABCRAFT_TEST_VALKEY=redis://127.0.0.1:6379/15 cargo test -- --ignored
    /// ```
    ///
    /// What it proves is the part the pure tests cannot. A record survives a
    /// round trip through the bus, the use counter is spent once and then
    /// refuses, and withdrawing the share deletes every key minted from it.
    #[tokio::test]
    #[ignore = "needs a Valkey; set COLLABCRAFT_TEST_VALKEY"]
    async fn the_bus_spends_a_key_once_and_withdrawal_removes_it() {
        let Ok(url) = std::env::var("COLLABCRAFT_TEST_VALKEY") else {
            panic!("set COLLABCRAFT_TEST_VALKEY to a Valkey this test may write to");
        };
        let store = crate::store::Store::new(&url, "collabcraft:selftest").expect("a client");

        let key = Key::mint();
        let link = link_for(&key, NOW + 3600, 1, Permission::Edit);
        assert!(store.put_keyed_link(&key.handle, &link, 300).await, "the record goes in");

        let read_back = store.keyed_link(&key.handle).await.expect("the record comes back");
        assert_eq!(read_back, link, "a round trip through the bus changes nothing");
        assert_eq!(store.keyed_uses(&key.handle).await, 0, "a fresh key is unspent");

        // Spending once is allowed. Spending again is not, which is the whole
        // of a single-use link.
        assert!(store.spend_keyed_link(&key.handle, link.max_uses, 300).await, "the first use is granted");
        assert_eq!(store.keyed_uses(&key.handle).await, 1);
        assert!(!store.spend_keyed_link(&key.handle, link.max_uses, 300).await, "the second use is refused");

        // A key that allows three uses grants three and then stops.
        let three = Key::mint();
        let generous = link_for(&three, NOW + 3600, 3, Permission::View);
        assert!(store.put_keyed_link(&three.handle, &generous, 300).await);
        for use_number in 1..=3 {
            assert!(store.spend_keyed_link(&three.handle, 3, 300).await, "use {use_number} is granted");
        }
        assert!(!store.spend_keyed_link(&three.handle, 3, 300).await, "the fourth is refused");

        // Withdrawing the share withdraws both of them, because both were
        // minted from the same one.
        let dropped = store.drop_keyed_links_for_share(&link.share).await;
        assert_eq!(dropped, 2, "both keys were minted from share {}", link.share);
        assert!(store.keyed_link(&key.handle).await.is_none(), "the record is gone");
        assert!(store.keyed_link(&three.handle).await.is_none(), "so is the other");
        assert_eq!(store.keyed_uses(&key.handle).await, 0, "and so is the counter");

        // Withdrawing a share nothing was minted from is not an error.
        assert_eq!(store.drop_keyed_links_for_share("share-nobody-used").await, 0);
    }

    /// Plant a key a live service can be asked to follow.
    ///
    /// Ignored for the same reason, and separate because it leaves the record
    /// in place on purpose. It writes the key to the path in
    /// `COLLABCRAFT_TEST_KEY_OUT` so a shell can follow the link against a
    /// running service and see the whole chain run.
    #[tokio::test]
    #[ignore = "needs a Valkey; set COLLABCRAFT_TEST_VALKEY and COLLABCRAFT_TEST_KEY_OUT"]
    async fn plant_a_key_for_a_live_service() {
        let Ok(url) = std::env::var("COLLABCRAFT_TEST_VALKEY") else {
            panic!("set COLLABCRAFT_TEST_VALKEY to a Valkey this test may write to");
        };
        let Ok(out) = std::env::var("COLLABCRAFT_TEST_KEY_OUT") else {
            panic!("set COLLABCRAFT_TEST_KEY_OUT to a path the key may be written to");
        };
        // The same prefix the service uses, so the service finds what this
        // writes. The secret has to match the running service's too.
        let secret = std::env::var("COLLABCRAFT_SECRET").expect("the service secret");
        let store = crate::store::Store::new(&url, "collabcraft:svc").expect("a client");

        let key = Key::mint();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let link = Link {
            verifier: key.verifier(secret.as_bytes()).expect("a verifier"),
            room: "room-planted".to_owned(),
            doc: "/Documents/Budget.xlsx".to_owned(),
            app: "gridcraft".to_owned(),
            permission: Permission::Edit,
            recipient: "outsider@example.org".to_owned(),
            owner: "alice".to_owned(),
            share: "share-planted".to_owned(),
            exp: now + 3600,
            max_uses: 1,
        };
        assert!(store.put_keyed_link(&key.handle, &link, 3600).await);
        std::fs::write(&out, key.wire()).expect("the key is written where the shell can read it");
    }
}
