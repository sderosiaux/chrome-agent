//! `download --uid` / `download --selector`: the file a CLICK produces.
//!
//! The gap these cover is the one `download <url>` cannot reach by construction — a file built
//! in the page with `URL.createObjectURL` has no address to hand to a fetch, and an anchor whose
//! `href` is a blob is not something `inspect --urls` can resolve into a download.
//!
//! Four outcomes, because they have four different recoveries and only one of them is a file:
//! the download completed, the click landed and nothing began, the transfer went past
//! `--max-bytes`, and the transfer was still running when the wait ended.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

mod common;
use common::TestBrowser;

fn binary() -> PathBuf {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path
}

/// Every chrome-agent this suite started, by pid, and the browser it was started for.
///
/// A transfer directory is named `.incoming-<pid>-<nanos>` after the process that opened it, so
/// the pid is what tells this suite's leftovers from another test PROCESS's. The browser name is
/// what tells one test in this process from another: `TestBrowser` makes it unique per test, and
/// without it a test asserts on a sibling thread's in-flight transfers — which is the same "a test
/// owns what it names and nothing else" rule `harness_tests` enforces, one level in. See
/// `assert_nothing_outlives_its_process`.
static OURS: std::sync::Mutex<Option<std::collections::HashMap<u32, String>>> =
    std::sync::Mutex::new(None);

fn run(browser: &str, args: &[&str]) -> Output {
    let child = Command::new(binary())
        .args(["--browser", browser])
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("run chrome-agent");
    if let Ok(mut ours) = OURS.lock() {
        ours.get_or_insert_with(std::collections::HashMap::new)
            .insert(child.id(), browser.to_string());
    }
    child.wait_with_output().expect("run chrome-agent")
}

fn json_of(output: &Output) -> serde_json::Value {
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!("not JSON ({error}): stdout={stdout} stderr={}", String::from_utf8_lossy(&output.stderr))
    })
}


fn unique_temp_dir(tag: &str) -> PathBuf {
    let path = common::temp_path(tag, "d");
    std::fs::create_dir_all(&path).unwrap();
    path
}

/// The private directories `download --selector` writes into while Chrome is transferring.
///
/// Named after the chrome-agent process that opened it. Swept before that process exits, which
/// usually removes it — but not always, and never on a transfer Chrome is still running, which is
/// why `assert_nothing_outlives_its_process` and not a count. Compared as a SET rather than a
/// count for a second reason: these tests run in parallel, and a sibling's in-flight transfer
/// would otherwise read as this one's leak.
fn incoming_dirs() -> std::collections::HashSet<String> {
    let Some(home) = dirs_home() else { return std::collections::HashSet::new() };
    let Ok(entries) = std::fs::read_dir(home.join(".chrome-agent").join("tmp")) else {
        return std::collections::HashSet::new();
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".incoming-"))
        .collect()
}

/// Only leftovers THIS test produced count.
///
/// Two neighbours are excluded, and both were measured rather than imagined. A second test
/// PROCESS on the machine — the normal regime here — starts its own transfers after the `before`
/// snapshot and they sit in the difference for as long as they run: on two full suites in
/// parallel, `.incoming-76300-…` and `.incoming-76268-…`, neither pid a child of this test,
/// were reported as this test's leak. And a second test in THIS process does the same across
/// threads: `.incoming-71509-…` and `.incoming-71525-…`, opened by the slow-transfer test and
/// still being written to by Chrome, failed three sibling tests at once. The pid separates the
/// first, the browser name separates the second, and `TestBrowser` is what makes that name unique
/// per test.
///
/// Third property, which is what lets the assertion below need no liveness probe of its own:
/// `run` waits for its child, so every pid in this map has already been reaped. A name this
/// returns is therefore always a directory whose owner is gone.
fn ours(browser: &str, names: &std::collections::HashSet<String>) -> Vec<String> {
    let started = OURS.lock().ok().and_then(|o| o.clone()).unwrap_or_default();
    names
        .iter()
        .filter(|name| {
            name.split('-')
                .nth(1)
                .and_then(|pid| pid.parse::<u32>().ok())
                .and_then(|pid| started.get(&pid))
                .is_some_and(|theirs| theirs == browser)
        })
        .cloned()
        .collect()
}

