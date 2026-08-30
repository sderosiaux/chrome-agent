//! `upload` and `drag`, the two verbs that had no test at all — neither a `#[cfg(test)]` module
//! nor a suite naming them. Both act through `element_controls`, and both are judged here by what
//! the PAGE ends up holding, not by the sentence the command prints about itself.
//!
//! `drag` sends CDP mouse events, so a mousedown-based list (the Sortable.js / React-DnD-mouse
//! shape) is the fixture. `tests/fixtures/drag_list.html` also carries an HTML5 Drag and Drop
//! pair, which is NOT asserted on: see the comment in the fixture for what was measured.

use std::process::{Command, Output};

mod common;
use common::TestBrowser;

fn run(browser: &str, args: &[&str]) -> Output {
    Command::new(common::binary())
        .args(["--browser", browser])
        .args(args)
        .output()
        .expect("run chrome-agent")
}

fn stdout_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_string()
}

/// What the page itself says, read through `text --selector`.
fn text(browser: &str, selector: &str) -> String {
    let output = run(browser, &["text", "--selector", selector]);
    assert!(output.status.success(), "text {selector}: {}", stderr_of(&output));
    stdout_of(&output)
}

/// The uid of the first snapshot line containing `needle`. `drag` and `upload --uid` both need a
/// uid from a STORED snapshot, so the caller must have run `inspect` first.
fn uid_of(tree: &str, needle: &str) -> String {
    tree.lines()
        .find(|line| line.contains(needle))
        .and_then(|line| line.split_whitespace().next())
        .and_then(|token| token.strip_prefix("uid="))
        .unwrap_or_else(|| panic!("no uid for {needle} in:\n{tree}"))
        .to_string()
}

fn inspect(browser: &str) -> String {
    let output = run(browser, &["inspect"]);
    assert!(output.status.success(), "inspect: {}", stderr_of(&output));
    stdout_of(&output)
}

/// A file this test owns, holding `contents`.
fn file_with(label: &str, contents: &str) -> std::path::PathBuf {
    let path = common::temp_path(label, "txt");
    std::fs::write(&path, contents).expect("write the upload fixture file");
    path
}

/// Both targeting modes, judged by the `change` event the page received: an `<input type=file>`
/// whose `files` list the page can read is the only thing "uploaded" can mean.
#[test]
fn upload_hands_the_file_to_the_page_by_selector_and_by_uid() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-upload");
    let browser = guard.name().to_string();
    let one = file_with("upload-one", "hello\n");
    let two = file_with("upload-two", "hi\n");

    let page = common::fixture_url("upload_form.html");
    assert!(run(&browser, &["goto", &page]).status.success());
    assert_eq!(text(&browser, "#picked"), "no file", "the fixture starts empty");

    // By selector.
    let output = run(&browser, &["upload", one.to_str().unwrap(), "--selector", "#picker"]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    let name = one.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(text(&browser, "#picked"), format!("{name}:6"), "the page read a different file");
    assert_eq!(text(&browser, "#events"), "1", "the input never fired `change`");

    // By uid, which needs a stored snapshot: the file input is a `button` in the a11y tree.
    let uid = uid_of(&inspect(&browser), "Attachment\" value=");
    let output = run(
        &browser,
        &["upload", one.to_str().unwrap(), two.to_str().unwrap(), "--uid", &uid, "--json"],
    );
    assert!(output.status.success(), "{}", stderr_of(&output));
    let value: serde_json::Value = serde_json::from_str(&stdout_of(&output)).expect("JSON");
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["uid"], uid, "the node acted on is named back: {value}");
    assert!(
        value["message"].as_str().unwrap().contains("2 file(s)"),
        "the count is the caller's own: {value}"
    );
    let two_name = two.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(
        text(&browser, "#picked"),
        format!("{name}:6,{two_name}:3"),
        "both files must reach the input, in the order they were given"
    );
    assert_eq!(text(&browser, "#events"), "2");

    std::fs::remove_file(&one).ok();
    std::fs::remove_file(&two).ok();
}

