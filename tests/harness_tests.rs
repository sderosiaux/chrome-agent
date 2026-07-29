//! Tests for the test harness itself.
//!
//! The suite's worst failure mode was never a wrong assertion — it was a green run that
//! asserted nothing. These tests pin the two guards that make that impossible: a missing
//! precondition fails under `CHROME_AGENT_REQUIRE_CHROME`, and a missing fixture always does.

mod common;

#[test]
fn a_skip_is_a_failure_when_a_browser_run_is_required() {
    let panicked = std::panic::catch_unwind(|| common::unavailable_with(true, "Chrome not found"));
    let payload = panicked.expect_err("a required browser run must not be skippable");
    let message = payload
        .downcast_ref::<String>()
        .map_or_else(|| String::from("<non-string panic>"), Clone::clone);
    assert!(
        message.contains("Chrome not found"),
        "the panic must name the unmet precondition, got: {message}"
    );
    assert!(
        message.contains(common::REQUIRE_ENV),
        "the panic must name the variable that made it fatal, got: {message}"
    );
}

#[test]
fn a_skip_stays_a_skip_when_no_browser_run_is_required() {
    assert!(
        !common::unavailable_with(false, "Chrome not found"),
        "without the variable a missing Chrome skips, and the caller returns early"
    );
}

#[test]
fn the_require_flag_reads_its_value_not_just_its_presence() {
    // The env lookup itself is not exercised here: the crate forbids `unsafe`, and
    // `set_var` is unsafe in edition 2024. What is worth pinning is the parse — an
    // exported-but-disabled `CHROME_AGENT_REQUIRE_CHROME=0` must not make CI fatal.
    assert!(!common::require_from(None), "unset means skips are allowed");
    assert!(common::require_from(Some("1")), "set means skips are fatal");
    assert!(!common::require_from(Some("0")), "an explicit 0 opts back out");
    assert!(!common::require_from(Some("")), "an empty value opts back out");
}

#[test]
#[should_panic(expected = "fixture does not exist")]
fn a_missing_fixture_is_never_a_usable_url() {
    // Before this guard, `file://…/deleted.html` produced an error page and every test that
    // loaded it returned early on a "goto failed" skip.
    let _ = common::fixture_url("this_fixture_was_deleted.html");
}

#[test]
fn a_present_fixture_resolves_to_a_file_url() {
    let url = common::fixture_url("press_keys.html");
    assert!(url.starts_with("file:///"), "got {url}");
    assert!(url.ends_with("/tests/fixtures/press_keys.html"), "got {url}");
}
