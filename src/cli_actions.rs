//! The subcommand enums: what each verb of the CLI can be asked to do.
//!
//! Split out of `cli.rs` for the repo's 1000-line file cap and re-exported from it, so every
//! call site stays `crate::cli::EmulateAction`. The seam is the natural one: `cli.rs` declares
//! the flags and the verbs, and these declare what the verbs that have modes accept.

use clap::Subcommand;

#[derive(Subcommand)]
pub enum MacroAction {
    /// List the macros this machine knows
    List,
    /// Print one macro, with its steps and their guards
    Show {
        /// Macro name
        name: String,
    },
    /// Distil a recorded session into a macro
    Record {
        /// Name to save it under
        name: String,
        /// The recording to distil (a `_record` file written by a pipe session)
        #[arg(long)]
        from_recording: String,
        /// Index of the first entry of the task (default: the last successful navigation)
        #[arg(long)]
        from: Option<usize>,
    },
    /// Run a macro, guarding every step
    Run {
        /// Macro name
        name: String,
        /// Parameter values (repeatable, or comma-separated): --var email=a@b.c
        #[arg(long, value_delimiter = ',')]
        var: Vec<String>,
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
pub enum WebmcpAction {
    /// List tools this page has registered — name, description, inputSchema. No outputSchema:
    /// the protocol defines none.
    List,
    /// Call a tool by name (resolved to the `RegisteredTool` the page's own `getTools()` reported —
    /// executeTool refuses a bare name). Reports what the tool declared AND what the page's
    /// accessibility tree measurably did, side by side.
    Call {
        /// Tool name, exactly as `webmcp list` reported it
        name: String,
        /// JSON object of arguments (default: "{}")
        #[arg(long, default_value = "{}")]
        args: String,
        /// Inspect page after calling
        #[arg(long)]
        inspect: bool,
        /// Max depth for inspect output (also accepted as global flag)
        #[arg(long)]
        max_depth: Option<usize>,
    },
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
