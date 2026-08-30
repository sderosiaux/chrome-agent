//! What to do about a navigation Chrome refused to make.
//!
//! These happen before a document exists, so no hint has a uid or an element to name. Each
//! states the stage that failed — DNS, TCP, TLS, HTTP — since each rules out different causes.

/// One hint per `net::` code, each naming the stage that failed. The cert and connection-reset
/// branches carry no command on purpose: neither has a recovery inside this tool.
pub(super) fn navigation_failure(msg: &str, run: &str) -> String {
    let url = failed_url(msg);
    let code = msg.split("net::").nth(1).map_or("", str::trim);
    let host = url.and_then(|url| crate::landing::host_and_path(url).map(|(host, _)| host));
    let origin = url.and_then(crate::landing::origin_of);
    let named = host.unwrap_or("the host");

    match code {
        "ERR_NAME_NOT_RESOLVED" => {
            // An apex with no address record is usually a missing `www.` — the one guess worth
            // making, and the hint states the criterion that chooses it.
            let alternative = match (host, url) {
                (Some(host), Some(url)) if !host.starts_with("www.") => {
                    format!(
                        " If it is missing a subdomain, `{run} goto {}` is the usual form.",
                        url.replacen(host, &format!("www.{host}"), 1)
                    )
                }
                _ => String::new(),
            };
            format!(
                "DNS returned no address for `{named}`, so no connection was opened and \
                 nothing about the site is known — this is the name, not the path, not the \
                 server.{alternative} If the name is right as typed, the domain publishes no \
                 address and no flag in this tool changes that."
            )
        }
        "ERR_CONNECTION_REFUSED" => {
            // `goto` is what prefixed `https://`, so a plain-HTTP server refusing it is this
            // tool's own default failing.
            let plain = match url {
                Some(url) if url.starts_with("https://") => format!(
                    " `goto` prefixes `https://` when the URL carries no scheme — if that \
                     server speaks plain HTTP, `{run} goto {}` is the same address with the \
                     scheme it actually serves.",
                    url.replacen("https://", "http://", 1)
                ),
                _ => String::new(),
            };
            format!(
                "`{named}` resolved and then refused the connection: the name exists and \
                 nothing is listening behind it at that port.{plain}"
            )
        }
        code if code.starts_with("ERR_CERT_") => format!(
            "Chrome rejected the TLS certificate `{named}` presented ({code}), so the \
             connection was dropped before any HTTP request was sent and no page exists to \
             read. That certificate is the site's, not this tool's: no chrome-agent flag \
             makes Chrome accept it, and navigating again gets the same rejection. The \
             hostname the certificate does name is the one to navigate to."
        ),
        "ERR_CONNECTION_RESET" | "ERR_EMPTY_RESPONSE" => format!(
            "The connection to `{named}` opened and was then closed with no response ({code}): \
             the request may have reached the server, the answer did not come back. That is \
             what a proxy or a filter dropping the connection looks like and what a server \
             closing mid-response looks like, and nothing here tells them apart. No document \
             was served, so there is nothing to inspect and nothing this tool can read."
        ),
        "ERR_HTTP_RESPONSE_CODE_FAILURE" => {
            let root = origin.unwrap_or_else(|| "the site root".to_string());
            format!(
                "The server answered and Chrome refused to render what it sent ({code}) — an \
                 error status with a body it will not display. The request reached the server, \
                 so the host and the network are fine and the path or the authorisation is \
                 not. Run `{run} goto {root}` to reach the site root, where a path that does \
                 serve a document can be found."
            )
        }
        "ERR_UNSAFE_PORT" => format!(
            "Chrome refuses to connect to this port at all ({code}): it keeps a fixed list of \
             ports it will not open — 9, 87, 6000 and about eighty others — and the refusal \
             happens before any connection is attempted, whatever is listening there. Move \
             the service to a port outside that list; no flag in this tool overrides it."
        ),
        "ERR_ABORTED" => format!(
            "The navigation was cancelled before it produced a document ({code}). A download \
             the page started, a script navigating elsewhere, and a redirect to a scheme \
             Chrome hands to another application all end this way. Run `{run} inspect` to see \
             which document the page is holding now."
        ),
        code if code.starts_with("ERR_") => {
            let root = origin.unwrap_or_else(|| "the site root".to_string());
            format!(
                "Chrome refused this navigation with {code} before any document existed, so \
                 nothing about the page is known and there is nothing to inspect. The code is \
                 Chromium's own and names the stage that failed. Run `{run} goto {root}` to \
                 see whether the host answers at all."
            )
        }
        // No `net::` code at all: say so rather than guess a cause.
        _ => format!(
            "Chrome refused this navigation and gave no `net::` code this version recognises, \
             so which stage failed — DNS, connection, TLS, HTTP — is not known from here. Run \
             `{run} status` to confirm the browser is the one this session recorded."
        ),
    }
}

