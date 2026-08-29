use clap::{Parser, Subcommand};

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("invalid positive integer: {value}"))?;
    if parsed == 0 {
        Err("value must be greater than zero".to_string())
    } else {
        Ok(parsed)
    }
}

const CLI_LONG_ABOUT: &str = "\
chrome-agent — browser automation for AI agents. Controls Chrome via CDP.\n\
Single binary, zero runtime dependencies. Named pages persist between invocations.\n\
\n\
Reading a response: ok:true means the command ran, not that the page complied.\n\
Read `verdict` (and `value.verbatim` after a fill) before reporting success; branch on\n\
`next` — never repeat an action on `unknown`, because the first one may have landed.\n\
\n\
Use --stealth to bypass bot detection (Cloudflare, Turnstile).\n\
Use --copy-cookies to access sites where you're already logged in (X.com, Gmail).\n\
\n\
Workflow: inspect → read uids → act (click/fill) → assert.\n\
Use --inspect on action commands to combine action + observation in one call.";

const CLI_AFTER_LONG_HELP: &str = include_str!("../llm-guide.txt");

/// The flags that must still precede the verb, and why each one could not be `global = true`.
///
/// Every other flag on `Cli` is global, so it parses on either side of the subcommand. These two
/// are redeclared by some commands with their own meaning and their own defaults — `--timeout` by
/// `wait` (10 s) and `download` (30 s), `--max-depth` by the twelve action commands that take
/// `--inspect`. A global arg propagates into EVERY subcommand, so sharing an id with one of them
/// is a duplicate-argument panic at startup, not a parse error. Unifying them would mean giving
/// `wait` the global 30 s default, which is a real regression: every `wait` that gives up after
/// 10 s today would hang three times longer. A correct default is worth more than a parsing
/// convenience.
///
/// So the position rule stays, and `hints::flag_position_hint` makes the failure teach it. The
/// second element is the clause that explains this flag specifically; the harm was never the
/// rule, it was clap's `tip: to pass '--timeout' as a value, use '-- --timeout'` — advice for a
/// different problem entirely, on the caller's first attempt.
pub const BEFORE_VERB_ONLY: &[(&str, &str)] = &[
    (
        "--timeout",
        "`wait` and `download` declare their own --timeout with different defaults, so this one \
         is not global",
    ),
    (
        "--max-depth",
        "the action commands declare their own --max-depth for `--inspect`, so this one is not \
         global",
    ),
];

/// Every flag on this struct except the two in [`BEFORE_VERB_ONLY`] is `global = true`, so it
/// parses on either side of the subcommand.
///
/// `chrome-agent fill --selector "#micro" "x" --json` used to fail with a raw clap error and the
/// tip "to pass '--json' as a value, use '-- --json'" — advice for a different problem, on the
/// most natural way to reach for the flag. Requiring a global flag to precede the verb is the
/// opposite of the reflex a shell teaches, and the failure lands on the caller's FIRST attempt.
#[derive(Parser)]
#[command(
    name = "chrome-agent",
    version,
    about = "chrome-agent — browser automation for AI agents",
    long_about = CLI_LONG_ABOUT,
    after_long_help = CLI_AFTER_LONG_HELP,
)]
#[allow(clippy::struct_excessive_bools)]
pub struct Cli {
    /// Named browser profile (default: "default")
    #[arg(long, default_value = "default", global = true)]
    pub browser: String,

    /// Connect to existing browser: ws:// URL, http:// URL, or "auto"
    #[arg(long, global = true)]
    pub connect: Option<String>,

    /// Proxy server for a managed browser: http(s)://host:port or socks4/5://host:port
    #[arg(long, global = true)]
    pub proxy_server: Option<String>,

    /// Launch browser with a visible window (default is headless)
    #[arg(long, global = true)]
    pub headed: bool,

    /// Global timeout in seconds for page loads
    #[arg(long, default_value = "30")]
    pub timeout: u64,

