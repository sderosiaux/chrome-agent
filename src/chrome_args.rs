//! Validation for `--chrome-arg`, which passes extra flags to the Chrome chrome-agent launches
//! (e.g. `--enable-features=WebMCP,WebMCPTesting`). Re-exported from `browser.rs`.

use crate::browser::BrowserError;
use crate::session::{BrowserSession, SessionError, SessionStore};

/// Why a flag is refused. Two different questions wear one deny-list, and a caller who reads
/// "chrome-agent could not reconnect" over a flag that publishes the browser to the network has
/// been told the smaller half. The kind is in the data, so the refusal cannot describe the
/// wrong one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Refusal {
    /// chrome-agent could not find or reconnect to the browser it just launched. Costs this
    /// invocation; costs nothing outside it.
    BreaksReconnect,
    /// The flag widens what the launched browser exposes — to the network, to other origins,
    /// or to code that is not this tool's. `--chrome-arg` is `global = true` and its values are
    /// often composed by an agent reading a page, so these are not the caller's own judgement
    /// in the way a hand-typed launch flag is.
    WidensExposure,
}

impl Refusal {
    /// The sentence that says which of the two questions the caller hit, before the flag's own
    /// reason. Stated first, because it is what decides whether the refusal is negotiable.
    const fn preamble(self) -> &'static str {
        match self {
            Self::BreaksReconnect => {
                "chrome-agent could not then find or reconnect to the browser it launches."
            }
            Self::WidensExposure => {
                "It widens what the launched browser exposes, and no chrome-agent flag narrows \
                 it again."
            }
        }
    }
}

/// Chrome flags refused via `--chrome-arg`, each paired with the kind of refusal and the reason.
/// There is no safe rewrite to suggest, so the reason is the whole message.
///
/// This is a deny-list and therefore not a boundary: it names the flags measured to be
/// dangerous, not every flag that is. `--chrome-arg` remains a way to change how Chrome behaves,
/// and an invocation that composes it from page content is trusting that page with the browser.
pub const FORBIDDEN_CHROME_ARGS: &[(&str, Refusal, &str)] = &[
    (
        "user-data-dir",
        Refusal::BreaksReconnect,
        "chrome-agent manages this itself: it writes DevToolsActivePort inside the profile \
         directory it tracks per --browser name and reads it back to reconnect on the next \
         invocation. A different --user-data-dir points Chrome at a directory chrome-agent \
         never looks in, and reconnection breaks. Each --browser name already gets its own \
         isolated profile — pass a different name instead of overriding this flag.",
    ),
    (
        "remote-debugging-port",
        Refusal::BreaksReconnect,
        "chrome-agent launches Chrome with --remote-debugging-port=0 on purpose, so the OS \
         assigns a free port and chrome-agent reads it back from DevToolsActivePort. A fixed \
         port can collide with another profile's Chrome and chrome-agent would still be \
         reading the port it asked for, not the one Chrome bound.",
    ),
    (
        "remote-debugging-pipe",
        Refusal::BreaksReconnect,
        "this replaces the CDP transport chrome-agent expects (a WebSocket read from \
         DevToolsActivePort) with a stdio pipe it never opens, so chrome-agent could not \
         attach to the browser it just launched.",
    ),
    (
        "proxy-server",
        Refusal::BreaksReconnect,
        "chrome-agent has a dedicated --proxy-server flag that validates the value and \
         persists it, so a later command with a different proxy is refused with a clear \
         error instead of silently launching a second, differently-routed browser. Use \
         --proxy-server instead of --chrome-arg for this.",
    ),
    (
        "headless",
        Refusal::BreaksReconnect,
        "chrome-agent decides headless or headed from --headed and stores the chosen mode \
         per --browser name to detect a mismatch on the next invocation. A --chrome-arg \
         setting this directly can disagree with that stored mode without chrome-agent ever \
         seeing the disagreement. Use --headed instead of --chrome-arg for this.",
    ),
    (
        "remote-debugging-address",
        Refusal::WidensExposure,
        "chrome-agent already launches Chrome with --remote-debugging-port=0, so this flag \
         decides only which interfaces that debugging endpoint is reachable on, and 0.0.0.0 \
         publishes it on all of them. CDP is not an API with permissions: whoever reaches that \
         port reads any file through file:// URLs, reads every cookie in the profile and \
         evaluates JavaScript in any page. The endpoint stays on loopback.",
    ),
    (
        "disable-web-security",
        Refusal::WidensExposure,
        "this turns off the same-origin policy for every page in the browser, so any page \
         loaded afterwards — including one this tool navigated to on a link it read — can \
         issue credentialed requests to any other origin and read the responses. A page \
         reached during a session where this is set has the profile's whole logged-in surface.",
    ),
    (
        "load-extension",
        Refusal::WidensExposure,
        "an extension runs its own code in this browser with the browser's privileges, on \
         every page it declares a match for, for as long as the profile lives. Nothing chosen \
         through --chrome-arg is reviewed, and nothing chrome-agent does afterwards is outside \
         that extension's reach.",
    ),
    (
        "host-resolver-rules",
        Refusal::WidensExposure,
        "this remaps hostnames to addresses inside Chrome, silently and with no trace in any \
         response. Every URL this tool reports — landed.final, a screenshot, an extracted \
         price — would then name a host that is not what answered, which is the one guarantee \
         a browser-automation tool has to keep.",
    ),
    (
        "remote-allow-origins",
        Refusal::WidensExposure,
        "this lets a web page's own JavaScript open the CDP WebSocket, which is the connection \
         that drives this browser. With `*` any page loaded in any browser on this machine can \
         take over the Chrome chrome-agent launched.",
    ),
    (
        "auth-server-allowlist",
        Refusal::WidensExposure,
        "this tells Chrome which servers it may hand the OS user's Kerberos/NTLM credentials \
         to automatically, with no prompt. A server named here is authenticated to as that \
         user by any page that provokes a request to it.",
    ),
];

