//! Where a navigation actually ended up, as opposed to where it was aimed.
//!
//! `Landing` reports what was asked for, what answered, whether those differ, and the HTTP
//! status when the page gives one. *What* was there is settled in `serving.rs`. Pure.

use serde::Serialize;

use crate::serving::Assessment;

/// What a navigation reports about itself.
#[derive(Debug, Clone, Serialize)]
pub struct Landing {
    /// The URL as the tool sent it, after `goto`'s `https://` prefixing — not the caller's raw
    /// argument, which would make every `goto example.com` a redirect.
    pub requested: String,
    /// `location.href` once the page settled.
    #[serde(rename = "final")]
    pub final_url: String,
    pub redirected: bool,
    /// Status of the final document, from Navigation Timing. Absent — never zero, never guessed
    /// — when the page exposes none. What the browser answered, not proof of an HTTP response (a
    /// `file://` document reports 200), and the last hop only, so a followed `302` reads as 200.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// What answered: `serving`, and `challenge_from` when a vendor's frame was found.
    /// Flattened so one `goto` answers both questions in one object.
    #[serde(flatten)]
    pub served: Assessment,
}

/// Path segments that suggest an authentication wall. A guess about a URL, not a reading of the
/// page; kept small, so every entry is a word that only appears when a site asks who you are.
const AUTH_WALL_SEGMENTS: &[&str] =
    &["login", "log-in", "signin", "sign-in", "sign_in", "auth", "sso"];

impl Landing {
    /// Build a landing from the requested and settled URLs and the shape of what answered.
    pub fn new(
        requested: &str,
        final_url: &str,
        http_status: Option<u16>,
        shape: Option<&crate::serving::PageShape>,
    ) -> Self {
        let http_status = status_or_none(http_status);
        Self {
            requested: requested.to_string(),
            final_url: final_url.to_string(),
            redirected: is_redirect(requested, final_url),
            http_status,
            served: crate::serving::assess(http_status, shape),
        }
    }

    /// What to do about this landing, or nothing when there is nothing to say. Two judgements,
    /// one field: what was *served* wins, being measured rather than a guess about a URL, and
    /// the auth-wall half fires only on a redirect.
    pub fn hint(&self, browser: &str) -> Option<String> {
        if let Some(hint) = self.served.hint(self.http_status, &self.final_url, browser) {
            return Some(hint);
        }
        if !self.redirected {
            return None;
        }
        let segment = auth_wall_segment(&self.final_url)?;
        let run = if browser == "default" {
            "chrome-agent".to_string()
        } else {
            format!("chrome-agent --browser {browser}")
        };
        Some(format!(
            "The redirect landed on a path containing '{segment}', which often means the \
             session expired. This is a guess from the URL, not a reading of the page: run \
             `{run} inspect` to see what is there, and re-authenticate if it is a login form."
        ))
    }

    /// Attach `landed` (and its hint) to a response object. Lives here, not in the three call
    /// sites, so CLI, pipe and batch cannot drift.
    pub fn attach(&self, out: &mut serde_json::Value, browser: &str) {
        if let Some(map) = out.as_object_mut() {
            map.insert(
                "landed".into(),
                serde_json::to_value(self).unwrap_or(serde_json::Value::Null),
            );
            if let Some(hint) = self.hint(browser) {
                map.entry("hint").or_insert_with(|| serde_json::json!(hint));
            }
        }
    }

    /// What a person reads in text mode, or nothing when the navigation went where it was told
    /// and the page that answered is the page.
    pub fn text_line(&self, browser: &str) -> Option<String> {
        let mut lines: Vec<String> = Vec::new();
        if self.redirected {
            let status = self
                .http_status
                .map_or_else(String::new, |code| format!(" (HTTP {code})"));
            lines.push(format!("redirected from {}{}", self.requested, status));
        }
        if let Some(gloss) = self.served.gloss() {
            lines.push(format!("serving: {} — {gloss}", self.served.serving.as_str()));
        }
        if lines.is_empty() {
            return None;
        }
        if let Some(hint) = self.hint(browser) {
            lines.push(format!("hint: {hint}"));
        }
        Some(lines.join("\n"))
    }

}

/// A status only counts when it could have come off the wire. Navigation Timing reports `0` for
/// a document with no HTTP response; reporting that as a status invents an answer.
fn status_or_none(raw: Option<u16>) -> Option<u16> {
    raw.filter(|code| (100..=599).contains(code))
}