    /// Ignore HTTPS certificate errors
    #[arg(long, global = true)]
    pub ignore_https_errors: bool,

    /// Output structured JSON instead of text
    #[arg(long, global = true)]
    pub json: bool,

    /// Stealth mode: 7 anti-detection patches (webdriver, UA, WebGL, input leak, Runtime.enable skipped)
    #[arg(long, global = true)]
    pub stealth: bool,

    /// Max depth for --inspect output (used by goto, click, fill, etc.)
    #[arg(long)]
    pub max_depth: Option<usize>,

    /// What an action command reports after it runs.
    /// `auto` (default) appends what changed on the page. `off` restores the older,
    /// faster behaviour: the action is reported, the page is not re-read.
    #[arg(long, default_value = "auto", value_parser = ["auto", "off"], global = true)]
    pub verdict: String,

    /// Character budget for the change report on an action. 0 removes the cap.
    #[arg(long, default_value = "1200", global = true)]
    pub budget: usize,

    /// What a click/double-click does when the hit test says another element occupies the
    /// point it was aimed at. `dispatch` (default) sends it anyway — what a pointer does —
    /// and names the receiver in `intercepted_by`. `refuse` returns an error and dispatches
    /// nothing.
    #[arg(long, default_value = "dispatch", value_parser = ["dispatch", "refuse"], global = true)]
    pub on_intercept: String,

    /// Copy cookies from your real Chrome profile (uses your logged-in sessions)
    #[arg(long, global = true)]
    pub copy_cookies: bool,

    /// Named page/tab within the browser (default: "default")
    #[arg(long, default_value = "default", global = true)]
    pub page: String,

    /// How to answer JS dialogs (alert/confirm/prompt/beforeunload): accept, dismiss, or manual
    #[arg(long, default_value = "accept", global = true)]
    pub dialog: String,

    /// Text to submit for `prompt()` dialogs when --dialog accept (default: empty)
    #[arg(long, global = true)]
    pub dialog_text: Option<String>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Navigate to a URL — reports `landed` (where you aimed vs where you ended up, and any redirect)
    #[command(alias = "navigate", alias = "open", alias = "go")]
    Goto {
        /// Target URL
        url: String,
        /// Inspect page after navigation
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output (also accepted as global flag)
        #[arg(long)]
        max_depth: Option<usize>,
        /// Wait for a CSS selector to appear after navigation
        #[arg(long)]
        wait_for: Option<String>,
        /// Extra HTTP header "Name: Value" (repeatable) sent with the navigation
        #[arg(long = "header")]
        headers: Vec<String>,
    },

