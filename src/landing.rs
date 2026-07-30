//! Where a navigation actually ended up, as opposed to where it was aimed.
//!
//! `goto` used to answer `{url, title}`, which is the destination and nothing about the
//! journey: an expired session redirecting to a login wall reads exactly like a successful
//! load, and every step after it runs against the wrong page. `Landing` is the witness —
//! what was asked for, what answered, whether those differ, and the HTTP status when the
//! page is willing to say.
//!
//! Pure: no CDP, no I/O. `commands::goto` fills it in from one in-page read.

use serde::Serialize;

/// What a navigation reports about itself.
#[derive(Debug, Clone, Serialize)]
pub struct Landing {
    /// The URL as the tool sent it, after the `https://` prefixing `goto` applies. Not the
    /// caller's raw argument: comparing `example.com` against `https://example.com/` would
    /// call every navigation a redirect.
    pub requested: String,
    /// `location.href` once the page settled.
    #[serde(rename = "final")]
    pub final_url: String,
    pub redirected: bool,
    /// Status of the response that produced the final document, from the Navigation Timing
    /// entry. Absent — never zero, never guessed — when the page exposes none: an older
    /// Chrome, a document with no navigation entry, or a `responseStatus` of 0.
    ///
    /// This is what the browser answered, not proof that an HTTP response happened: measured
    /// on Chrome 151, a `file://` document reports 200. A redirect chain reports its last
    /// hop, so a followed `302` reads as 200.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
}

/// Path segments that suggest an authentication wall.
///
/// A guess about a URL, not a reading of the page. Kept small on purpose: every entry here
/// is a word that only appears in a path when the site is asking who you are.
const AUTH_WALL_SEGMENTS: &[&str] =
    &["login", "log-in", "signin", "sign-in", "sign_in", "auth", "sso"];

impl Landing {
    /// Build a landing from the requested and settled URLs.
    pub fn new(requested: &str, final_url: &str, http_status: Option<u16>) -> Self {
        Self {
            requested: requested.to_string(),
            final_url: final_url.to_string(),
            redirected: is_redirect(requested, final_url),
            http_status: status_or_none(http_status),
        }
    }

    /// The auth-wall guess, worded as a guess.
    ///
    /// Only ever fires on a redirect: a caller who typed `/login` themselves knows where
    /// they are, and telling them would be noise on every deliberate visit to a login page.
    pub fn hint(&self) -> Option<String> {
        if !self.redirected {
            return None;
        }
        let segment = auth_wall_segment(&self.final_url)?;
        Some(format!(
            "The redirect landed on a path containing '{segment}', which often means the \
             session expired. This is a guess from the URL, not a reading of the page: run \
             `inspect` to see what is there, and re-authenticate if it is a login form."
        ))
    }

    /// Attach `landed` (and the auth-wall hint) to a response object.
    ///
    /// Lives here rather than in the three call sites so CLI, pipe and batch cannot drift.
    pub fn attach(&self, out: &mut serde_json::Value) {
        if let Some(map) = out.as_object_mut() {
            map.insert(
                "landed".into(),
                serde_json::to_value(self).unwrap_or(serde_json::Value::Null),
            );
            if let Some(hint) = self.hint() {
                map.entry("hint").or_insert_with(|| serde_json::json!(hint));
            }
        }
    }

    /// What a person reads in text mode, or nothing when the navigation went where it was
    /// told. A caller who typed the URL does not need it read back.
    pub fn text_line(&self) -> Option<String> {
        if !self.redirected {
            return None;
        }
        let status = self
            .http_status
            .map_or_else(String::new, |code| format!(" (HTTP {code})"));
        let mut line = format!("redirected from {}{}", self.requested, status);
        if let Some(hint) = self.hint() {
            line.push_str("\nhint: ");
            line.push_str(&hint);
        }
        Some(line)
    }
}

/// A status only counts when it could have come off the wire.
///
/// The Navigation Timing entry reports `0` for a document that had no HTTP response and for
/// a cross-origin one it will not describe. `0` is not a status, and reporting it as one
/// invents an answer where the page declined to give one.
fn status_or_none(raw: Option<u16>) -> Option<u16> {
    raw.filter(|code| (100..=599).contains(code))
}