/// Whether the page ended up somewhere other than where it was sent.
///
/// Ignored: the fragment (never sent), one trailing slash on the path, the default port, host
/// case (the path stays case-sensitive), an empty query. Reported: any change of scheme (an
/// `http`→`https` upgrade changes which cookies travel), of host, of path beyond that slash, and
/// any query change — a gained `?next=…` is the login bounce this exists to expose. Query
/// parameters are compared in order; reordering is a claim about server semantics.
pub fn is_redirect(requested: &str, final_url: &str) -> bool {
    comparison_key(requested) != comparison_key(final_url)
}

/// The part of a URL that has to change for a navigation to have been redirected. Hand-rolled
/// rather than a URL crate: the musl release keeps a pure-Rust dependency graph.
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

/// The host and path of a URL, path defaulting to `/`. Shared by `serving` and `hints`, since a
/// second copy is a second set of edge cases (credentials, port, slash in query).
#[must_use]
pub fn host_and_path(url: &str) -> Option<(&str, &str)> {
    let (_, rest) = url.split_once("://")?;
    let rest = rest.split('#').next().unwrap_or(rest);
    let rest = rest.split('?').next().unwrap_or(rest);
    let (authority, path) = rest.find('/').map_or((rest, "/"), |i| (&rest[..i], &rest[i..]));
    // Credentials and a port are not part of the host.
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = authority.split(':').next().unwrap_or(authority);
    (!host.is_empty()).then_some((host, path))
}

/// `scheme://authority/` of a URL, for a hint that sends the caller to the site root. The port
/// is kept: `http://localhost:3000/` is a different server from `http://localhost/`.
#[must_use]
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    (!authority.is_empty()).then(|| format!("{scheme}://{authority}/"))
}