/// A selector no fixture has. Arming happens before the click, so a `download` that cannot find
/// its target is still the cheapest invocation that collects.
const SWEEPING_TARGET: &str = "#no-element-has-this-id";

/// The promise, which is not the one this used to assert.
///
/// It used to demand that no transfer directory of ours be on disk within three seconds, and it
/// polled for those three seconds without doing anything that could bring it about — nothing in
/// the tool collects one of these except the invocation that opened it, and that invocation had
/// already exited. So the assertion held only when Chrome happened to finish inside `clean_up`'s
/// 120 ms window, and failed on a loaded runner. That is a promise the product does not make and
/// cannot: measured on the path where the transfer is still running when `--timeout` expires,
/// eight invocations out of eight had their directory back on disk fifteen seconds later, because
/// Chrome keeps the download after chrome-agent is gone. Any window wide enough to cover that is
/// the length of the download.
///
/// What IS promised, and what this now asserts: nothing accumulates from one invocation to the
/// next. A transfer directory belongs to the process named in it, and once that process is gone
/// nothing can write to it any more, so the next `download` that arms collects it. This therefore
/// RUNS one instead of waiting for one, and repeats — a transfer still finishing can put another
/// directory there after a sweep, and the promise is that it converges, not that it is instant.
///
/// The other half of the promise — that no partial file is ever handed back to the caller — is
/// asserted at each call site on the caller's own `--out` path, where it belongs.
fn assert_nothing_outlives_its_process(browser: &str, before: &std::collections::HashSet<String>) {
    // Everything `ours` returns has an exited owner by construction (see its docs), so this needs
    // no liveness probe — and must not grow one, since that would be a second implementation of a
    // rule `session::liveness` already owns and this binary-only crate cannot export.
    let abandoned = || {
        let new: std::collections::HashSet<String> =
            incoming_dirs().difference(before).cloned().collect();
        ours(browser, &new)
    };
    for _ in 0..5 {
        let _ = run(browser, &["download", "--selector", SWEEPING_TARGET, "--timeout", "1", "--json"]);
        if abandoned().is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        abandoned().is_empty(),
        "transfer directories outlived the processes that opened them, so a later invocation \
         could not collect them: {:?}",
        abandoned()
    );
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(unix)]
fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// A blob anchor and a JS-built download: neither has a URL anything outside the page can fetch,
/// which is the whole reason this path exists. Both land on disk at 0600, named by the server's
/// suggestion when `--out` says nothing.
#[test]
fn a_click_captures_a_blob_download_by_selector_and_by_uid() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-dl-click");
    let browser = guard.name().to_string();
    let out_dir = unique_temp_dir("dl-click");
    let before = incoming_dirs();

    let page = common::fixture_url("download_click.html");
    assert!(run(&browser, &["goto", &page]).status.success());

    // By selector: an <a download> whose href is a Blob.
    let anchor = out_dir.join("anchor.csv");
    let output = run(
        &browser,
        &["download", "--selector", "#blob-link", "--out", anchor.to_str().unwrap(), "--json"],
    );
    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    let value = json_of(&output);
    assert_eq!(value["ok"], true);
    assert_eq!(value["downloaded"], true, "{value}");
    assert_eq!(value["via"], "click");
    // The name the page proposed is reported even though --out overrode where it went: it is
    // the only place the server's own name for the file survives.
    assert_eq!(value["suggested_filename"], "report.csv", "{value}");
    assert_eq!(value["bytes"], 22, "{value}");
    // The click's own evidence rides along, so a caller can tell a file that arrived from the
    // element it aimed at from one that arrived from whatever was stacked on top of it.
    assert_eq!(value["delivery"], "target_hit", "{value}");
    assert!(value["uid"].is_string(), "the node that was clicked is named: {value}");
    // `verdict_hint` belongs to a vocabulary this command does not carry.
    assert!(value.get("verdict_hint").is_none(), "{value}");
    assert_eq!(std::fs::read_to_string(&anchor).unwrap(), "id,name\n1,ada\n2,grace\n");
    #[cfg(unix)]
    assert_eq!(mode_of(&anchor), 0o600, "a downloaded file is 0600 like every other file we write");

    // By uid, on the handler-built download: the anchor is created, clicked and removed inside
    // JS, so nothing in the DOM ever names the file.
    let inspect = run(&browser, &["inspect", "--filter", "button"]);
    let tree = String::from_utf8_lossy(&inspect.stdout);
    let uid = tree
        .lines()
        .find(|line| line.contains("Export via JS"))
        .and_then(|line| line.split_whitespace().next())
        .and_then(|token| token.strip_prefix("uid="))
        .expect("the JS export button is in the snapshot")
        .to_string();

    let generated = out_dir.join("generated.csv");
    let output = run(
        &browser,
        &["download", "--uid", &uid, "--out", generated.to_str().unwrap(), "--json"],
    );
    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    let value = json_of(&output);
    assert_eq!(value["downloaded"], true, "{value}");
    assert_eq!(value["suggested_filename"], "generated.csv", "{value}");
    assert_eq!(value["uid"], uid, "{value}");
    assert_eq!(std::fs::read_to_string(&generated).unwrap(), "id,name\n1,ada\n2,grace\n");

    assert_nothing_outlives_its_process(&browser, &before);
    std::fs::remove_dir_all(out_dir).unwrap();
}

