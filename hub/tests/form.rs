//! H2 — §10 AC10's decoder cases, and the ones around them.
//!
//! Every line here is a way a free-text answer typed on a phone can come back
//! wrong, and a wrong answer is one the orchestrator then acts on.

use hub::form::{Form, decode, decode_path, encode_component};

#[test]
fn ac10_the_spec_string_round_trips() {
    let form = Form::parse("text=a+b%26c%3Dd%2B%F0%9F%99%82");
    assert_eq!(form.get("text"), Some("a b&c=d+🙂"));
}

#[test]
fn plus_is_a_space_and_percent_2b_is_a_plus() {
    assert_eq!(decode("a+b"), "a b");
    assert_eq!(decode("a%2Bb"), "a+b");
    assert_eq!(decode("a%2bb"), "a+b", "hex digits are case-insensitive");
    // A path is not a form: there, `+` is a plus.
    assert_eq!(decode_path("a+b"), "a+b");
    assert_eq!(decode_path("a%20b"), "a b");
}

#[test]
fn percent_25_is_a_literal_percent_and_does_not_start_a_second_escape() {
    assert_eq!(decode("100%25"), "100%");
    assert_eq!(decode("%2541"), "%41", "the decode happens once, not twice");
}

#[test]
fn a_multi_byte_character_split_across_escapes_survives() {
    // 🙂 is four bytes and therefore four escapes; a decoder that made a char
    // per escape would produce four replacement characters here.
    assert_eq!(decode("%F0%9F%99%82"), "🙂");
    assert_eq!(decode("%E2%80%94"), "—");
    assert_eq!(decode("caf%C3%A9"), "café");
    // Split across a literal too.
    assert_eq!(decode("a%C3%A9b"), "aéb");
}

#[test]
fn ampersands_and_equals_inside_a_value_stay_inside_it() {
    let form = Form::parse("id=ABC&text=x%3Dy%26z");
    assert_eq!(form.get("id"), Some("ABC"));
    assert_eq!(form.get("text"), Some("x=y&z"));

    // An unescaped `=` in a value is a value, not a second field: the split is
    // on the first `=` only.
    let form = Form::parse("text=a=b=c");
    assert_eq!(form.get("text"), Some("a=b=c"));
}

#[test]
fn missing_and_empty_are_different_answers() {
    let form = Form::parse("id=ABC&text=");
    assert_eq!(form.get("text"), Some(""), "sent, and empty");
    assert_eq!(form.get("nothing"), None, "never sent");

    // A bare field name is present with an empty value.
    let form = Form::parse("flag&id=ABC");
    assert_eq!(form.get("flag"), Some(""));
    assert_eq!(form.get("id"), Some("ABC"));
}

#[test]
fn a_broken_escape_is_kept_rather_than_swallowed() {
    assert_eq!(decode("%ZZ"), "%ZZ");
    assert_eq!(decode("50%"), "50%");
    assert_eq!(decode("%4"), "%4");
    assert_eq!(decode("a%"), "a%");
    assert_eq!(decode("%%41"), "%A");
}

#[test]
fn invalid_utf8_is_replaced_rather_than_fatal() {
    // A lone continuation byte cannot come from a keyboard, but it can come
    // from a crafted request; hub must answer, not panic.
    assert_eq!(decode("%FF"), "\u{fffd}");
    assert_eq!(decode("a%FFb"), "a\u{fffd}b");
}

#[test]
fn an_empty_or_junk_body_decodes_to_nothing_rather_than_panicking() {
    assert!(Form::parse("").is_empty());
    assert!(Form::parse("&&&").is_empty());
    assert_eq!(Form::parse("=value").get(""), Some("value"));
}

#[test]
fn the_first_value_wins_when_a_field_is_repeated() {
    // A crafted POST can send `id` twice hoping the display and the write pick
    // different ones. There is one rule, and it is the first.
    let form = Form::parse("id=GOOD&id=EVIL");
    assert_eq!(form.get("id"), Some("GOOD"));
    assert_eq!(form.iter().count(), 2);
}

#[test]
fn a_newline_heavy_batched_answer_survives_the_trip() {
    let answer = "1. yes\r\n2. no, use sqlite\r\n3. skip";
    let body = format!("text={}", encode_component(answer));
    assert_eq!(Form::parse(&body).get("text"), Some(answer));
}
