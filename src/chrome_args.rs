//! Validation for `--chrome-arg`, which passes extra flags to the Chrome chrome-agent launches
//! (e.g. `--enable-features=WebMCP,WebMCPTesting`). Re-exported from `browser.rs`.

use crate::browser::BrowserError;
use crate::session::{BrowserSession, SessionError, SessionStore};

/// Chrome flags refused via `--chrome-arg`, each paired with the chrome-agent assumption it
/// would break. There is no safe rewrite to suggest, so the reason is the whole message.
pub const FORBIDDEN_CHROME_ARGS: &[(&str, &str)] = &[
    (
        "user-data-dir",
        "chrome-agent manages this itself: it writes DevToolsActivePort inside the profile \
         directory it tracks per --browser name and reads it back to reconnect on the next \
         invocation. A different --user-data-dir points Chrome at a directory chrome-agent \
         never looks in, and reconnection breaks. Each --browser name already gets its own \
         isolated profile — pass a different name instead of overriding this flag.",
    ),
    (
        "remote-debugging-port",
        "chrome-agent launches Chrome with --remote-debugging-port=0 on purpose, so the OS \
         assigns a free port and chrome-agent reads it back from DevToolsActivePort. A fixed \
         port can collide with another profile's Chrome and chrome-agent would still be \
         reading the port it asked for, not the one Chrome bound.",
    ),
    (
        "remote-debugging-pipe",
        "this replaces the CDP transport chrome-agent expects (a WebSocket read from \
         DevToolsActivePort) with a stdio pipe it never opens, so chrome-agent could not \
         attach to the browser it just launched.",
    ),
    (
        "proxy-server",
        "chrome-agent has a dedicated --proxy-server flag that validates the value and \
         persists it, so a later command with a different proxy is refused with a clear \
         error instead of silently launching a second, differently-routed browser. Use \
         --proxy-server instead of --chrome-arg for this.",
    ),
    (
        "headless",
        "chrome-agent decides headless or headed from --headed and stores the chosen mode \
         per --browser name to detect a mismatch on the next invocation. A --chrome-arg \
         setting this directly can disagree with that stored mode without chrome-agent ever \
         seeing the disagreement. Use --headed instead of --chrome-arg for this.",
    ),
];

/// The flag name a `--chrome-arg` value sets, e.g. `--user-data-dir=/tmp/x` → `user-data-dir`.
/// `None` without a `--` prefix: not a switch shape this tool judges, so Chrome decides.
fn chrome_arg_flag_name(arg: &str) -> Option<&str> {
    let stripped = arg.strip_prefix("--")?;
    Some(stripped.split('=').next().unwrap_or(stripped))
}

/// Refuse a `--chrome-arg` overriding a flag chrome-agent depends on for launch or
/// reconnection. Explained per flag, not as one generic refusal.
fn validate_chrome_args(args: &[String]) -> Result<(), BrowserError> {
    for arg in args {
        let Some(name) = chrome_arg_flag_name(arg) else {
            continue;
        };
        if let Some((flag, why)) = FORBIDDEN_CHROME_ARGS.iter().find(|(flag, _)| *flag == name) {
            return Err(BrowserError::Launch(format!(
                "--chrome-arg={arg} is refused: --{flag} is not available through --chrome-arg. {why}"
            )));
        }
    }
    Ok(())
}

/// Resolve the launch-only `--chrome-arg` contract shared by CLI, pipe and replay. Mirrors
/// `normalized_proxy_option`: with `--connect` it is refused, not silently ignored.
pub fn normalized_chrome_args_option(
    connect: Option<&str>,
    chrome_args: &[String],
) -> Result<Vec<String>, BrowserError> {
    if connect.is_some() && !chrome_args.is_empty() {
        return Err(BrowserError::Launch(
            "--chrome-arg applies only when chrome-agent launches Chrome; it has no effect on \
             an attached browser. Drop --connect to let chrome-agent launch its own Chrome with \
             these flags, or drop --chrome-arg and pass the flag to the Chrome you are attaching \
             to directly."
                .into(),
        ));
    }
    validate_chrome_args(chrome_args)?;
    Ok(chrome_args.to_vec())
}

/// Merge a requested `--chrome-arg` list with the named browser's stored one: an omitted list
/// inherits, the same rule `proxy_server` gets from `Option::or`.
pub fn effective_chrome_args(store: &SessionStore, name: &str, requested: &[String]) -> Vec<String> {
    if !requested.is_empty() {
        return requested.to_vec();
    }
    store.browsers.get(name).map(|b| b.chrome_args.clone()).unwrap_or_default()
}