/// The URL a failed navigation was aimed at, as `commands::goto` writes it into the message.
fn failed_url(msg: &str) -> Option<&str> {
    let rest = msg.split("Navigation failed for ").nth(1)?;
    let end = rest.find(": ").unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hints::error_hint;

    /// Each of the five common codes states its own stage, and no two share a hint.
    #[test]
    fn each_navigation_failure_states_the_stage_that_failed() {
        let cases = [
            ("net::ERR_NAME_NOT_RESOLVED", "DNS"),
            ("net::ERR_CONNECTION_REFUSED", "refused the connection"),
            ("net::ERR_CERT_COMMON_NAME_INVALID", "TLS certificate"),
            ("net::ERR_CONNECTION_RESET", "no response"),
            ("net::ERR_HTTP_RESPONSE_CODE_FAILURE", "refused to render"),
        ];
        let mut seen: Vec<String> = Vec::new();
        for (code, fact) in cases {
            let hint = error_hint(
                &format!("Navigation failed for https://site.test/a: {code}"),
                "default",
            )
            .expect("a hint");
            assert!(hint.contains(fact), "{code} does not state its stage: {hint}");
            assert!(
                !hint.contains("Check the URL is valid"),
                "the one-size sentence survived: {hint}"
            );
            assert!(!seen.contains(&hint), "two causes share a hint: {hint}");
            seen.push(hint);
        }
    }

    /// Rule 1: the fact is about the host that failed, and the tool knows which one it was.
    #[test]
    fn a_navigation_failure_names_the_host_it_could_not_reach() {
        let hint = error_hint(
            "Navigation failed for https://akamai.net: net::ERR_NAME_NOT_RESOLVED",
            "default",
        )
        .expect("a hint");
        assert!(hint.contains("akamai.net"), "{hint}");
        // Rule 2: the guess, with the criterion that chooses it.
        assert!(hint.contains("chrome-agent goto https://www.akamai.net"), "{hint}");
        assert!(hint.contains("If it is missing a subdomain"), "the criterion: {hint}");
    }

    /// `goto` prefixes `https://` itself, so the `http://` recovery is the tool's to name.
    #[test]
    fn a_refused_connection_offers_the_scheme_goto_did_not_choose() {
        let hint = error_hint(
            "Navigation failed for https://localhost:3000/a: net::ERR_CONNECTION_REFUSED",
            "agent-7",
        )
        .expect("a hint");
        assert!(
            hint.contains("`chrome-agent --browser agent-7 goto http://localhost:3000/a`"),
            "{hint}"
        );
        // Not offered when the caller chose http themselves.
        let plain = error_hint(
            "Navigation failed for http://localhost:3000/a: net::ERR_CONNECTION_REFUSED",
            "default",
        )
        .expect("a hint");
        assert!(!plain.contains("goto http://"), "nothing to change: {plain}");
    }

    /// The two failures with no recovery say so; repeating the navigation gets the same answer.
    #[test]
    fn the_two_failures_with_no_recovery_say_so_instead_of_guessing() {
        let cert = error_hint(
            "Navigation failed for https://x.test/a: net::ERR_CERT_COMMON_NAME_INVALID",
            "default",
        )
        .expect("a hint");
        assert!(cert.contains("no chrome-agent flag makes Chrome accept it"), "{cert}");

        let reset = error_hint(
            "Navigation failed for https://x.test/a: net::ERR_CONNECTION_RESET",
            "default",
        )
        .expect("a hint");
        assert!(reset.contains("nothing to inspect"), "{reset}");
        assert!(reset.contains("may have reached the server"), "{reset}");
    }

    /// An unknown code still gets a fact and a command; a codeless message says it has none.
    #[test]
    fn an_unrecognised_code_is_reported_as_itself() {
        let unknown = error_hint(
            "Navigation failed for https://x.test:8443/a: net::ERR_SOCKS_CONNECTION_FAILED",
            "default",
        )
        .expect("a hint");
        assert!(unknown.contains("ERR_SOCKS_CONNECTION_FAILED"), "{unknown}");
        assert!(unknown.contains("chrome-agent goto https://x.test:8443/"), "the port survives: {unknown}");

        let codeless = error_hint("Navigation failed", "default").expect("a hint");
        assert!(codeless.contains("no `net::` code"), "{codeless}");
    }

    #[test]
    fn the_failed_url_is_read_out_of_the_message() {
        assert_eq!(
            failed_url("Navigation failed for https://x.test/a: net::ERR_ABORTED"),
            Some("https://x.test/a")
        );
        assert_eq!(failed_url("Navigation failed"), None);
    }
}