/// Whether the page ended up somewhere other than where it was sent.
///
/// The rule, and what it deliberately ignores:
/// - **Fragment**: dropped entirely. `#section` is resolved by the browser, never sent, and
///   a page that jumps to an anchor did not redirect anywhere.
/// - **Trailing slash**: one is stripped from the path. `/orders` and `/orders/` are the
///   same resource to every server that normalises between them, and calling that a
///   redirect would fire on most well-configured sites.
/// - **Default port**: `:80` on http and `:443` on https are elided; the host is lowercased
///   (case-insensitive by RFC 3986) while the path is not (it is case-sensitive on most
///   servers).
/// - **Empty query**: `?` with nothing after it equals no query.
///
/// What counts as a redirect, on purpose: any change of scheme (an `http` → `https` upgrade
/// is the server overriding the caller, and it changes which cookies travel), of host, of
/// path beyond that one slash, and any change of query — including a gained `?next=…`,
/// which is the usual shape of the login bounce this field exists to expose. Query
/// parameters are compared in the order they appear: reordering them would be a claim about
/// the server's semantics that this code is in no position to make.
pub fn is_redirect(requested: &str, final_url: &str) -> bool {
    comparison_key(requested) != comparison_key(final_url)
}

/// The part of a URL that has to change for a navigation to have been redirected.
///
/// Hand-rolled rather than a URL crate: the Linux release targets musl with a pure-Rust
/// dependency graph, and this needs to split a string, not to validate one.
fn comparison_key(url: &str) -> String {
    let without_fragment = url.split_once('#').map_or(url, |(head, _)| head);
    let (scheme, rest) = without_fragment
        .split_once("://")
        .map_or(("", without_fragment), |(scheme, rest)| (scheme, rest));
    let scheme = scheme.to_ascii_lowercase();

    let authority_end = rest.find(['/', '?']).unwrap_or(rest.len());
    let (authority, path_and_query) = rest.split_at(authority_end);
    let mut authority = authority.to_ascii_lowercase();
    let default_port = match scheme.as_str() {
        "http" => ":80",
        "https" => ":443",
        _ => "",
    };
    if !default_port.is_empty()
        && let Some(host) = authority.strip_suffix(default_port)
    {
        authority = host.to_string();
    }

    let (path, query) = path_and_query
        .split_once('?')
        .map_or((path_and_query, ""), |(path, query)| (path, query));
    let path = path.strip_suffix('/').unwrap_or(path);
    let query = if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    };
    format!("{scheme}://{authority}{path}{query}")
}

/// The path segment that made the URL look like an auth wall, if any.
///
/// Segment-level, and on the stem before the first `.`, so `/login`, `/login.php` and
/// `/users/sign_in` match while `/authors/tolkien` and `/ssology` do not. Matching a bare
/// substring made `/authors` a login page.
pub fn auth_wall_segment(url: &str) -> Option<&'static str> {
    let without_fragment = url.split_once('#').map_or(url, |(head, _)| head);
    let without_query = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(head, _)| head);
    let path = without_query
        .split_once("://")
        .map_or(without_query, |(_, rest)| {
            rest.find('/').map_or("", |index| &rest[index..])
        });
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .find_map(|segment| {
            let stem = segment.split('.').next().unwrap_or(segment).to_ascii_lowercase();
            AUTH_WALL_SEGMENTS.iter().copied().find(|token| *token == stem)
        })
}

#[cfg(test)]
mod tests {
    use super::{auth_wall_segment, is_redirect, Landing};

    #[test]
    fn same_url_is_not_a_redirect() {
        assert!(!is_redirect(
            "https://example.com/orders",
            "https://example.com/orders"
        ));
    }

    #[test]
    fn fragment_only_change_is_not_a_redirect() {
        // The fragment never leaves the browser, so nothing redirected anything.
        assert!(!is_redirect(
            "https://example.com/docs",
            "https://example.com/docs#install"
        ));
        assert!(!is_redirect(
            "https://example.com/docs#install",
            "https://example.com/docs"
        ));
    }

    #[test]
    fn trailing_slash_is_not_a_redirect() {
        assert!(!is_redirect(
            "https://example.com/orders",
            "https://example.com/orders/"
        ));
        assert!(!is_redirect("https://example.com", "https://example.com/"));
    }

    #[test]
    fn default_port_and_host_case_are_not_a_redirect() {
        assert!(!is_redirect(
            "https://Example.COM:443/a",
            "https://example.com/a"
        ));
        assert!(!is_redirect("http://example.com:80/a", "http://example.com/a"));
    }

    #[test]
    fn path_case_is_a_redirect() {
        // Paths are case-sensitive on most servers, so this is a different resource.
        assert!(is_redirect("https://example.com/A", "https://example.com/a"));
    }

    #[test]
    fn gained_query_is_a_redirect() {
        // The shape of the login bounce this field exists to expose.
        assert!(is_redirect(
            "https://app.example.com/orders",
            "https://app.example.com/login?next=/orders"
        ));
    }