/// The path segment that made the URL look like an auth wall, if any. Matched per segment on the
/// stem before the first `.`, so `/login.php` hits and `/authors/tolkien` does not.
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
    use super::{auth_wall_segment, host_and_path, is_redirect, origin_of, Landing};

    #[test]
    fn a_url_is_split_into_host_and_path() {
        assert_eq!(
            host_and_path("https://geo.captcha-delivery.com/captcha/?cid=x"),
            Some(("geo.captcha-delivery.com", "/captcha/"))
        );
        assert_eq!(host_and_path("https://host.test:8443"), Some(("host.test", "/")));
        assert_eq!(host_and_path("https://user@host.test/a#b"), Some(("host.test", "/a")));
        assert_eq!(host_and_path("about:blank"), None);
        assert_eq!(host_and_path("https:///nohost"), None);
    }

    #[test]
    fn the_site_root_keeps_the_port_it_was_given() {
        assert_eq!(origin_of("https://x.test/a/b?c=1").as_deref(), Some("https://x.test/"));
        assert_eq!(origin_of("http://x.test:8080").as_deref(), Some("http://x.test:8080/"));
        assert_eq!(origin_of("not a url"), None);
    }

    #[test]
    fn same_url_is_not_a_redirect() {
        assert!(!is_redirect(
            "https://example.com/orders",
            "https://example.com/orders"
        ));
    }

    #[test]
    fn fragment_only_change_is_not_a_redirect() {
        // The fragment never leaves the browser.
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
        // Paths are case-sensitive on most servers: a different resource.
        assert!(is_redirect("https://example.com/A", "https://example.com/a"));
    }

    #[test]
    fn gained_query_is_a_redirect() {
        // The login bounce this field exists to expose.
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
        // Substring matching would make every author page a login wall.
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

    /// A document that answered normally. Tests about the *redirect* half need one, or the
    /// `serving` half speaks instead and the assertion judges the wrong thing.
    fn served() -> crate::serving::PageShape {
        crate::serving::PageShape {
            resource_urls: Vec::new(),
            controls: 6,
            links: 12,
            scripts: 4,
            text_length: 4096,
        }
    }

    #[test]
    fn hint_fires_only_on_a_redirect_to_an_auth_wall() {
        let bounced = Landing::new(
            "https://app.example.com/orders",
            "https://app.example.com/login?next=/orders",
            Some(200),
            Some(&served()),
        );
        assert!(bounced.redirected);
        assert!(bounced.hint("default").is_some());

        // An ordinary redirect says nothing about authentication.
        let ordinary = Landing::new(
            "https://example.com/start",
            "https://example.com/settled",
            Some(200),
            Some(&served()),
        );
        assert!(ordinary.redirected);
        assert!(ordinary.hint("default").is_none());

        // A caller who asked for the login page already knows.
        let deliberate =
            Landing::new("https://x.com/login", "https://x.com/login", None, Some(&served()));
        assert!(!deliberate.redirected);
        assert!(deliberate.hint("default").is_none());
    }

    /// Rule 2 of the hint contract reaches this module: under `--browser`, a bare `inspect`
    /// would aim at another agent's session.
    #[test]
    fn the_auth_wall_hint_names_this_invocation_s_browser() {
        let bounced = Landing::new(
            "https://x.com/orders",
            "https://x.com/login?next=/orders",
            Some(200),
            Some(&served()),
        );
        let hint = bounced.hint("agent-7").expect("a hint");
        assert!(hint.contains("`chrome-agent --browser agent-7 inspect`"), "{hint}");
    }

    /// Two judgements, one hint field. What was served is measured; the auth wall is a guess
    /// about a string in a URL.
    #[test]
    fn what_was_served_outranks_the_auth_wall_guess() {
        let blocked = Landing::new(
            "https://x.com/orders",
            "https://x.com/login?next=/orders",
            Some(403),
            Some(&served()),
        );
        let hint = blocked.hint("default").expect("a hint");
        assert!(hint.contains("403"), "the measured half comes first: {hint}");
        assert!(!hint.contains("guess"), "{hint}");
    }

    #[test]
    fn status_zero_is_absent_rather_than_reported() {
        // A document with no HTTP response reports 0, which means "I don't know".
        let landing =
            Landing::new("file:///tmp/a.html", "file:///tmp/a.html", Some(0), Some(&served()));
        assert!(landing.http_status.is_none());
        let json = serde_json::to_value(&landing).unwrap();
        assert!(json.get("http_status").is_none());

        let unavailable = Landing::new("https://x.com/a", "https://x.com/a", None, Some(&served()));
        assert!(unavailable.http_status.is_none());
    }

    /// A status outside 100..=599 is dropped before `serving` sees it, so a `file://` document
    /// reporting 0 cannot come back as `error`.
    #[test]
    fn an_implausible_status_cannot_produce_an_error_verdict() {
        let landing = Landing::new("file:///tmp/a.html", "file:///tmp/a.html", Some(0), Some(&served()));
        assert_eq!(landing.served.serving, crate::serving::Serving::Page);
    }

    #[test]
    fn serialises_final_not_final_url() {
        let landing = Landing::new("https://x.com/a", "https://x.com/b", Some(200), Some(&served()));
        let json = serde_json::to_value(&landing).unwrap();
        assert_eq!(json["requested"], "https://x.com/a");
        assert_eq!(json["final"], "https://x.com/b");
        assert_eq!(json["redirected"], true);
        assert_eq!(json["http_status"], 200);
        assert_eq!(json["serving"], "page", "`serving` is flattened onto `landed`");
        assert!(json.get("final_url").is_none(), "must not emit a `final_url` key");
        assert!(json.get("served").is_none(), "the carrier must not appear as a key");
    }

    #[test]
    fn attach_does_not_overwrite_an_existing_hint() {
        let landing =
            Landing::new("https://x.com/a", "https://x.com/login", Some(200), Some(&served()));
        let mut out = serde_json::json!({"ok": true, "hint": "something more specific"});
        landing.attach(&mut out, "default");
        assert_eq!(out["hint"], "something more specific");
        assert_eq!(out["landed"]["final"], "https://x.com/login");
    }

    #[test]
    fn text_line_is_silent_when_nothing_moved() {
        let straight = Landing::new("https://x.com/a", "https://x.com/a", Some(200), Some(&served()));
        assert!(straight.text_line("default").is_none());

        let moved = Landing::new("https://x.com/a", "https://x.com/b", Some(301), Some(&served()));
        let line = moved.text_line("default").unwrap();
        assert!(line.contains("redirected from https://x.com/a"), "got {line:?}");
        assert!(line.contains("HTTP 301"), "got {line:?}");
    }

    /// A refusal served on the URL asked for has nothing to say about redirects and everything
    /// to say about what is there.
    #[test]
    fn text_line_speaks_up_when_only_what_was_served_moved() {
        let refused = Landing::new(
            "https://x.com/a",
            "https://x.com/a",
            Some(200),
            Some(&crate::serving::PageShape {
                resource_urls: Vec::new(),
                controls: 0,
                links: 0,
                scripts: 0,
                text_length: 152,
            }),
        );
        let line = refused.text_line("default").expect("a line");
        assert!(!line.contains("redirected"), "nothing moved: {line}");
        assert!(line.contains("serving: nothing_actionable"), "{line}");
        assert!(line.contains("152 characters"), "the measurement, not the conclusion: {line}");
        assert!(line.contains("hint:"), "{line}");
    }
}