/// The case the mission is really about not getting wrong: the click landed, nothing downloaded.
///
/// It must terminate at the bound, say so in a field rather than only in prose, exit 0 — a
/// delivered click is not an error — and forbid the retry, because repeating it is a second real
/// click and the page cannot tell that from a second deliberate action.
#[test]
fn a_click_that_downloads_nothing_says_so_and_forbids_the_retry() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-dl-none");
    let browser = guard.name().to_string();
    let before = incoming_dirs();

    let page = common::fixture_url("download_click.html");
    assert!(run(&browser, &["goto", &page]).status.success());

    let started = std::time::Instant::now();
    let output = run(&browser, &["download", "--selector", "#inert", "--timeout", "2", "--json"]);
    let waited = started.elapsed();

    assert!(output.status.success(), "a delivered click is not an error");
    assert!(waited < Duration::from_secs(20), "the wait was not bounded: {waited:?}");
    let value = json_of(&output);
    assert_eq!(value["ok"], true, "{value}");
    assert_eq!(value["downloaded"], false, "{value}");
    assert!(value.get("path").is_none(), "no file, so no path is claimed: {value}");
    assert!(value["message"].as_str().unwrap().contains("no download began"), "{value}");
    let hint = value["hint"].as_str().expect("a hint");
    assert!(hint.contains("Do not click again"), "rule 3, in words: {hint}");
    assert!(hint.contains(&format!("chrome-agent --browser {browser} inspect --urls")), "{hint}");
    // The click really did reach the page: the fixture's handler wrote a status line.
    let status = run(&browser, &["text", "--selector", "#status"]);
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("nothing downloaded"),
        "the click was not delivered, so this test proves nothing"
    );

    assert_nothing_outlives_its_process(&browser, &before);
}