    #[test]
    fn empty_query_is_not_a_redirect() {
        assert!(!is_redirect("https://example.com/a?", "https://example.com/a"));
    }

    #[test]
    fn scheme_upgrade_is_a_redirect() {
        // The server overrode the caller, and which cookies travel changed with it.
        assert!(is_redirect("http://example.com/a", "https://example.com/a"));
    }

    #[test]
    fn host_change_is_a_redirect() {
        assert!(is_redirect("https://example.com/a", "https://www.example.com/a"));
    }

    #[test]
    fn auth_wall_matches_whole_segments_and_stems() {
        assert_eq!(auth_wall_segment("https://x.com/login"), Some("login"));
        assert_eq!(auth_wall_segment("https://x.com/login.php"), Some("login"));
        assert_eq!(auth_wall_segment("https://x.com/users/sign_in"), Some("sign_in"));
        assert_eq!(auth_wall_segment("https://x.com/auth/realms/x"), Some("auth"));
        assert_eq!(auth_wall_segment("https://x.com/sso/saml"), Some("sso"));
        assert_eq!(auth_wall_segment("https://x.com/LOGIN"), Some("login"));
    }

    #[test]
    fn auth_wall_does_not_fire_on_a_longer_word() {
        // Substring matching turned every author page into a login wall.
        assert_eq!(auth_wall_segment("https://x.com/authors/tolkien"), None);
        assert_eq!(auth_wall_segment("https://x.com/ssology"), None);
        assert_eq!(auth_wall_segment("https://x.com/loginformation"), None);
        assert_eq!(auth_wall_segment("https://x.com/orders"), None);
    }

    #[test]
    fn auth_wall_ignores_the_query_and_fragment() {
        // `?next=/login` is where the caller came from, not where they landed.
        assert_eq!(auth_wall_segment("https://x.com/orders?next=/login"), None);
        assert_eq!(auth_wall_segment("https://x.com/orders#login"), None);
    }

    #[test]
    fn hint_fires_only_on_a_redirect_to_an_auth_wall() {
        let bounced = Landing::new(
            "https://app.example.com/orders",
            "https://app.example.com/login?next=/orders",
            Some(200),
        );
        assert!(bounced.redirected);
        assert!(bounced.hint().is_some());

        // An ordinary redirect says nothing about authentication.
        let ordinary = Landing::new(
            "https://example.com/start",
            "https://example.com/settled",
            Some(200),
        );
        assert!(ordinary.redirected);
        assert!(ordinary.hint().is_none());

        // A caller who asked for the login page already knows.
        let deliberate = Landing::new("https://x.com/login", "https://x.com/login", None);
        assert!(!deliberate.redirected);
        assert!(deliberate.hint().is_none());
    }

    #[test]
    fn status_zero_is_absent_rather_than_reported() {
        // A document with no HTTP response of its own reports 0. That is "I don't know", and
        // serialising it as a status invents an answer.
        let landing = Landing::new("file:///tmp/a.html", "file:///tmp/a.html", Some(0));
        assert!(landing.http_status.is_none());
        let json = serde_json::to_value(&landing).unwrap();
        assert!(json.get("http_status").is_none());

        let unavailable = Landing::new("https://x.com/a", "https://x.com/a", None);
        assert!(unavailable.http_status.is_none());
    }

    #[test]
    fn serialises_final_not_final_url() {
        let landing = Landing::new("https://x.com/a", "https://x.com/b", Some(200));
        let json = serde_json::to_value(&landing).unwrap();
        assert_eq!(json["requested"], "https://x.com/a");
        assert_eq!(json["final"], "https://x.com/b");
        assert_eq!(json["redirected"], true);
        assert_eq!(json["http_status"], 200);
        assert!(json.get("final_url").is_none(), "must not emit a `final_url` key");
    }

    #[test]
    fn attach_does_not_overwrite_an_existing_hint() {
        let landing = Landing::new("https://x.com/a", "https://x.com/login", Some(200));
        let mut out = serde_json::json!({"ok": true, "hint": "something more specific"});
        landing.attach(&mut out);
        assert_eq!(out["hint"], "something more specific");
        assert_eq!(out["landed"]["final"], "https://x.com/login");
    }

    #[test]
    fn text_line_is_silent_when_nothing_moved() {
        let straight = Landing::new("https://x.com/a", "https://x.com/a", Some(200));
        assert!(straight.text_line().is_none());

        let moved = Landing::new("https://x.com/a", "https://x.com/b", Some(301));
        let line = moved.text_line().unwrap();
        assert!(line.contains("redirected from https://x.com/a"), "got {line:?}");
        assert!(line.contains("HTTP 301"), "got {line:?}");
    }
}
