//! `goto` says where it actually landed, not just where it ended up.
//!
//! The failure this pins down: an expired session bounces `/orders` to `/login?next=/orders`
//! and the old response — `{url, title}` — was shaped exactly like a successful load. Every
//! command after it then ran against a login wall, blind.
//!
//! Two redirect mechanisms are exercised on purpose. A server-side `302` (the local fixture
//! server below) is followed before the load event, so `location.href` is already final and
//! the Navigation Timing entry reports the status of the LAST hop — 200, not 302. A
//! client-side `<meta http-equiv=refresh>` (`goto_meta_redirect.html`) happens *after* the
//! load event, so it only shows up because the settle probe outlives it; that is the one
//! that would regress silently if the probe stopped waiting.

use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

mod common;
use common::TestBrowser;

fn binary() -> std::path::PathBuf {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path
}

fn run(browser: &str, args: &[&str]) -> Output {
    Command::new(binary())
        .args(["--browser", browser])
        .args(args)
        .output()
        .expect("run chrome-agent")
}

fn run_json(browser: &str, args: &[&str]) -> Value {
    let output = run(browser, args);
    assert!(
        output.status.success(),
        "chrome-agent {args:?} failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!("expected JSON, got {:?}: {e}", String::from_utf8_lossy(&output.stdout))
    })
}