/// `--max-bytes` means one thing on both halves of the verb. On the click path Chrome is asked to
/// stop and the partial file is removed, rather than the flag being declared inapplicable here.
#[test]
fn a_click_download_past_the_byte_ceiling_is_cancelled_and_nothing_is_written() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-dl-cap");
    let browser = guard.name().to_string();
    let out_dir = unique_temp_dir("dl-cap");
    let before = incoming_dirs();

    let page = common::fixture_url("download_click.html");
    assert!(run(&browser, &["goto", &page]).status.success());

    let capped = out_dir.join("capped.csv");
    let output = run(
        &browser,
        &[
            "download",
            "--selector",
            "#blob-link",
            "--out",
            capped.to_str().unwrap(),
            "--max-bytes",
            "5",
            "--json",
        ],
    );
    assert!(output.status.success(), "a delivered click is not an error");
    let value = json_of(&output);
    assert_eq!(value["downloaded"], false, "{value}");
    assert!(value["message"].as_str().unwrap().contains("exceeded 5 bytes"), "{value}");
    assert!(!capped.exists(), "a cancelled transfer wrote {capped:?}");
    // Repeated, because the cancel is the path where Chrome can finalise after we have swept,
    // recreating the directory and a zero-byte stub behind us. Three invocations is a backlog the
    // collector has to drain, not a single directory it could have got lucky on.
    for _ in 0..2 {
        let _ = run(&browser, &["download", "--selector", "#blob-link", "--max-bytes", "5", "--json"]);
    }
    assert_nothing_outlives_its_process(&browser, &before);

    std::fs::remove_dir_all(out_dir).unwrap();
}

/// Arming is where the collection happens, and this is the only test that says so end to end.
///
/// The unit tests in `download_click.rs` pin the predicate; what they cannot see is whether
/// anything calls it. Without this, a `collect_abandoned` that was written and never wired would
/// pass every other test here — verified by neutering the predicate and running the suite: seven
/// of eight still passed, and this one did not.
///
/// The race itself is deliberately NOT what the suite reproduces. It is timing-dependent by
/// construction — the leak was measured at eight invocations out of eight against a transfer that
/// was still running, and at zero out of fifteen against a blob that finalises in milliseconds —
/// so a test built on it would be green on one machine and red on another, which is the failure
/// mode that brought this fix about. A directory whose owner has exited is the state the race
/// LEAVES BEHIND, it can be planted deterministically, and collecting it is the whole promise.
#[test]
fn a_directory_left_by_a_process_that_has_exited_is_collected_by_the_next_arming() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-dl-collect");
    let browser = guard.name().to_string();
    let before = incoming_dirs();

    // A pid nothing holds any more: spawned, waited on, and therefore reaped. It is not one of
    // OURS, so the planted directory is identified by its path rather than through `ours` — and
    // if a sibling test process happens to collect it first, that is this feature working.
    let mut child =
        Command::new("/bin/sh").args(["-c", "exit 0"]).spawn().expect("spawn a short-lived process");
    let dead = child.id();
    child.wait().expect("reap it");

    let tmp = dirs_home().expect("HOME").join(".chrome-agent").join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let planted = tmp.join(format!(".incoming-{dead}-1788086042802162000"));
    std::fs::create_dir_all(&planted).unwrap();
    std::fs::write(planted.join("6f1c1f0e-guid"), b"partial").unwrap();
    assert!(planted.exists(), "the fixture did not get planted, so this proves nothing");

    let page = common::fixture_url("download_click.html");
    assert!(run(&browser, &["goto", &page]).status.success());
    // Any `download` with a click target arms, and arming is what collects — the click itself is
    // beside the point, so this one is aimed at nothing and fails.
    let _ = run(&browser, &["download", "--selector", SWEEPING_TARGET, "--timeout", "1", "--json"]);

    assert!(
        !planted.exists(),
        "a transfer directory whose process is gone survived an arming: {planted:?}"
    );
    assert_nothing_outlives_its_process(&browser, &before);
}

/// A transfer that begins and does not finish is not a file. The bytes on disk are a prefix, so
/// nothing is moved into place and the response says which of the two it was — that is the whole
/// difference between "raise the wait" and "the click was wrong".
#[test]
fn a_transfer_still_running_when_the_wait_ends_writes_nothing() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-dl-slow");
    let browser = guard.name().to_string();
    let out_dir = unique_temp_dir("dl-slow");
    let server = SlowServer::start();
    let before = incoming_dirs();

    let root = server.url("/");
    assert!(run(&browser, &["goto", &root]).status.success());

    let partial = out_dir.join("slow.bin");
    let output = run(
        &browser,
        &[
            "download",
            "--selector",
            "#slow",
            "--out",
            partial.to_str().unwrap(),
            "--timeout",
            "2",
            "--json",
        ],
    );
    assert!(output.status.success(), "a delivered click is not an error");
    let value = json_of(&output);
    assert_eq!(value["downloaded"], false, "{value}");
    assert_eq!(value["suggested_filename"], "slow.bin", "the file is named even unfinished: {value}");
    assert!(value["message"].as_str().unwrap().contains("incomplete"), "{value}");
    assert!(
        value["hint"].as_str().unwrap().contains("--timeout"),
        "the recovery is a longer wait, not another click: {value}"
    );
    assert!(!partial.exists(), "an unfinished transfer wrote {partial:?}");

    drop(server);
    assert_nothing_outlives_its_process(&browser, &before);
    std::fs::remove_dir_all(out_dir).unwrap();
}