    /// Click an element (uid, CSS, or x,y) — the response says whether it landed and who received the event
    #[command(alias = "tap")]
    Click {
        /// Element uid (e.g. "n47") — omit if using --selector or --xy
        uid: Option<String>,
        /// CSS selector to click
        #[arg(long)]
        selector: Option<String>,
        /// Click at x,y coordinates (e.g. --xy 100,200)
        #[arg(long, value_delimiter = ',')]
        xy: Option<Vec<f64>>,
        /// Inspect page after clicking
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output (also accepted as global flag)
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Fill an input (uid or CSS) — the response reports what the page actually kept, in `value`
    Fill {
        /// Value to fill
        value: String,
        /// Element uid (e.g. "n47") — omit if using --selector
        #[arg(long)]
        uid: Option<String>,
        /// CSS selector to fill
        #[arg(long)]
        selector: Option<String>,
        /// Inspect page after filling
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output (also accepted as global flag)
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Fill multiple form fields at once — one kept-value report per field, not just a count
    #[command(name = "fill-form")]
    FillForm {
        /// uid=value pairs (e.g. "e5=hello" "e7=world")
        pairs: Vec<String>,
        /// Inspect page after filling
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output (also accepted as global flag)
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Extract visible text from the page or an element
    Text {
        /// Element uid to extract text from (default: entire page)
        uid: Option<String>,
        /// CSS selector to extract text from (e.g. "article", ".content")
        #[arg(long)]
        selector: Option<String>,
        /// Truncate output to N characters (appends "..." if truncated)
        #[arg(long)]
        truncate: Option<usize>,
    },

    /// Extract main content using Readability (Mozilla's reader mode)
    Read {
        /// Return cleaned HTML instead of plain text
        #[arg(long)]
        html: bool,
        /// Truncate output to N characters
        #[arg(long)]
        truncate: Option<usize>,
    },

    /// Navigate back in browser history
    Back,

    /// Navigate forward in browser history
    Forward {
        /// Inspect page after navigation
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Double-click an element (uid, CSS, or x,y) — hit-tested, so an interception is named, not silent
    Dblclick {
        /// Element uid
        uid: Option<String>,
        /// CSS selector
        #[arg(long)]
        selector: Option<String>,
        /// Click at x,y coordinates
        #[arg(long, value_delimiter = ',')]
        xy: Option<Vec<f64>>,
        /// Inspect page after double-clicking
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Select a dropdown option by value or visible text — refuses if the page reverts the selection
    Select {
        /// Value or visible text to select
        value: String,
        /// Element uid of the <select>
        #[arg(long)]
        uid: Option<String>,
        /// CSS selector of the <select>
        #[arg(long)]
        selector: Option<String>,
        /// Inspect page after selecting
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Ensure a checkbox/radio is checked — idempotent, state read back, refuses what it cannot classify
    Check {
        /// Element uid
        uid: Option<String>,
        /// CSS selector
        #[arg(long)]
        selector: Option<String>,
        /// Inspect page after checking
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Ensure a checkbox/radio is unchecked — idempotent, state read back, unchecking a radio is refused
    Uncheck {
        /// Element uid
        uid: Option<String>,
        /// CSS selector
        #[arg(long)]
        selector: Option<String>,
        /// Inspect page after unchecking
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Upload file(s) to a file input — paths are validated before the page is touched
    Upload {
        /// File path(s) to upload
        files: Vec<String>,
        /// Element uid of the file input
        #[arg(long)]
        uid: Option<String>,
        /// CSS selector of the file input
        #[arg(long)]
        selector: Option<String>,
        /// Inspect page after uploading
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Drag an element to another element
    Drag {
        /// Source element uid
        from: String,
        /// Destination element uid
        to: String,
        /// Inspect page after dragging
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output
        #[arg(long)]
        max_depth: Option<usize>,
    },

    /// Take an accessibility tree inspection
    #[command(alias = "snap", alias = "snapshot", alias = "tree")]
    Inspect {
        /// Include ignored/generic nodes
        #[arg(long)]
        verbose: bool,
        /// Maximum tree depth (0 = root only)
        #[arg(long)]
        max_depth: Option<usize>,
        /// Only inspect children of this uid
        #[arg(long)]
        uid: Option<String>,
        /// Only show nodes matching these roles (comma-separated, e.g. "button,link,textbox")
        #[arg(long)]
        filter: Option<String>,
        /// Scroll to load lazy content before inspecting
        #[arg(long)]
        scroll: bool,
        /// Collect N items by scrolling (for virtualized lists like X.com)
        #[arg(long)]
        limit: Option<usize>,
        /// Include href URLs on link nodes
        #[arg(long)]
        urls: bool,
        /// Cap output to N characters (appends a truncation note; keeps context small)
        #[arg(long)]
        max_chars: Option<usize>,
        /// Skip the first K characters of output (paging; use with --max-chars)
        #[arg(long, default_value = "0")]
        offset: usize,
    },

    /// Show what changed since the last inspect
    Diff,

    /// Capture a screenshot
    #[command(alias = "capture")]
    Screenshot {
        /// Output filename (default: timestamped)
        #[arg(long)]
        filename: Option<String>,
        /// Image format: png (default) or jpeg (smaller, use with --quality)
        #[arg(long, default_value = "png")]
        format: String,
        /// JPEG quality 0-100 (ignored for png)
        #[arg(long)]
        quality: Option<u32>,
        /// Downscale so the captured width fits within N CSS pixels (keeps files/tokens small)
        #[arg(long)]
        max_width: Option<u32>,
        /// Capture only the element with this uid
        #[arg(long)]
        uid: Option<String>,
        /// Capture only the element matching this CSS selector
        #[arg(long)]
        selector: Option<String>,
    },

    /// Download a URL to disk, fetched in-page so cookies/auth are preserved
    ///
    /// Click-triggered browser-native downloads are not supported; resolve the
    /// target href (e.g. `inspect --urls`) and pass it here.
    Download {
        /// URL to download (fetched with the page's session)
        url: String,
        /// Output path or filename (default: derived from Content-Disposition/URL into ~/.chrome-agent/tmp)
        #[arg(long)]
        out: Option<String>,
        /// Timeout in seconds
        #[arg(long, default_value = "30")]
        timeout: u64,
        /// Maximum response size in bytes
        #[arg(
            long,
            default_value = "67108864",
            value_parser = parse_positive_usize
        )]
        max_bytes: usize,
    },

    /// Print the current page to a PDF file
    Pdf {
        /// Output filename (default: timestamped)
        #[arg(long)]
        filename: Option<String>,
        /// Landscape orientation
        #[arg(long)]
        landscape: bool,
        /// Include background graphics/colors
        #[arg(long)]
        background: bool,
    },

    /// Auto-extract structured data from repeating page elements (lists, tables, cards)
    Extract {
        /// CSS selector to scope extraction (e.g. "main", ".results")
        #[arg(long)]
        selector: Option<String>,
        /// Max items to extract
        #[arg(long, default_value = "10")]
        limit: usize,
        /// Scroll to load lazy content before extracting (useful for infinite-scroll pages)
        #[arg(long)]
        scroll: bool,
        /// Use accessibility tree instead of DOM (works on React SPAs like X.com)
        #[arg(long)]
        a11y: bool,
    },

    /// Evaluate JavaScript in the page
    #[command(alias = "js", alias = "execute")]
    Eval {
        /// JS expression to evaluate (if --selector, `el` is the matched element)
        expression: String,
        /// CSS selector — the matched element is available as `el` in the expression
        #[arg(long)]
        selector: Option<String>,
    },

    /// Wait for a condition (text, url, selector, or network-idle)
    Wait {
        /// What to wait for: "text", "url", "selector", or "network-idle"
        what: String,
        /// Pattern to match (ignored for network-idle)
        #[arg(default_value = "")]
        pattern: String,
        /// Timeout in seconds
        #[arg(long, default_value = "10")]
        timeout: u64,
        /// For network-idle: required quiet window in milliseconds
        #[arg(long, default_value = "500")]
        idle_ms: u64,
    },

    /// Prove a claim about the page — exit 0 held (the only quotable evidence), 2 did not hold, 1 not checked
    ///
    /// The exit code is the answer, and it distinguishes three outcomes the rest of this
    /// binary collapses into two: 0 the claim held when we looked, 2 it did not (the page
    /// is not in the asserted state), 1 it could not be checked at all — no browser, a
    /// selector that matches nothing, an invalid regex, a CDP timeout. A CI job or a recipe
    /// runner needs 2 and 1 apart: the first is a fact to report, the second a retry.
    Assert {
        #[command(subcommand)]
        what: AssertWhat,
    },

    /// Type text into the focused element (or focus a selector first)
    Type {
        /// Text to type
        text: String,
        /// CSS selector to focus before typing
        #[arg(long)]
        selector: Option<String>,
    },

    /// Press a key (Enter, Tab, Escape, etc.)
    Press {
        /// Key name
        key: String,
    },

    /// Scroll the page or an element into view
    Scroll {
        /// "up", "down", or a uid to scroll into view
        target: String,
        /// Pixels to scroll when using "up" or "down" (default: 500)
        #[arg(long, default_value = "500")]
        px: u64,
    },

    /// Hover over an element by uid
    Hover {
        /// Element uid (e.g. "n47")
        uid: String,
    },

    /// Capture network requests (API responses, XHR, fetch)
    Network {
        /// URL pattern to filter (case-insensitive contains match)
        #[arg(long)]
        filter: Option<String>,
        /// Include response bodies (JSON/text only, truncated to 2000 chars)
        #[arg(long)]
        body: bool,
        /// Capture live traffic for N seconds (default: show already-loaded resources via Performance API)
        #[arg(long)]
        live: Option<u64>,
        /// Max entries to show
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Block requests matching this URL pattern
        #[arg(long)]
        abort: Option<String>,
    },

    /// Show captured console messages and JS errors
    Console {
        /// Filter by level: log, warn, error, info, exception
        #[arg(long)]
        level: Option<String>,
        /// Clear captured messages after reading
        #[arg(long)]
        clear: bool,
        /// Max entries to show
        #[arg(long, default_value = "50")]
        limit: usize,
    },

    /// Replay a recorded session file
    Replay {
        /// Path to the recording file
        file: String,
        /// Variable substitutions (key=value, comma-separated)
        #[arg(long, value_delimiter = ',')]
        vars: Option<Vec<String>>,
    },

    /// Show browsing history
    History {
        /// Filter entries by URL pattern (case-insensitive)
        #[arg(long)]
        filter: Option<String>,
        /// Max entries to show
        #[arg(long, default_value = "20")]
        limit: usize,
    },

    /// Switch execution context to an iframe (or back to main)
    ///
    /// The selector is resolved inside the currently bound frame, so you can
    /// descend into nested iframes. Use "main" to reset to the top document
    /// before switching to a top-level sibling frame.
    Frame {
        /// CSS selector of the iframe, or "main" to return to top-level
        target: String,
    },

    /// Manage explicit device metrics for this named page only
    Emulate {
        #[command(subcommand)]
        action: EmulateAction,
    },

    /// Execute multiple commands from a JSON array on stdin
    Batch {
        /// Stop at the first command that fails (default: run every command)
        #[arg(long)]
        stop_on_error: bool,
    },

    /// Persistent connection mode — read JSON commands from stdin (one per line)
    Pipe,

    /// List open browser tabs
    Tabs,

    /// Close the managed browser
    Close {
        /// Also delete the browser profile (cookies, cache, data)
        #[arg(long)]
        purge: bool,
        /// Delete every profile no session references, no browser holds, and nothing has
        /// touched for a day. The save path removes one per command; this sweeps the backlog.
        #[arg(long)]
        purge_orphans: bool,
        /// Close every running browser no session entry claims. Processes only — the
        /// profiles they leave behind are what --purge-orphans sweeps.
        #[arg(long)]
        orphans: bool,
    },

    /// Show session status
    Status,

    /// Stop the background daemon
    Stop,

    /// Daemon management
    Daemon {
        #[command(subcommand)]
        action: DaemonAction,
    },
}

#[derive(Subcommand)]
pub enum EmulateAction {
    /// Apply and persist explicit metrics for this named page
    Device {
        /// Optional display label reported by `emulate status`
        #[arg(long)]
        label: Option<String>,
        /// Emulated device width in CSS pixels
        #[arg(long)]
        width: u32,
        /// Emulated device height in CSS pixels
        #[arg(long)]
        height: u32,
        /// Device pixel ratio; must be finite and greater than zero
        #[arg(long, default_value = "1")]
        dpr: f64,
        /// Enable Chromium's mobile viewport mode
        #[arg(long)]
        mobile: bool,
        /// Advertise touch support and dispatch clicks as touch taps
        #[arg(long)]
        touch: bool,
        /// Screen orientation; inferred from width and height when omitted
        #[arg(long)]
        orientation: Option<crate::emulation::DeviceOrientation>,
    },
    /// Show requested metrics and values currently observed by the page
    Status,
    /// Clear target overrides and the persisted metrics for this named page
    Reset,
}

#[derive(Subcommand)]
pub enum DaemonAction {
    /// Start the daemon (foreground, used internally)
    Start,
}

/// What `assert` can be asked about the page.
///
/// One subcommand per kind of claim rather than a pile of flags on `assert`: the comparators
/// that make sense differ per kind (a URL is equal or matches a pattern; a page's text is
/// contained or matches), and clap's arg groups can then enforce "exactly one comparator"
/// and "exactly one state" instead of the dispatcher discovering it at run time.
#[derive(Subcommand)]
pub enum AssertWhat {
    /// A form control's value (input, textarea, select)
    #[command(group = clap::ArgGroup::new("value_cmp").required(true).args(["equals", "contains", "matches"]))]
    #[command(group = clap::ArgGroup::new("value_target").required(true).args(["selector", "uid"]))]
    Value {
        /// CSS selector of the control
        #[arg(long)]
        selector: Option<String>,
        /// Element uid of the control (e.g. "n47")
        #[arg(long)]
        uid: Option<String>,
        /// The value the control must hold, exactly
        #[arg(long)]
        equals: Option<String>,
        /// A substring the value must contain
        #[arg(long)]
        contains: Option<String>,
        /// A regular expression the value must match (Rust regex; \d \w \s are ASCII-only)
        #[arg(long)]
        matches: Option<String>,
    },

    /// Visible text — of the whole page, or of one element with --selector/--uid
    #[command(group = clap::ArgGroup::new("text_cmp").required(true).args(["contains", "matches"]))]
    #[command(group = clap::ArgGroup::new("text_target").args(["selector", "uid"]))]
    Text {
        /// CSS selector to scope the text to (default: the whole page)
        #[arg(long)]
        selector: Option<String>,
        /// Element uid to scope the text to
        #[arg(long)]
        uid: Option<String>,
        /// A substring the text must contain
        #[arg(long)]
        contains: Option<String>,
        /// A regular expression the text must match
        #[arg(long)]
        matches: Option<String>,
    },

    /// The current URL
    #[command(group = clap::ArgGroup::new("url_cmp").required(true).args(["equals", "matches"]))]
    Url {
        /// The URL the page must be on, exactly
        #[arg(long)]
        equals: Option<String>,
        /// A regular expression the URL must match
        #[arg(long)]
        matches: Option<String>,
    },

    /// An element's state: checked, selected, enabled or rendered
    #[command(group = clap::ArgGroup::new("state_want").required(true).args(["checked", "unchecked", "selected", "enabled", "disabled", "visible"]))]
    #[command(group = clap::ArgGroup::new("state_target").required(true).args(["selector", "uid"]))]
    State {
        /// CSS selector of the element
        #[arg(long)]
        selector: Option<String>,
        /// Element uid of the element
        #[arg(long)]
        uid: Option<String>,
        /// The checkbox/radio (native or ARIA) must be checked
        #[arg(long)]
        checked: bool,
        /// The checkbox/radio must be unchecked (indeterminate satisfies neither)
        #[arg(long)]
        unchecked: bool,
        /// The <select> must hold this option (by value or visible text)
        #[arg(long)]
        selected: Option<String>,
        /// The control must not be disabled (neither :disabled nor aria-disabled)
        #[arg(long)]
        enabled: bool,
        /// The control must be disabled (:disabled or aria-disabled)
        #[arg(long)]
        disabled: bool,
        /// The element must be rendered, opaque and not visibility:hidden
        #[arg(long)]
        visible: bool,
    },

    /// How many elements a CSS selector matches
    #[command(group = clap::ArgGroup::new("exists_count").args(["count", "min"]))]
    Exists {
        /// CSS selector to count
        #[arg(long)]
        selector: String,
        /// Exactly this many matches (0 asserts absence)
        #[arg(long)]
        count: Option<usize>,
        /// At least this many matches
        #[arg(long)]
        min: Option<usize>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_max_bytes_defaults_and_accepts_an_explicit_limit() {
        let default = Cli::try_parse_from(["chrome-agent", "download", "https://example.com/a"])
            .unwrap();
        let explicit = Cli::try_parse_from([
            "chrome-agent",
            "download",
            "https://example.com/a",
            "--max-bytes",
            "10",
        ])
        .unwrap();

        assert!(matches!(
            default.command,
            Command::Download { max_bytes: 67_108_864, .. }
        ));
        assert!(matches!(
            explicit.command,
            Command::Download { max_bytes: 10, .. }
        ));
    }

    #[test]
    fn download_max_bytes_rejects_zero() {
        assert!(
            Cli::try_parse_from([
                "chrome-agent",
                "download",
                "https://example.com/a",
                "--max-bytes",
                "0",
            ])
            .is_err()
        );
    }

    /// The arg groups are the validation: a claim with two comparators, or none, is not a
    /// claim, and clap must refuse it before a browser is opened.
    #[test]
    fn assert_requires_exactly_one_comparator_and_one_target() {
        let ok = |args: &[&str]| {
            Cli::try_parse_from(std::iter::once("chrome-agent").chain(args.iter().copied()))
        };
        assert!(ok(&["assert", "value", "--selector", "#a", "--equals", "x"]).is_ok());
        assert!(ok(&["assert", "value", "--uid", "n1", "--contains", "x"]).is_ok());
        // Two comparators, or none.
        assert!(ok(&["assert", "value", "--selector", "#a", "--equals", "x", "--contains", "y"]).is_err());
        assert!(ok(&["assert", "value", "--selector", "#a"]).is_err());
        // Two targets, or none.
        assert!(ok(&["assert", "value", "--selector", "#a", "--uid", "n1", "--equals", "x"]).is_err());
        assert!(ok(&["assert", "value", "--equals", "x"]).is_err());
        // `text` needs no target (the whole page) but still needs a comparator, and
        // `--equals` is not one of its options at all.
        assert!(ok(&["assert", "text", "--contains", "x"]).is_ok());
        assert!(ok(&["assert", "text"]).is_err());
        assert!(ok(&["assert", "text", "--equals", "x"]).is_err());
        // Exactly one state, and a target for it.
        assert!(ok(&["assert", "state", "--selector", "#a", "--checked"]).is_ok());
        assert!(ok(&["assert", "state", "--selector", "#a", "--checked", "--unchecked"]).is_err());
        assert!(ok(&["assert", "state", "--selector", "#a"]).is_err());
        assert!(ok(&["assert", "state", "--checked"]).is_err());
        // exists: selector required, count and min mutually exclusive but both optional.
        assert!(ok(&["assert", "exists", "--selector", ".row"]).is_ok());
        assert!(ok(&["assert", "exists", "--selector", ".row", "--count", "3"]).is_ok());
        assert!(ok(&["assert", "exists", "--selector", ".row", "--count", "3", "--min", "1"]).is_err());
        assert!(ok(&["assert", "exists", "--count", "3"]).is_err());
        // url takes no target at all.
        assert!(ok(&["assert", "url", "--equals", "https://a/"]).is_ok());
        assert!(ok(&["assert", "url", "--selector", "#a", "--equals", "x"]).is_err());
    }

    #[test]
    fn batch_stop_on_error_is_off_by_default() {
        let default = Cli::try_parse_from(["chrome-agent", "batch"]).unwrap();
        let explicit = Cli::try_parse_from(["chrome-agent", "batch", "--stop-on-error"]).unwrap();
        assert!(matches!(default.command, Command::Batch { stop_on_error: false }));
        assert!(matches!(explicit.command, Command::Batch { stop_on_error: true }));
    }

    #[test]
    fn parses_managed_browser_proxy() {
        let cli = Cli::try_parse_from([
            "chrome-agent",
            "--proxy-server",
            "http://127.0.0.1:8080",
            "status",
        ])
        .unwrap();

        assert_eq!(cli.proxy_server.as_deref(), Some("http://127.0.0.1:8080"));
    }
}