/// Feed commands to `pipe` (or `batch`) on stdin and collect the JSON lines.
fn run_piped(browser: &str, mode: &str, stdin_text: &str, timeout: Duration) -> Vec<Value> {
    let mut child = Command::new(binary())
        .args(["--browser", browser, mode])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn chrome-agent");
    {
        let mut stdin = child.stdin.take().expect("stdin");
        stdin.write_all(stdin_text.as_bytes()).expect("write commands");
    }
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("poll chrome-agent").is_some() {
            let output = child.wait_with_output().expect("collect output");
            assert!(
                output.status.success(),
                "{mode} failed: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| serde_json::from_str(line).expect("JSON response line"))
                .collect();
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().expect("collect timed-out output");
            panic!(
                "{mode} timed out after {timeout:?}: stdout={} stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}


struct Server {
    addr: SocketAddr,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// A page with enough prose to survive Readability's 200-char minimum, so that
/// `navigate_and_read` can be tested on the same fixtures. A one-word body made it fail with
/// "not an article" before it could report where it landed.
fn page(title: &str) -> Vec<u8> {
    let filler = "This fixture exists so the reader has something to return. ".repeat(6);
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>{title}</title>\
         <article><h1>{title}</h1><p>{filler}</p><p>{filler}</p></article>"
    );
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    [headers.as_bytes(), body.as_bytes()].concat()
}

fn found(location: &str) -> Vec<u8> {
    format!("HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .into_bytes()
}

/// An arbitrary status with an arbitrary body. The status is the point for the `serving`
/// tests: a 403 and a 200 carrying the same document must not read the same.
fn respond(status: &str, body: &str) -> Vec<u8> {
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    [headers.as_bytes(), body.as_bytes()].concat()
}

/// The F5 ASM refusal, as `cnrs.fr` served it: HTTP 200, a title, ~150 characters of text,
/// and one link that goes nowhere but back. Nothing here says "blocked" except the prose.
const WAF_REFUSAL: &str = "<!doctype html><html><head><title>Request Rejected</title></head>\
     <body>The requested URL was rejected. Please consult with your administrator.<br><br>\
     Your support ID is: 18003791503210196320<br><br>\
     <a href='javascript:history.back();'>[Go Back]</a></body></html>";

/// A Cloudflare interstitial, as `nowsecure.nl` served it: HTTP 200, the hostname as the
/// title, and a challenge widget as the only thing in the document.
///
/// The `<iframe>` carries an EMPTY `src` on purpose — that is what the live page reports.
/// Turnstile creates an `about:blank` frame and injects into it, so the only vendor-hosted URL
/// in the document is the script. A fixture with a plausible-looking frame URL would have
/// passed while the real site did not.
const CLOUDFLARE_INTERSTITIAL: &str = "<!doctype html><html><head><title>fixture.example</title>\
     <script src=\"https://challenges.cloudflare.com/turnstile/v0/api.js\"></script></head>\
     <body><iframe src=\"\" title=\"Widget containing a Cloudflare security challenge\"></iframe></body></html>";

/// A `DataDome` device check, as `leboncoin.fr` served it: HTTP 403 *and* a challenge frame.
/// Both facts are true and they suggest opposite recoveries, which is why they are ranked.
const DATADOME_403: &str = "<!doctype html><html><head><title>leboncoin</title></head>\
     <body><iframe src=\"https://geo.captcha-delivery.com/captcha/?initialCid=x&amp;cid=y\" \
     title=\"DataDome Device Check\"></iframe></body></html>";

/// The false positive that costs most: a real login form that happens to carry a Turnstile
/// widget. The challenge frame is present and the page is perfectly usable.
const LOGIN_WITH_TURNSTILE: &str = "<!doctype html><html><head><title>Sign in</title>\
     <script src=\"https://challenges.cloudflare.com/turnstile/v0/api.js\"></script></head>\
     <body><form action=\"/session\"><label>Email <input name=email></label>\
     <label>Password <input name=password type=password></label>\
     <iframe src=\"\"></iframe>\
     <input type=hidden name=\"cf-turnstile-response\" value=\"\">\
     <button type=submit>Sign in</button></form>\
     <a href=\"/help\">Need help?</a></body></html>";

/// A 404 a site actually built: a nav, a search box, somewhere to go. The status is the news
/// here, not the shape of the document.
const USABLE_404: &str = "<!doctype html><html><head><title>Not found</title></head>\
     <body><nav><a href=\"/\">Home</a> <a href=\"/help\">Help</a></nav><h1>404</h1>\
     <p>We could not find that page.</p><form><input name=q><button>Search</button></form></body></html>";

/// The other false positive: prose with no link, no form and no button. Nothing to act on and
/// nothing wrong with it — this is what keeps the rule from firing on every plain article.
fn prose_page() -> String {
    let filler = "This page is a plain article with no links and no form controls at all, which \
                  is a perfectly ordinary and perfectly usable thing for a page to be. "
        .repeat(6);
    format!(
        "<!doctype html><html><head><title>Prose</title></head><body><article><h1>An article</h1>\
         <p>{filler}</p></article></body></html>"
    )
}

/// Answer one connection.
///
/// A connection that carries no request line gets no response at all: Chrome preconnects,
/// and answering those with a 404 made an identical fixture server flake as
/// `net::ERR_HTTP_RESPONSE_CODE_FAILURE` on the navigation itself.
fn serve(mut stream: std::net::TcpStream) {
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => request.extend_from_slice(&chunk[..n]),
        }
    }
    let first_line = String::from_utf8_lossy(&request).lines().next().unwrap_or("").to_string();
    let Some(path) = first_line.split_whitespace().nth(1) else {
        return;
    };
    let response = match path {
        "/clean" => page("Clean landing"),
        // The auth bounce: the whole point of the feature.
        "/orders" => found("/login?next=/orders"),
        "/login?next=/orders" | "/login" => page("Sign in"),
        // A redirect with nothing to do with authentication.
        "/moved" => found("/settled"),
        "/settled" => page("Settled page"),
        // A server normalising a directory URL. Under the documented rule this is NOT a
        // redirect, even though the wire carried a 302.
        "/dir" => found("/dir/"),
        "/dir/" => page("Directory index"),
        // What the server sends when it will not serve the page — the `serving` fixtures.
        "/waf" => respond("200 OK", WAF_REFUSAL),
        "/challenge" => respond("200 OK", CLOUDFLARE_INTERSTITIAL),
        "/datadome" => respond("403 Forbidden", DATADOME_403),
        "/turnstile" => respond("200 OK", LOGIN_WITH_TURNSTILE),
        "/notfound" => respond("404 Not Found", USABLE_404),
        "/boom" => respond(
            "500 Internal Server Error",
            "<!doctype html><html><head><title>Error</title></head><body><h1>500</h1></body></html>",
        ),
        "/prose" => respond("200 OK", &prose_page()),
        _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    };
    let _ = stream.write_all(&response);
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::spawn(move || {
            let mut workers: Vec<JoinHandle<()>> = Vec::new();
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    // One thread per connection: serving in the accept loop makes the next
                    // request wait out a speculative socket's 2s read timeout.
                    Ok((stream, _)) => workers.push(std::thread::spawn(move || serve(stream))),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            }
            for worker in workers {
                let _ = worker.join();
            }
        });
        Self { addr, stop, thread: Some(thread) }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

/// A status is reported only when it could have come off the wire. `0` — what a `file://`
/// document and a cross-origin one report — is an absence, and serialising it as a status
/// would be an invented answer.
fn assert_status_plausible_or_absent(landed: &Value) {
    match landed.get("http_status") {
        None | Some(Value::Null) => {}
        Some(status) => {
            let code = status.as_u64().unwrap_or_else(|| panic!("http_status not a number: {status}"));
            assert!(
                (100..=599).contains(&code),
                "http_status {code} is not an HTTP status; it must be absent instead"
            );
        }
    }
}

#[test]
fn a_clean_landing_reports_no_redirect() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-clean");
    let url = server.url("/clean");
    let response = run_json(guard.name(), &["--json", "goto", &url]);

    let landed = &response["landed"];
    assert_eq!(landed["requested"], url.as_str());
    assert_eq!(landed["final"], url.as_str());
    assert_eq!(landed["redirected"], false);
    assert_eq!(landed["http_status"], 200, "a local 200 must be reported as one");
    assert!(response.get("hint").is_none(), "a clean landing needs no hint: {response}");
}

#[test]
fn the_auth_bounce_is_reported_and_hinted() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-bounce");
    let requested = server.url("/orders");
    let response = run_json(guard.name(), &["--json", "goto", &requested]);

    let landed = &response["landed"];
    assert_eq!(landed["requested"], requested.as_str());
    assert_eq!(landed["final"], server.url("/login?next=/orders").as_str());
    assert_eq!(landed["redirected"], true);
    // Navigation Timing reports the last hop, so a followed 302 lands on 200.
    assert_eq!(landed["http_status"], 200);
    let hint = response["hint"].as_str().unwrap_or_else(|| panic!("no hint on the auth bounce: {response}"));
    assert!(hint.contains("login"), "hint should name what it matched: {hint}");
    assert!(
        hint.contains("guess"),
        "the auth-wall heuristic must be worded as a guess, not a claim: {hint}"
    );
}

#[test]
fn an_ordinary_redirect_carries_no_auth_wall_hint() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-moved");
    let response = run_json(guard.name(), &["--json", "goto", &server.url("/moved")]);

    let landed = &response["landed"];
    assert_eq!(landed["redirected"], true);
    assert_eq!(landed["final"], server.url("/settled").as_str());
    assert!(
        response.get("hint").is_none(),
        "a redirect to /settled says nothing about authentication: {response}"
    );
}