/// Pipe and batch reach the same code as the CLI, so the shape has to match. This is the check
/// that they were wired to the one entry point rather than to a copy of it.
#[test]
fn pipe_and_batch_report_a_click_download_the_same_way() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-dl-modes");
    let browser = guard.name().to_string();
    let out_dir = unique_temp_dir("dl-modes");
    let page = common::fixture_url("download_click.html");

    let piped = out_dir.join("piped.csv");
    let script = format!(
        "{}\n{}\n",
        serde_json::json!({"cmd": "goto", "url": page}),
        serde_json::json!({"cmd": "download", "selector": "#blob-link", "out": piped}),
    );
    let mut child = Command::new(binary())
        .args(["--browser", &browser, "pipe"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn pipe");
    child.stdin.as_mut().unwrap().write_all(script.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let last = String::from_utf8_lossy(&output.stdout)
        .lines()
        .last()
        .map(str::to_string)
        .expect("a response per command");
    let value: serde_json::Value = serde_json::from_str(&last).unwrap();
    assert_eq!(value["downloaded"], true, "{value}");
    assert_eq!(value["via"], "click", "{value}");
    assert_eq!(value["path"], piped.to_str().unwrap(), "{value}");
    assert_eq!(std::fs::read_to_string(&piped).unwrap(), "id,name\n1,ada\n2,grace\n");

    let batched = out_dir.join("batched.csv");
    let commands =
        serde_json::json!([{"cmd": "download", "selector": "#js-download", "out": batched}])
            .to_string();
    let mut child = Command::new(binary())
        .args(["--browser", &browser, "batch", "--json"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("spawn batch");
    child.stdin.as_mut().unwrap().write_all(commands.as_bytes()).unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().unwrap();
    let value: serde_json::Value =
        serde_json::from_str(String::from_utf8_lossy(&output.stdout).trim()).unwrap();
    let first = &value["results"][0];
    assert_eq!(first["downloaded"], true, "{value}");
    assert_eq!(first["via"], "click", "{value}");
    assert_eq!(std::fs::read_to_string(&batched).unwrap(), "id,name\n1,ada\n2,grace\n");

    std::fs::remove_dir_all(out_dir).unwrap();
}

/// The URL path is untouched, and the two mechanisms are told apart on the response rather than
/// left for the caller to infer from which fields are present.
#[test]
fn the_url_path_still_fetches_and_says_which_mechanism_it_used() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("test-dl-fetch");
    let browser = guard.name().to_string();
    let out_dir = unique_temp_dir("dl-fetch");
    let server = SlowServer::start();

    let root = server.url("/");
    assert!(run(&browser, &["goto", &root]).status.success());
    let fetched = out_dir.join("fetched.txt");
    let url = server.url("/quick");
    let output = run(&browser, &["download", &url, "--out", fetched.to_str().unwrap(), "--json"]);
    assert!(output.status.success(), "stderr={}", String::from_utf8_lossy(&output.stderr));
    let value = json_of(&output);
    assert_eq!(value["via"], "fetch", "{value}");
    assert_eq!(value["downloaded"], true, "{value}");
    assert_eq!(value["bytes"], 5, "{value}");
    assert_eq!(std::fs::read_to_string(&fetched).unwrap(), "quick");

    drop(server);
    std::fs::remove_dir_all(out_dir).unwrap();
}

/// Two ways to name the same thing is a caller who has not decided; picking for them would
/// silently ignore half the invocation.
///
/// Run against an empty `HOME`, which is the state of a CI runner and the state this test used
/// to pass by accident: `Target::parse` sat in the `Command::Download` arm of `run.rs`, after
/// the session and the CDP client were resolved, so on a developer machine the caller got the
/// message about their argument and on a fresh one they got "No browser session 'default'" —
/// advice for a problem they did not have. Asserting the absence of that sentence is the point
/// of the test; accepting either message would freeze the bug in place.
#[test]
fn download_refuses_an_ambiguous_or_absent_target() {
    let home = unique_temp_dir("download-target");
    let refuse = |args: &[&str]| {
        let out = Command::new(binary()).args(args).env("HOME", &home).output().expect("run");
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        assert_eq!(out.status.code(), Some(1), "{args:?}: {stderr}");
        assert!(
            !stderr.contains("browser session"),
            "{args:?} answered about a browser, not about its arguments: {stderr}"
        );
        stderr
    };

    // Naming no target at all. The three ways to name one are listed, in the syntax that would
    // have worked — clap's own rendering of the `download_target` group.
    let none = refuse(&["download"]);
    for form in ["<URL", "--uid", "--selector"] {
        assert!(none.contains(form), "the refusal never names {form}: {none}");
    }

    // Naming two. Both spellings of the ambiguity, since the group covers the positional and
    // the two flags alike.
    for args in [
        &["download", "https://example.com/f.csv", "--selector", "#a"][..],
        &["download", "--uid", "n1", "--selector", "#a"][..],
    ] {
        let both = refuse(args);
        assert!(both.contains("cannot be used with"), "{args:?}: {both}");
    }

    std::fs::remove_dir_all(&home).ok();
}

/// A server that answers an attachment slowly enough that a 2 s wait cannot see the end of it,
/// plus a page linking to it. `Content-Disposition` is what turns the navigation into a download,
/// which is the shape a real "Export" endpoint takes — the blob fixture cannot produce it.
struct SlowServer {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl SlowServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // The listener is non-blocking and macOS hands that down to the accepted
                        // socket: without this the first `read` returns WouldBlock, the request
                        // line reads as empty, and every path falls through to the catch-all —
                        // which is a server answering the wrong body, not a test failure to chase.
                        stream.set_nonblocking(false).unwrap();
                        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                        let mut request = [0_u8; 8192];
                        let size = stream.read(&mut request).unwrap_or(0);
                        let path = String::from_utf8_lossy(&request[..size])
                            .lines()
                            .next()
                            .unwrap_or("")
                            .split_whitespace()
                            .nth(1)
                            .unwrap_or("/")
                            .to_string();
                        match path.as_str() {
                            "/slow" => {
                                // Headers announce 4096 bytes; only 16 ever arrive, and not
                                // quickly. Chrome reports downloadWillBegin on the headers and
                                // never a `completed`.
                                let _ = stream.write_all(
                                    b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                                      Content-Disposition: attachment; filename=\"slow.bin\"\r\n\
                                      Content-Length: 4096\r\nConnection: close\r\n\r\n",
                                );
                                let _ = stream.flush();
                                for _ in 0..8 {
                                    if thread_stop.load(Ordering::Relaxed) {
                                        break;
                                    }
                                    let _ = stream.write_all(b"xx");
                                    let _ = stream.flush();
                                    std::thread::sleep(Duration::from_millis(500));
                                }
                            }
                            "/quick" => {
                                let _ = stream.write_all(
                                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                                      Content-Length: 5\r\nConnection: close\r\n\r\nquick",
                                );
                            }
                            _ => {
                                let body = b"<html><body><a id=\"slow\" href=\"/slow\">slow</a>\
                                             <main>slow download fixture</main></body></html>";
                                let head = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                    body.len()
                                );
                                let _ = stream.write_all(head.as_bytes());
                                let _ = stream.write_all(body);
                            }
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            }
        });
        Self { addr, stop, thread: Some(thread) }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

impl Drop for SlowServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
