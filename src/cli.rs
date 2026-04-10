use clap::{Parser, Subcommand};

const CLI_LONG_ABOUT: &str = "\
chrome-agent — browser automation for AI agents. Controls Chrome via CDP.\n\
Single binary, zero runtime dependencies. Named pages persist between invocations.\n\
Use --stealth to bypass bot detection (Cloudflare, Turnstile).\n\
Use --copy-cookies to access sites where you're already logged in (X.com, Gmail).\n\
\n\
Workflow: inspect → read uids → act (click/fill) → inspect again.\n\
Use --inspect on action commands to combine action + observation in one call.";

const CLI_AFTER_LONG_HELP: &str = include_str!("../llm-guide.txt");

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
    #[arg(long, default_value = "default")]
    pub browser: String,

    /// Connect to existing browser: ws:// URL, http:// URL, or "auto"
    #[arg(long)]
    pub connect: Option<String>,

    /// Launch browser with a visible window (default is headless)
    #[arg(long)]
    pub headed: bool,

    /// Global timeout in seconds for page loads
    #[arg(long, default_value = "30")]
    pub timeout: u64,

    /// Ignore HTTPS certificate errors
    #[arg(long)]
    pub ignore_https_errors: bool,

    /// Output structured JSON instead of text
    #[arg(long)]
    pub json: bool,

    /// Stealth mode: 7 anti-detection patches (webdriver, UA, WebGL, input leak, Runtime.enable skipped)
    #[arg(long)]
    pub stealth: bool,

    /// Max depth for --inspect output (used by goto, click, fill, etc.)
    #[arg(long)]
    pub max_depth: Option<usize>,

    /// Copy cookies from your real Chrome profile (uses your logged-in sessions)
    #[arg(long)]
    pub copy_cookies: bool,

    /// Named page/tab within the browser (default: "default")
    #[arg(long, default_value = "default")]
    pub page: String,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Navigate to a URL
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
    },

    /// Click an element by uid, CSS selector, or coordinates
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

    /// Fill an input element by uid or CSS selector
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

    /// Fill multiple form fields at once
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
    Forward,

    /// Double-click an element by uid, CSS selector, or coordinates
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

    /// Select a dropdown option by value or visible text
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

    /// Ensure a checkbox/radio is checked (idempotent)
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

    /// Ensure a checkbox/radio is unchecked (idempotent)
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

    /// Upload file(s) to a file input element
    Upload {
        /// File path(s) to upload
        files: Vec<String>,
        /// Element uid of the file input
        #[arg(long)]
        uid: Option<String>,
        /// CSS selector of the file input
        #[arg(long)]
        selector: Option<String>,
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
    },

    /// Show what changed since the last inspect
    Diff,

    /// Capture a screenshot
    #[command(alias = "capture")]
    Screenshot {
        /// Output filename (default: timestamped)
        #[arg(long)]
        filename: Option<String>,
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

    /// Wait for a condition (text, url, or selector)
    Wait {
        /// What to wait for: "text", "url", or "selector"
        what: String,
        /// Pattern to match
        pattern: String,
        /// Timeout in seconds
        #[arg(long, default_value = "10")]
        timeout: u64,
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
    Frame {
        /// CSS selector of the iframe, or "main" to return to top-level
        target: String,
    },

    /// Execute multiple commands from a JSON array on stdin
    Batch,

    /// Persistent connection mode — read JSON commands from stdin (one per line)
    Pipe,

    /// List open browser tabs
    Tabs,

    /// Close the managed browser
    Close {
        /// Also delete the browser profile (cookies, cache, data)
        #[arg(long)]
        purge: bool,
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
pub enum DaemonAction {
    /// Start the daemon (foreground, used internally)
    Start,
}