/// The documented rule, checked against a real server rather than only against the unit
/// tests: a 302 that only adds the trailing slash is not a redirect worth reporting.
#[test]
fn a_directory_slash_redirect_is_not_reported_as_one() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-slash");
    let response = run_json(guard.name(), &["--json", "goto", &server.url("/dir")]);

    let landed = &response["landed"];
    assert_eq!(landed["final"], server.url("/dir/").as_str());
    assert_eq!(
        landed["redirected"], false,
        "one trailing slash is the same resource: {landed}"
    );
}

/// The client-side case: the load event fires on the bouncing document, so this is only
/// visible because the settle probe waits past it.
#[test]
fn a_meta_refresh_redirect_is_reported() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("landed-meta");
    let requested = common::fixture_url("goto_meta_redirect.html");
    let response = run_json(guard.name(), &["--json", "goto", &requested]);

    let landed = &response["landed"];
    assert_eq!(landed["requested"], requested.as_str());
    assert_eq!(landed["final"], common::fixture_url("goto_redirect_target.html").as_str());
    assert_eq!(landed["redirected"], true);
    // Whatever the page says, or nothing. Measured on Chrome 151: a `file://` document
    // reports `responseStatus: 200`, so this is deliberately not asserted absent — the
    // field is what the browser answered, and the only rule is that a 0 becomes an absence.
    assert_status_plausible_or_absent(landed);
}