/// Guard `--chrome-arg` compatibility when reconnecting to a live named browser. Chrome reads
/// its command line once, so an omitted list inherits and only an explicit, different one is
/// refused rather than killing a browser the caller never asked to close. Same rule as
/// `session::ensure_proxy_compatible`.
pub fn ensure_chrome_args_compatible(
    browser: &BrowserSession,
    effective: &[String],
) -> Result<(), SessionError> {
    if effective.is_empty() || browser.chrome_args == effective {
        return Ok(());
    }
    Err(SessionError(
        "named browser is already running with different --chrome-arg flags; close or purge it (chrome-agent --browser <name> close --purge), or select another browser name"
            .into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_browser() -> BrowserSession {
        BrowserSession {
            ws_endpoint: "ws://localhost:9222".into(),
            pid: Some(1),
            headless: true,
            proxy_server: None,
            chrome_args: Vec::new(),
            daemon_pid: None,
            pages: std::collections::HashMap::new(),
        }
    }

    #[test]
    fn named_browser_chrome_args_must_match_before_reuse() {
        let existing = a_browser();
        assert!(ensure_chrome_args_compatible(&existing, &[]).is_ok());
        // Requested against a browser launched with none: refused.
        let requested = vec!["--enable-features=WebMCP,WebMCPTesting".to_string()];
        let err = ensure_chrome_args_compatible(&existing, &requested).unwrap_err().to_string();
        assert!(err.contains("different --chrome-arg flags"), "{err}");
    }

    #[test]
    fn chrome_arg_browser_inherits_flags_when_omitted() {
        let mut existing = a_browser();
        existing.chrome_args = vec!["--enable-features=WebMCP,WebMCPTesting".to_string()];
        // Omitted inherits; the same flags re-requested are fine.
        assert!(ensure_chrome_args_compatible(&existing, &[]).is_ok());
        assert!(ensure_chrome_args_compatible(&existing, &existing.chrome_args.clone()).is_ok());
        // Different flags are refused: Chrome reads its command line only at process start.
        let different = vec!["--enable-features=Other".to_string()];
        assert!(ensure_chrome_args_compatible(&existing, &different).is_err());
    }

    #[test]
    fn chrome_args_under_connect_is_refused_rather_than_ignored() {
        let error = normalized_chrome_args_option(
            Some("http://127.0.0.1:9222"),
            &["--enable-features=WebMCP".to_string()],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("applies only when chrome-agent launches Chrome"));
        // Omitting it under --connect is not an error: there is nothing to apply.
        assert!(normalized_chrome_args_option(Some("http://127.0.0.1:9222"), &[]).is_ok());
    }

    #[test]
    fn each_forbidden_chrome_arg_is_refused() {
        for (flag, value) in [
            ("user-data-dir", "--user-data-dir=/tmp/evil"),
            ("remote-debugging-port", "--remote-debugging-port=9222"),
            ("remote-debugging-pipe", "--remote-debugging-pipe"),
            ("proxy-server", "--proxy-server=http://127.0.0.1:8080"),
            ("headless", "--headless=new"),
        ] {
            let error = normalized_chrome_args_option(None, &[value.to_string()])
                .unwrap_err()
                .to_string();
            assert!(error.contains(&format!("--{flag}")), "{flag}: {error}");
        }
    }

    #[test]
    fn a_non_forbidden_chrome_arg_is_accepted() {
        assert!(
            normalized_chrome_args_option(
                None,
                &["--enable-features=WebMCP,WebMCPTesting".to_string()]
            )
            .is_ok()
        );
    }

    /// Same contract as `hints.rs`: one fact, no unresolved placeholder, and no wording that
    /// invites a blind retry (this error means "don't send this at all").
    #[test]
    fn forbidden_chrome_arg_errors_follow_the_hints_contract() {
        for (flag, why) in FORBIDDEN_CHROME_ARGS {
            let value = format!("--{flag}=x");
            let error = normalized_chrome_args_option(None, std::slice::from_ref(&value))
                .unwrap_err()
                .to_string();
            // Rule 1: states a fact chrome-agent depends on.
            assert!(error.contains(why), "{flag}: reason missing: {error}");
            // Rule 2: the real value is quoted back, never a placeholder like <value>.
            assert!(error.contains(&value), "{flag}: real value not echoed: {error}");
            for placeholder in ["<value>", "<name>", "<uid>", "<n>"] {
                assert!(!error.contains(placeholder), "{flag}: {error}");
            }
            // Rule 3: nothing here invites a blind retry.
            for forbidden in ["Try running the command again", "run the command again"] {
                assert!(!error.contains(forbidden), "{flag}: {error}");
            }
        }
    }
}
