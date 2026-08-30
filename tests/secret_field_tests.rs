//! `element::SECRET_FIELD` beyond `type=password`.
//!
//! The predicate used to be structural only, so a masked field the DOM spells any other way —
//! a "show password" toggle, an OTP widget, a card expiry, an IBAN — reached stdout, the agent
//! transcript and any `--record` file in clear. Both directions are pinned here: the four
//! families that must be redacted, and the four that must NOT be, because a false positive
//! withholds a value the caller legitimately needs to read back.

use std::process::Command;

use serde_json::Value;

mod common;
use common::TestBrowser;

fn run_cli(args: &[&str]) -> (String, i32) {
    let output = Command::new(common::binary())
        .args(args)
        .output()
        .expect("run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Fill one field of the shapes fixture and return the response.
fn fill_shape(browser: &str, selector: &str, value: &str) -> Option<Value> {
    if !common::browser_ready() {
        return None;
    }
    let url = common::fixture_url("secret_field_shapes.html");
    let (_, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        common::unavailable("goto secret_field_shapes.html failed");
        return None;
    }
    let (out, code) = run_cli(&[
        "--browser",
        browser,
        "--verdict",
        "off",
        "--json",
        "fill",
        "--selector",
        selector,
        value,
    ]);
    assert_eq!(code, 0, "the fill itself must land: {out}");
    Some(serde_json::from_str(&out).unwrap_or_else(|e| panic!("not JSON ({e}): {out}")))
}

/// A response that redacted: lengths and `verbatim` survive, the value never appears.
fn assert_redacted(response: &Value, value: &str) {
    assert_eq!(response["value"]["redacted"], true, "{response}");
    assert_eq!(
        response["value"]["requested_length"],
        value.chars().count(),
        "the length still classifies it: {response}"
    );
    assert!(response["value"].get("requested").is_none(), "{response}");
    assert!(response["value"].get("actual").is_none(), "{response}");
    assert!(
        !serde_json::to_string(response)
            .unwrap_or_default()
            .contains(value),
        "the value reached stdout: {response}"
    );
}

/// A response that did not redact: both strings are there, which is what a read-back is for.
fn assert_printed(response: &Value, value: &str) {
    assert!(
        response["value"].get("redacted").is_none(),
        "a field that declares nothing secret must still be readable back: {response}"
    );
    assert_eq!(response["value"]["requested"], value, "{response}");
    assert_eq!(response["value"]["actual"], value, "{response}");
}

/// The toggle is the point: the site sets `type = 'text'` so the user can see the password.
/// Nothing structural is left, and the name is the only thing still saying what it holds.
#[test]
fn a_shown_password_is_still_a_password() {
    let b = TestBrowser::new("secret-shape-toggle");
    let Some(v) = fill_shape(b.name(), "#passwordToggle", "hunter2secret") else {
        return;
    };
    assert_redacted(&v, "hunter2secret");
}

/// An OTP widget that declares nothing: `type=text inputmode=numeric autocomplete="off"`, and a
/// name that names nothing. Its shape is the whole signal.
#[test]
fn a_numeric_short_field_that_declares_nothing_is_treated_as_a_code() {
    let b = TestBrowser::new("secret-shape-otp");
    let Some(v) = fill_shape(b.name(), "#field7", "483920") else {
        return;
    };
    assert_redacted(&v, "483920");
}

/// `cc-exp` and `cc-name` were absent from the token list while `cc-number` was in it; a card's
/// expiry and holder are part of the same card.
#[test]
fn the_rest_of_the_card_is_as_secret_as_its_number() {
    let b = TestBrowser::new("secret-shape-card");
    let Some(v) = fill_shape(b.name(), "#exp", "12/29") else {
        return;
    };
    assert_redacted(&v, "12/29");

    let Some(v) = fill_shape(b.name(), "#holder", "Ada Lovelace") else {
        return;
    };
    assert_redacted(&v, "Ada Lovelace");
}

/// An IBAN names nothing in any autocomplete list, and no `type` describes it. The field names
/// itself, which is the only handle there is.
#[test]
fn an_iban_field_is_redacted_by_the_name_it_carries() {
    let b = TestBrowser::new("secret-shape-iban");
    let Some(v) = fill_shape(b.name(), "#ibanField", "GB33BUKB20201555555555") else {
        return;
    };
    assert_redacted(&v, "GB33BUKB20201555555555");
}

/// `aria-label` counts too: a field whose `name` and `id` are opaque still tells a screen reader
/// what it holds.
#[test]
fn an_aria_label_names_a_national_id_when_nothing_else_does() {
    let b = TestBrowser::new("secret-shape-aria");
    let Some(v) = fill_shape(b.name(), "#x9", "180012345678") else {
        return;
    };
    assert_redacted(&v, "180012345678");
}

/// The other direction, and the one that costs something when it is wrong: an ordinary field
/// must still report what the page kept.
#[test]
fn an_ordinary_field_is_not_redacted() {
    let b = TestBrowser::new("secret-shape-plain");
    let Some(v) = fill_shape(b.name(), "#email", "ada@example.com") else {
        return;
    };
    assert_printed(&v, "ada@example.com");
}

/// Numeric and five characters, which is the OTP shape — but it DECLARED its purpose, so it is
/// judged by what it declared and not by its shape. Without this the postal code of every
/// checkout form would come back redacted.
#[test]
fn a_declared_postal_code_is_not_mistaken_for_a_code() {
    let b = TestBrowser::new("secret-shape-zip");
    let Some(v) = fill_shape(b.name(), "#zip", "75011") else {
        return;
    };
    assert_printed(&v, "75011");
}

/// Below four digits is a quantity spinner or a CVC, and a CVC declares `cc-csc`.
#[test]
fn a_two_digit_numeric_field_is_a_quantity_not_a_pin() {
    let b = TestBrowser::new("secret-shape-qty");
    let Some(v) = fill_shape(b.name(), "#qty", "12") else {
        return;
    };
    assert_printed(&v, "12");
}

/// The word rule matches WORDS. `pin` inside `pinterest` is the false positive a bare substring
/// test would produce, and the normalisation (camelCase split, then word boundaries) is what
/// stops it.
#[test]
fn the_word_rule_does_not_match_a_substring() {
    let b = TestBrowser::new("secret-shape-substring");
    let Some(v) = fill_shape(b.name(), "#pinterestUrl", "https://pinterest.com/ada") else {
        return;
    };
    assert_printed(&v, "https://pinterest.com/ada");
}

/// The tree renderer reads the same predicate (`snapshot_secret`), so the values it now hides
/// are the same set. A widened predicate that only reached `fill` would put the OTP back on
/// stdout through `inspect`.
#[test]
fn the_accessibility_tree_hides_the_same_set_of_values() {
    if !common::browser_ready() {
        return;
    }
    let b = TestBrowser::new("secret-shape-tree");
    let url = common::fixture_url("secret_field_shapes.html");
    let (_, code) = run_cli(&["--browser", b.name(), "goto", &url]);
    if code != 0 {
        common::unavailable("goto secret_field_shapes.html failed");
        return;
    }
    for (selector, value) in [
        ("#field7", "483920"),
        ("#ibanField", "GB33BUKB20201555555555"),
    ] {
        let (out, code) = run_cli(&[
            "--browser",
            b.name(),
            "--verdict",
            "off",
            "fill",
            "--selector",
            selector,
            value,
        ]);
        assert_eq!(code, 0, "{out}");
    }
    let (tree, code) = run_cli(&["--browser", b.name(), "inspect"]);
    assert_eq!(code, 0, "{tree}");
    assert!(!tree.contains("483920"), "the code is in the tree: {tree}");
    assert!(
        !tree.contains("GB33BUKB20201555555555"),
        "the IBAN is in the tree: {tree}"
    );
    assert!(
        tree.contains("<redacted>"),
        "and something was redacted in its place: {tree}"
    );
}
