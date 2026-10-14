//! What a failed token call means, and what a recipient is told.
//!
//! The one piece of judgement in the landing flow, tested where there is no
//! browser. Getting it wrong either tells an ordinary visitor their link is
//! broken, or shows a recipient an empty document with no reason for it.

use super::*;

fn refused(code: &str, detail: &str) -> Error {
    Error::Refused { code: code.to_owned(), detail: detail.to_owned() }
}

const FOLLOWED: bool = true;
const RELOADED: bool = false;

#[test]
fn a_dead_key_is_a_refusal_the_recipient_reads() {
    let dead = refused(LINK_INVALID, "this link no longer works. Ask whoever sent it to share the document again");
    let said = Verdict::Refused("this link no longer works. Ask whoever sent it to share the document again".to_owned());
    assert_eq!(verdict(&dead, FOLLOWED), said);
    // A reload is how somebody tries again after it was spent, so the reason
    // has to survive the marker going away.
    assert_eq!(verdict(&dead, RELOADED), said);
}

#[test]
fn somebody_signed_in_is_an_ordinary_visit_however_they_arrived() {
    // An account holder who followed an invitation of their own holds more
    // than it grants and has the ordinary routes. Telling them the link is
    // broken would be a lie about a link that works.
    let theirs = refused(NOT_A_RECIPIENT, "this route is for a shared link");
    assert_eq!(verdict(&theirs, FOLLOWED), Verdict::Ordinary);
    assert_eq!(verdict(&theirs, RELOADED), Verdict::Ordinary);
}

#[test]
fn nobody_at_all_is_an_ordinary_visit() {
    // The usual answer, on every load of every app build by anybody who was
    // not sent anything. Nothing is said to them about any of this.
    assert_eq!(verdict(&Error::Unauthenticated, RELOADED), Verdict::Ordinary);
    assert_eq!(verdict(&Error::Unreachable("offline".to_owned()), RELOADED), Verdict::Ordinary);
    assert_eq!(verdict(&Error::Malformed("not json".to_owned()), RELOADED), Verdict::Ordinary);
}

#[test]
fn a_recipient_whose_service_is_unreachable_is_told_so() {
    // The hole the marker exists to close. Without it this browser looks
    // exactly like an ordinary visitor, and somebody who followed a link to
    // read one document gets an empty application and no reason for it.
    let down = Error::Unreachable("collabcraft did not answer within 60 seconds".to_owned());
    let Verdict::Refused(said) = verdict(&down, FOLLOWED) else { panic!("a recipient was passed over in silence") };
    assert!(said.contains("could not be reached"), "{said}");
    assert!(said.contains("60 seconds"), "the detail is worth keeping: {said}");
}

#[test]
fn a_refusal_with_nothing_in_it_still_says_something() {
    // The service always sends a sentence. A deployment behind something that
    // rewrites error bodies would not, and a blank page is what this whole
    // flow exists to prevent.
    assert_eq!(verdict(&refused(LINK_INVALID, ""), RELOADED), Verdict::Refused(UNEXPLAINED.to_owned()));
    assert_eq!(verdict(&refused(LINK_INVALID, "   "), RELOADED), Verdict::Refused(UNEXPLAINED.to_owned()));
}

#[test]
fn a_refusal_nobody_named_is_still_a_refusal_for_a_recipient() {
    // Some other code from the service, on a browser the landing page sent.
    let odd = refused("mint_failed", "the relay token could not be signed");
    assert_eq!(verdict(&odd, FOLLOWED), Verdict::Refused("the relay token could not be signed".to_owned()));
    // The same thing on an ordinary visit says nothing, because an ordinary
    // visitor is not owed an explanation of a route they did not ask for.
    assert_eq!(verdict(&odd, RELOADED), Verdict::Ordinary);
}

#[test]
fn the_marker_is_one_word_and_matched_whole() {
    assert!(followed_a_link("?invite"));
    assert!(followed_a_link("?invite=1"));
    assert!(followed_a_link("?webgl&invite"));
    assert!(followed_a_link("invite"), "a search string with no question mark is the same string");
    // A parameter that merely contains the word is a different parameter.
    assert!(!followed_a_link("?invited=yes"));
    assert!(!followed_a_link("?sample=invite"), "the value is not the name");
    assert!(!followed_a_link(""));
    assert!(!followed_a_link("?webgl"));
}