/// One shape in all three modes. The CLI, pipe and batch each assemble their own response,
/// and this is the property that stops them drifting.
#[test]
fn cli_pipe_and_batch_report_the_same_landing() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let requested = server.url("/orders");

    let cli_guard = TestBrowser::new("landed-parity-cli");
    let cli = run_json(cli_guard.name(), &["--json", "goto", &requested]);

    let pipe_guard = TestBrowser::new("landed-parity-pipe");
    let pipe_lines = run_piped(
        pipe_guard.name(),
        "pipe",
        &format!("{}\n", json!({"cmd": "goto", "url": requested})),
        Duration::from_secs(45),
    );
    let piped = pipe_lines.last().expect("a pipe response");

    let batch_guard = TestBrowser::new("landed-parity-batch");
    // `batch` never opens a browser, so it needs the session to exist first.
    run_json(batch_guard.name(), &["--json", "goto", &server.url("/clean")]);
    let batch_lines = run_piped(
        batch_guard.name(),
        "batch",
        &format!("{}\n", json!([{"cmd": "goto", "url": requested}])),
        Duration::from_secs(45),
    );
    let batched = &batch_lines.last().expect("a batch response")["results"][0];

    for (mode, response) in [("cli", &cli), ("pipe", piped), ("batch", batched)] {
        let landed = &response["landed"];
        assert_eq!(landed["requested"], requested.as_str(), "{mode}");
        assert_eq!(landed["final"], server.url("/login?next=/orders").as_str(), "{mode}");
        assert_eq!(landed["redirected"], true, "{mode}");
        assert_status_plausible_or_absent(landed);
        assert!(response["hint"].as_str().is_some_and(|h| h.contains("login")), "{mode}: {response}");
    }
    assert_eq!(cli["landed"], piped["landed"]);
    assert_eq!(cli["landed"], batched["landed"]);
}

/// `navigate_and_read` navigates to a caller-supplied URL and hands back prose. Without a
/// landing it returns a login page's text as if it were the article that was asked for.
#[test]
fn navigate_and_read_reports_its_landing_too() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-nav-read");
    let requested = server.url("/orders");
    let lines = run_piped(
        guard.name(),
        "pipe",
        &format!("{}\n", json!({"cmd": "navigate_and_read", "url": requested})),
        Duration::from_secs(45),
    );
    let response = lines.last().expect("a navigate_and_read response");

    let landed = &response["landed"];
    assert_eq!(landed["requested"], requested.as_str(), "{response}");
    assert_eq!(landed["final"], server.url("/login?next=/orders").as_str());
    assert_eq!(landed["redirected"], true);
    assert!(response["hint"].as_str().is_some_and(|h| h.contains("login")), "{response}");
}

/// Text mode is what a person reads. It stays quiet when the navigation went where it was
/// told, and speaks up when it did not.
#[test]
fn text_mode_names_the_redirect_and_stays_quiet_otherwise() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-text");

    let bounced = run(guard.name(), &["goto", &server.url("/orders")]);
    let bounced_out = String::from_utf8_lossy(&bounced.stdout).to_string();
    assert!(bounced.status.success(), "goto failed: {bounced_out}");
    assert!(
        bounced_out.contains(&format!("redirected from {}", server.url("/orders"))),
        "text mode must name where the caller was sent: {bounced_out}"
    );
    assert!(bounced_out.contains("hint:"), "the auth-wall guess belongs in text mode too: {bounced_out}");

    let clean = run(guard.name(), &["goto", &server.url("/clean")]);
    let clean_out = String::from_utf8_lossy(&clean.stdout).to_string();
    assert!(clean.status.success(), "goto failed: {clean_out}");
    assert!(
        !clean_out.contains("redirected"),
        "a caller who typed the URL does not need it read back: {clean_out}"
    );
}

// ---------------------------------------------------------------------------
// `serving`: what answered, as opposed to where the navigation landed.
//
// Every case below was measured on a real site first and is reproduced here byte for byte:
// cnrs.fr (F5 ASM, HTTP 200), nowsecure.nl (Cloudflare, HTTP 200), leboncoin.fr (DataDome,
// HTTP 403). The two `page` cases are the false positives the rule must not produce.
// ---------------------------------------------------------------------------

/// Defect 1: a refusal served with a 200 used to be shaped exactly like the page asked for.
#[test]
fn a_refusal_served_with_200_is_not_reported_as_the_page() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-waf");
    let response = run_json(guard.name(), &["--json", "goto", &server.url("/waf")]);

    let landed = &response["landed"];
    assert_eq!(landed["http_status"], 200, "the status really is 200: {landed}");
    assert_eq!(
        landed["serving"], "nothing_actionable",
        "a document with one javascript: link and 150 characters of text offers nothing to act on: {response}"
    );
    let hint = response["hint"].as_str().unwrap_or_else(|| panic!("no hint: {response}"));
    assert!(
        hint.contains(&format!("chrome-agent --browser {} inspect", guard.name())),
        "the recovery is one command, aimed at this session's browser: {hint}"
    );
    // The word is a measurement of the document, never a claim that the caller was blocked.
    assert!(
        hint.contains("not a claim you were blocked"),
        "the hint must refuse to assert a block: {hint}"
    );
    assert_eq!(response["ok"], true, "the navigation happened; the page is there to read");
}