/// The flag name a `--chrome-arg` value sets, e.g. `--user-data-dir=/tmp/x` → `user-data-dir`.
/// `None` without a `--` prefix: not a switch shape this tool judges, so Chrome decides.
fn chrome_arg_flag_name(arg: &str) -> Option<&str> {
    let stripped = arg.strip_prefix("--")?;
    Some(stripped.split('=').next().unwrap_or(stripped))
}

/// Refuse a `--chrome-arg` that either breaks a chrome-agent launch assumption or widens what
/// the browser exposes. Explained per flag, and the message names which of the two it hit.
fn validate_chrome_args(args: &[String]) -> Result<(), BrowserError> {
    for arg in args {
        let Some(name) = chrome_arg_flag_name(arg) else {
            continue;
        };
        if let Some((flag, kind, why)) = FORBIDDEN_CHROME_ARGS
            .iter()
            .find(|(flag, _, _)| *flag == name)
        {
            return Err(BrowserError::Launch(format!(
                "--chrome-arg={arg} is refused: --{flag} is not available through --chrome-arg. \
                 {} {why}",
                kind.preamble()
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
pub fn effective_chrome_args(
    store: &SessionStore,
    name: &str,
    requested: &[String],
) -> Vec<String> {
    if !requested.is_empty() {
        return requested.to_vec();
    }
    store
        .browsers
        .get(name)
        .map(|b| b.chrome_args.clone())
        .unwrap_or_default()
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
        let err = ensure_chrome_args_compatible(&existing, &requested)
            .unwrap_err()
            .to_string();
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
            (
                "remote-debugging-address",
                "--remote-debugging-address=0.0.0.0",
            ),
            ("disable-web-security", "--disable-web-security"),
            ("load-extension", "--load-extension=/tmp/ext"),
            (
                "host-resolver-rules",
                "--host-resolver-rules=MAP * 127.0.0.1",
            ),
            ("remote-allow-origins", "--remote-allow-origins=*"),
            (
                "auth-server-allowlist",
                "--auth-server-allowlist=*.corp.test",
            ),
        ] {
            let error = normalized_chrome_args_option(None, &[value.to_string()])
                .unwrap_err()
                .to_string();
            assert!(error.contains(&format!("--{flag}")), "{flag}: {error}");
        }
    }

    /// The list answers two different questions, and a caller told "chrome-agent could not
    /// reconnect" about a flag that publishes CDP on every interface has been told the smaller
    /// half. The kind is in the data, so the message cannot describe the wrong one.
    #[test]
    fn a_refusal_says_which_kind_of_refusal_it_is() {
        let exposure = normalized_chrome_args_option(
            None,
            &["--remote-debugging-address=0.0.0.0".to_string()],
        )
        .unwrap_err()
        .to_string();
        assert!(
            exposure.contains("widens what the launched browser exposes"),
            "{exposure}"
        );
        assert!(
            !exposure.contains("could not then find or reconnect"),
            "an exposure refusal must not be worded as a reconnection problem: {exposure}"
        );

        let reconnect =
            normalized_chrome_args_option(None, &["--user-data-dir=/tmp/x".to_string()])
                .unwrap_err()
                .to_string();
        assert!(
            reconnect.contains("could not then find or reconnect"),
            "{reconnect}"
        );
        assert!(!reconnect.contains("widens what"), "{reconnect}");

        // Both kinds are actually present, or the distinction is decoration.
        for kind in [Refusal::BreaksReconnect, Refusal::WidensExposure] {
            assert!(
                FORBIDDEN_CHROME_ARGS.iter().any(|(_, k, _)| *k == kind),
                "no entry of kind {kind:?}"
            );
        }
    }

    /// Every flag the security review named, refused by the flag name Chrome parses — the
    /// value form does not matter, since `chrome_arg_flag_name` splits on `=`.
    #[test]
    fn the_flags_that_widen_exposure_are_refused_whatever_value_they_carry() {
        for flag in [
            "remote-debugging-address",
            "disable-web-security",
            "load-extension",
            "host-resolver-rules",
            "remote-allow-origins",
            "auth-server-allowlist",
        ] {
            let listed = FORBIDDEN_CHROME_ARGS
                .iter()
                .find(|(name, _, _)| *name == flag)
                .unwrap_or_else(|| panic!("{flag} is not on the list"));
            assert_eq!(
                listed.1,
                Refusal::WidensExposure,
                "{flag} is listed as the wrong kind"
            );
            for spelling in [
                format!("--{flag}"),
                format!("--{flag}=x"),
                format!("--{flag}=*"),
            ] {
                assert!(
                    normalized_chrome_args_option(None, std::slice::from_ref(&spelling)).is_err(),
                    "{spelling} was accepted"
                );
            }
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
        for (flag, kind, why) in FORBIDDEN_CHROME_ARGS {
            let value = format!("--{flag}=x");
            let error = normalized_chrome_args_option(None, std::slice::from_ref(&value))
                .unwrap_err()
                .to_string();
            // Rule 1: states a fact chrome-agent depends on, and which kind of fact it is.
            assert!(error.contains(why), "{flag}: reason missing: {error}");
            assert!(
                error.contains(kind.preamble()),
                "{flag}: kind missing: {error}"
            );
            // Rule 2: the real value is quoted back, never a placeholder like <value>.
            assert!(
                error.contains(&value),
                "{flag}: real value not echoed: {error}"
            );
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