/// A path that is not there is refused BEFORE the CDP call, so the input keeps what it held.
/// The same for an absent or doubled target: an invocation is judged before a page is.
#[test]
fn upload_refuses_a_missing_file_or_an_unclear_target_without_touching_the_page() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-upload-refuse");
    let browser = guard.name().to_string();
    let real = file_with("upload-real", "hello\n");
    let absent = common::temp_path("upload-absent", "txt");
    assert!(!absent.exists(), "the fixture path must not exist for this to prove anything");

    let page = common::fixture_url("upload_form.html");
    assert!(run(&browser, &["goto", &page]).status.success());
    let output = run(&browser, &["upload", real.to_str().unwrap(), "--selector", "#picker"]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    let held = text(&browser, "#picked");

    let refused = run(&browser, &["upload", absent.to_str().unwrap(), "--selector", "#picker"]);
    assert_eq!(refused.status.code(), Some(1), "{}", stdout_of(&refused));
    let message = stderr_of(&refused);
    assert!(
        message.contains(&absent.display().to_string()),
        "the refusal must name the path it could not find: {message}"
    );
    assert_eq!(
        text(&browser, "#picked"),
        held,
        "the input was changed by an upload that failed, so the refusal came too late"
    );
    assert_eq!(text(&browser, "#events"), "1", "a refused upload fired `change`");

    // Neither target, and both: named by argument, not by machine state.
    let none = run(&browser, &["upload", real.to_str().unwrap()]);
    assert_eq!(none.status.code(), Some(1));
    assert!(stderr_of(&none).contains("--uid or --selector"), "{}", stderr_of(&none));
    let both = run(
        &browser,
        &["upload", real.to_str().unwrap(), "--uid", "n10", "--selector", "#picker"],
    );
    assert_eq!(both.status.code(), Some(1));
    assert!(stderr_of(&both).contains("Only one of"), "{}", stderr_of(&both));

    std::fs::remove_file(&real).ok();
}

/// The happy path: a list that reorders on mousedown/mousemove/mouseup, which is what `drag`
/// dispatches. The move count is asserted too — a press and a release with nothing between them
/// is a click, and this fixture refuses to reorder for one.
#[test]
fn drag_reorders_a_mousedown_list_and_sends_the_moves_between() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-drag");
    let browser = guard.name().to_string();

    let page = common::fixture_url("drag_list.html");
    assert!(run(&browser, &["goto", &page]).status.success());
    assert_eq!(text(&browser, "#order"), "alpha,bravo,charlie");
    assert_eq!(text(&browser, "#moves"), "0");

    let tree = inspect(&browser);
    let from = uid_of(&tree, "\"Alpha\"");
    let to = uid_of(&tree, "\"Charlie\"");

    let output = run(&browser, &["drag", &from, &to]);
    assert!(output.status.success(), "{}", stderr_of(&output));
    assert!(
        stdout_of(&output).contains(&format!("Dragged uid={from} to uid={to}")),
        "{}",
        stdout_of(&output)
    );
    assert_eq!(
        text(&browser, "#order"),
        "bravo,charlie,alpha",
        "the item was not carried to the destination"
    );
    assert_eq!(
        text(&browser, "#moves"),
        "5",
        "the 5-step interpolation is what makes this a drag and not a click"
    );
}

/// A uid no stored snapshot holds is refused, and nothing is dispatched: the list is where it
/// was, and no button was ever pressed on the page.
#[test]
fn drag_refuses_a_uid_no_snapshot_holds_and_moves_nothing() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-drag-refuse");
    let browser = guard.name().to_string();

    let page = common::fixture_url("drag_list.html");
    assert!(run(&browser, &["goto", &page]).status.success());
    let destination = uid_of(&inspect(&browser), "\"Charlie\"");

    let refused = run(&browser, &["drag", "n999999", &destination]);
    assert_eq!(refused.status.code(), Some(1), "{}", stdout_of(&refused));
    let message = stderr_of(&refused);
    assert!(message.contains("n999999"), "the refusal names the uid it could not resolve: {message}");
    assert!(message.contains("inspect"), "and how to get a live one: {message}");

    assert_eq!(text(&browser, "#order"), "alpha,bravo,charlie", "a refused drag reordered the list");
    assert_eq!(text(&browser, "#moves"), "0", "a refused drag still moved the mouse");
}