/// Defect 1, the other half: an interstitial whose vendor the tool can name.
#[test]
fn a_challenge_interstitial_names_the_vendor_that_served_it() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-challenge");
    let response = run_json(guard.name(), &["--json", "goto", &server.url("/challenge")]);

    let landed = &response["landed"];
    assert_eq!(landed["serving"], "challenge", "{response}");
    assert_eq!(landed["challenge_from"], "challenges.cloudflare.com", "{landed}");
    let hint = response["hint"].as_str().unwrap_or_else(|| panic!("no hint: {response}"));
    assert!(hint.contains("--connect"), "the documented route past a fingerprint check: {hint}");
}

/// Defect 2: a 4xx used to pass for a successful load with no judgement on it at all.
#[test]
fn a_4xx_is_reported_as_an_error_and_hinted() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-notfound");
    let response = run_json(guard.name(), &["--json", "goto", &server.url("/notfound")]);

    let landed = &response["landed"];
    assert_eq!(landed["http_status"], 404);
    assert_eq!(landed["serving"], "error", "{response}");
    let hint = response["hint"].as_str().unwrap_or_else(|| panic!("no hint: {response}"));
    assert!(hint.contains("404"), "the hint has to name the code it is about: {hint}");
}

#[test]
fn a_5xx_is_reported_as_an_error_too() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-boom");
    let response = run_json(guard.name(), &["--json", "goto", &server.url("/boom")]);
    assert_eq!(response["landed"]["http_status"], 500);
    assert_eq!(response["landed"]["serving"], "error", "{response}");
    let hint = response["hint"].as_str().unwrap_or_else(|| panic!("no hint: {response}"));
    assert!(hint.contains("500"), "{hint}");
}

/// Both facts hold at once and they suggest opposite recoveries: 403 reads as "find
/// credentials", the frame reads as "this Chrome is being fingerprinted". The frame wins and
/// the status stays on the response beside it.
#[test]
fn a_challenge_outranks_the_status_it_arrived_with() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-datadome");
    let response = run_json(guard.name(), &["--json", "goto", &server.url("/datadome")]);

    let landed = &response["landed"];
    assert_eq!(landed["serving"], "challenge", "{response}");
    assert_eq!(landed["challenge_from"], "geo.captcha-delivery.com", "{landed}");
    assert_eq!(landed["http_status"], 403, "the status is not dropped: {landed}");
}

/// The expensive false positive. A Turnstile widget sits on a large share of the login forms
/// on the web; calling those pages blocked would make an agent give up on a page it could
/// have filled in.
#[test]
fn a_challenge_widget_on_a_usable_page_is_not_a_block() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-turnstile");
    let response = run_json(guard.name(), &["--json", "goto", &server.url("/turnstile")]);

    let landed = &response["landed"];
    assert_eq!(
        landed["serving"], "page",
        "a form, a button and a link are things to act on: {response}"
    );
    // The frame is still a measured fact, and it explains a lot when the submit fails.
    assert_eq!(landed["challenge_from"], "challenges.cloudflare.com", "{landed}");
    assert!(response.get("hint").is_none(), "nothing to warn about: {response}");
}

/// The other false positive: an article with no link, no button and no input.
#[test]
fn prose_with_nothing_to_click_is_still_the_page() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-prose");
    let response = run_json(guard.name(), &["--json", "goto", &server.url("/prose")]);
    assert_eq!(response["landed"]["serving"], "page", "{response}");
    assert!(response.get("hint").is_none(), "{response}");
}

#[test]
fn an_ordinary_page_says_page_and_carries_no_challenge_field() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-serving-clean");
    let response = run_json(guard.name(), &["--json", "goto", &server.url("/clean")]);
    let landed = &response["landed"];
    assert_eq!(landed["serving"], "page", "{response}");
    assert!(
        landed.get("challenge_from").is_none(),
        "the field exists only when a frame was found: {landed}"
    );
}

/// One shape in all three modes, for the new field as for the old ones.
#[test]
fn cli_pipe_and_batch_report_the_same_serving() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let requested = server.url("/datadome");

    let cli_guard = TestBrowser::new("landed-serving-cli");
    let cli = run_json(cli_guard.name(), &["--json", "goto", &requested]);

    let pipe_guard = TestBrowser::new("landed-serving-pipe");
    let pipe_lines = run_piped(
        pipe_guard.name(),
        "pipe",
        &format!("{}\n", json!({"cmd": "goto", "url": requested})),
        Duration::from_secs(45),
    );
    let piped = pipe_lines.last().expect("a pipe response");

    let batch_guard = TestBrowser::new("landed-serving-batch");
    run_json(batch_guard.name(), &["--json", "goto", &server.url("/clean")]);
    let batch_lines = run_piped(
        batch_guard.name(),
        "batch",
        &format!("{}\n", json!([{"cmd": "goto", "url": requested}])),
        Duration::from_secs(45),
    );
    let batched = &batch_lines.last().expect("a batch response")["results"][0];

    for (mode, response) in [("cli", &cli), ("pipe", piped), ("batch", batched)] {
        assert_eq!(response["landed"]["serving"], "challenge", "{mode}: {response}");
        assert!(response["hint"].as_str().is_some(), "{mode}: {response}");
    }
    assert_eq!(cli["landed"], piped["landed"]);
    assert_eq!(cli["landed"], batched["landed"]);
}

/// `navigate_and_read` hands back prose. Without this it returns a refusal notice's text as
/// if it were the article that was asked for.
#[test]
fn navigate_and_read_reports_what_was_served() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-nav-read-serving");
    let lines = run_piped(
        guard.name(),
        "pipe",
        &format!("{}\n", json!({"cmd": "navigate_and_read", "url": server.url("/prose")})),
        Duration::from_secs(45),
    );
    let response = lines.last().expect("a navigate_and_read response");
    assert_eq!(response["landed"]["serving"], "page", "{response}");
}

/// Text mode is what a person judges the tool on. It says nothing on a clean load and names
/// what was served otherwise.
#[test]
fn text_mode_names_what_was_served() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-serving-text");

    let blocked = run(guard.name(), &["goto", &server.url("/datadome")]);
    let out = String::from_utf8_lossy(&blocked.stdout).to_string();
    assert!(blocked.status.success(), "{out}");
    assert!(out.contains("serving: challenge"), "{out}");
    assert!(out.contains("geo.captcha-delivery.com"), "the evidence, not just the word: {out}");
    assert!(out.contains("hint:"), "{out}");

    let clean = run(guard.name(), &["goto", &server.url("/clean")]);
    let clean_out = String::from_utf8_lossy(&clean.stdout).to_string();
    assert!(
        !clean_out.contains("serving:"),
        "a page that answered normally gets no line: {clean_out}"
    );
}

/// Defect 3: five distinct network failures shared one sentence that stated no fact and named
/// no command. DNS is the one reachable without a network.
#[test]
fn a_dns_failure_is_told_apart_from_every_other_navigation_failure() {
    if !common::browser_ready() {
        return;
    }
    let guard = TestBrowser::new("landed-dns");
    let output = run(guard.name(), &["--json", "goto", "https://not-a-real-host.invalid"]);
    let response: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|e| panic!("expected JSON, got {:?}: {e}", String::from_utf8_lossy(&output.stdout)));

    assert_eq!(response["ok"], false);
    let hint = response["hint"].as_str().unwrap_or_else(|| panic!("no hint: {response}"));
    assert!(
        !hint.contains("Check the URL is valid and the page is reachable"),
        "one sentence for five causes: {hint}"
    );
    assert!(hint.contains("not-a-real-host.invalid"), "the hint knows the host: {hint}");
    assert!(
        hint.to_lowercase().contains("dns") || hint.contains("no address"),
        "the fact is that the name did not resolve: {hint}"
    );
}

/// `goto` must not enable the Network domain to learn the status: `--stealth` skips
/// `Runtime.enable` and avoids the Network domain on purpose, and Navigation Timing is the
/// path that keeps both promises.
#[test]
fn stealth_still_reports_a_landing() {
    if !common::browser_ready() {
        return;
    }
    let server = Server::start();
    let guard = TestBrowser::new("landed-stealth");
    let response = run_json(guard.name(), &["--stealth", "--json", "goto", &server.url("/orders")]);

    let landed = &response["landed"];
    assert_eq!(landed["redirected"], true);
    assert_eq!(landed["final"], server.url("/login?next=/orders").as_str());
    assert_status_plausible_or_absent(landed);
}
