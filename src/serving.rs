//! What answered, as opposed to where the navigation landed.
//!
//! `landing.rs` settled *where* a `goto` ended up. It said nothing about *what* was there, and
//! three shapes measured on real sites were indistinguishable from a successful load:
//!
//! - `cnrs.fr` answered `http_status: 200`, `ok: true`, and a document reading "The requested
//!   URL was rejected. Please consult with your administrator." — an F5 ASM refusal served
//!   with a success status.
//! - `nowsecure.nl` answered `http_status: 200` with a Cloudflare challenge widget as the
//!   only thing in the document.
//! - `leboncoin.fr` answered `http_status: 403` with a `DataDome` frame, `ok: true`, and no
//!   judgement of any kind on the response. On a sweep of 92 domains, 7 behaved this way.
//!
//! # The rule
//!
//! One token, `serving`, from a closed set of five, on every `landed`. It is decided from two
//! measurements and nothing else: the HTTP status the browser reported, and a [`PageShape`]
//! read from the settled document — the `src` of its frames and scripts, how many form
//! controls and how many links there are outside its frames, how many scripts it loads, and
//! how much text it holds.
//!
//! | word | when | measured or inferred |
//! |---|---|---|
//! | `challenge` | a frame or script from a known anti-bot vendor, no form control, under [`TEXT_FLOOR`] characters | measured (the vendor's own host) |
//! | `error` | the status is 4xx or 5xx | measured (the server's own answer) |
//! | `nothing_actionable` | no control, no link, no script, under [`TEXT_FLOOR`] characters | measured shape, ambiguous meaning |
//! | `unreadable` | the shape probe did not run | an absence, never a guess |
//! | `page` | none of the above | **not a certificate** — see below |
//!
//! Every condition is a conjunction and every one of them was calibrated against a real page
//! that broke the previous version of the rule. `PageShape` says which page and what it
//! measured.
//!
//! Ranked in that order, and the two rungs that can disagree are the substance:
//!
//! **`challenge` above `error`.** `leboncoin.fr` is both at once. "403" reads as an
//! authorization problem and sends an agent looking for credentials; the frame names the
//! mechanism and the recovery (`--connect` to a real Chrome), which the status cannot express.
//! Both facts stay on the response — `http_status: 403` sits beside `serving: "challenge"` —
//! so ranking one costs the caller nothing.
//!
//! **`error` above `nothing_actionable`.** A status is the server's own statement about what
//! it did; the shape of a document is our inference about what that means. A 404 page with a
//! nav bar and a search box is `error`, which is the news; a 404 page with nothing on it is
//! still `error`, because the code is the more specific thing to act on and the shape adds
//! nothing to it.
//!
//! # What `page` does and does not claim
//!
//! `page` means no measurement contradicted the load. It is the absence of evidence, not a
//! certificate: a paywall, a cookie wall, a captcha vendor not in [`CHALLENGE_ORIGINS`], and a
//! page whose content arrived after the settle probe stopped all read as `page`. The rule
//! leans that way on purpose. Declaring a usable page blocked makes an agent abandon work it
//! could have done, which is silent and expensive; staying quiet leaves it exactly where it
//! was before this module existed.
//!
//! # The one false positive it still produces, measured
//!
//! `nothing_actionable` fires on a page whose FIRST paint is genuinely empty. Measured on
//! `www.amazon.fr`, three consecutive runs: `nothing_actionable`, `page`, `page` — on the
//! first, the document `goto`'s settle probe saw had no control, no link and no `<script src>`
//! at all, and the follow-up read seconds later still showed only 6 scripts. One domain in a
//! 30-domain sweep. It is not fixable from here: the word describes the document at the moment
//! it was measured, and at that moment it was true. What keeps the cost to one wasted command
//! is that the word is a measurement rather than a claim of a block, and the hint names this
//! case explicitly — an agent that follows it runs `inspect` and finds the page. Waiting and
//! re-measuring was rejected: it would put a second observation window on the path of every
//! genuine refusal to rescue a page the settle probe should have waited for.
//!
//! # What it will not detect, stated
//!
//! - A refusal notice with enough prose to clear [`TEXT_FLOOR`], one carrying a real link (a
//!   "contact support" anchor is a thing to act on, and this module counts it as one), or one
//!   that loads a script. The script condition is the price of not flagging every page that
//!   had not finished rendering, and it is paid knowingly: an appliance refusal that ships a
//!   script now reads as `page`.
//! - An anti-bot vendor whose host is not in the table, and every vendor that ships its
//!   challenge inline — no frame, no external script — rather than from its own origin
//!   (Kasada, and `PerimeterX` in some deployments). Those fall through to
//!   `nothing_actionable` when the document is otherwise empty: silence about the mechanism,
//!   not a false claim about it.
//! - A vendor resource past the probe's cap. Frames are read to 20 and scripts to 60, and a
//!   document carrying more than 60 scripts is not an interstitial.
//! - A soft 404: a site answering 200 with "we could not find that" and its normal
//!   navigation. Nothing here can tell that from the page it was asked for.
//! - A cookie or consent wall, which is actionable by construction.
//! - Anything inside an iframe. The shape probe counts what is outside them, so a document
//!   whose whole content is a same-origin frame reads as `nothing_actionable`.

use serde::Serialize;

/// How much text a document may hold and still be called "nothing to act on".
///
/// The F5 refusal measured on `cnrs.fr` holds ~150 characters; a Cloudflare interstitial
/// holds almost none. 512 is roughly eighty words — below the length of anything written to
/// be read, and above every refusal notice measured here. Text is counted up to a cap and
/// *over*-counts (hidden text and text in collapsed regions are included), which biases the
/// answer towards `page`, which is the direction this module errs in.
pub const TEXT_FLOOR: u32 = 512;

/// Resource origins that belong to an anti-bot challenge, matched on the vendor's own host
/// and, where the host is shared, a path prefix.
///
/// A hostname is not a keyword. It is chosen by the vendor, identical on every site that
/// deploys them, unaffected by the page's language, and not written by whoever is trying to
/// describe their own block page — which is what makes a list of *words* like "Request
/// Rejected" or "Access Denied" fragile in a way this is not.
///
/// **Scripts as well as frames, and the reason is measured.** The first version matched frame
/// `src` only, on the strength of `leboncoin.fr`, whose `DataDome` frame really does carry
/// `https://geo.captcha-delivery.com/captcha/?…`. On `nowsecure.nl` — the Cloudflare case in
/// the same brief — the interstitial's only `<iframe>` reports `src: ""`: Turnstile creates an
/// `about:blank` frame and injects into it, and the one vendor-hosted URL in the document is
/// `<script src="https://challenges.cloudflare.com/turnstile/v0/api.js">`. Frames alone
/// reported `nothing_actionable` there, which is true and names no mechanism. A script's host
/// is the same class of evidence as a frame's — vendor-controlled, not page content — and the
/// guard that stops either from over-firing is the same one: nothing else to act on.
///
/// The first two entries were measured in this task. The rest are the published origins of
/// the other widely deployed vendors and were not reproduced here; a wrong entry can only fire
/// on a document that has nothing else to act on, where `nothing_actionable` would otherwise
/// have been the answer.
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
    /// Nothing measured contradicts the page having been served. Not a certificate: see the
    /// module docs.
    Page,
    /// A known anti-bot vendor's frame or script is in the document and there is nothing else
    /// in it to act on.
    Challenge,
    /// The server answered with a 4xx or 5xx status.
    Error,
    /// The document offers nothing to act on and holds almost no text. A measurement of the
    /// document, deliberately not a claim that the caller was blocked.
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

/// The settled document, as measured in the page by the read `goto` already performs.
///
/// Pure data: `commands::goto` fills it from one `Runtime.evaluate`, and every decision below
/// is taken from these five numbers so it can be tested without Chrome.
///
/// Controls and links are counted separately rather than as one "actionable" total, and the
/// separation is what tells an interstitial from a page. Measured on `npmjs.com`: Cloudflare's
/// full block page carries two links — to `cloudflare.com` and to its privacy policy — and
/// nothing else, so a single total was never zero and the page fell through to `error`, whose
/// 403 reads as an authorization problem. A page that carries a captcha *and* is usable
/// carries FORM CONTROLS, because that is what a captcha is deployed to protect.
#[derive(Debug, Clone, Default)]
pub struct PageShape {
    /// The `src` of every frame and every external script in the top document, query string
    /// stripped, capped. Two element kinds because the two vendors measured here use one
    /// each — see [`CHALLENGE_ORIGINS`].
    pub resource_urls: Vec<String>,
    /// Form controls outside any frame: `input` (not hidden), `textarea`, `select`, `button`,
    /// `[role=button]`, `[contenteditable]`.
    pub controls: u32,
    /// Links outside any frame whose href resolves to http(s). A `javascript:` link is not a
    /// destination and is not counted — the F5 refusal's only anchor is one.
    pub links: u32,
    /// `<script src>` elements. Zero is the signal: an edge appliance generates its refusal
    /// without reaching the origin's asset pipeline, so those pages are self-contained.
    /// Measured on `cnrs.fr`: 0 scripts, 0 stylesheets, 0 images.
    pub scripts: u32,
    /// Characters of text outside `<script>`/`<style>`/`<noscript>`/`<template>`, counted up
    /// to a cap.
    pub text_length: u32,
}

impl PageShape {
    /// Read the shape out of the probe's JSON, or `None` when the probe returned nothing.
    ///
    /// Fails to `None` rather than to a default: a zero-valued `PageShape` would be read as
    /// "an empty document", which is the strongest thing this module can say, from having
    /// measured nothing at all.
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

/// The host of a challenge vendor's frame or script in this document, if there is one.
///
/// Returns the vendor's host as the page spells it, so the hint and the response can name the
/// evidence rather than the conclusion.
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

/// Whether `host` is the vendor's host or a subdomain of it.
///
/// A suffix test, not a substring one: `newassets.hcaptcha.com` is hCaptcha and
/// `nothcaptcha.com` is somebody else.
fn host_matches(host: &str, vendor: &str) -> bool {
    host == vendor || host.strip_suffix(vendor).is_some_and(|head| head.ends_with('.'))
}

/// What was served, and the evidence for it.
#[derive(Debug, Clone, Serialize)]
pub struct Assessment {
    pub serving: Serving,
    /// The host of a challenge frame found in the document, whatever the verdict.
    ///
    /// Present on a `page` too, and deliberately: a Turnstile widget on a login form is a
    /// fact worth having when the submit later fails, and hiding a measurement because it
    /// could be misread is how the leak this module fixes started. `serving` is the branch;
    /// this is the evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub challenge_from: Option<String>,
    /// Behind `nothing_actionable`, for the hint. Not serialised: it is evidence for a
    /// sentence the hint already spells out, and a field that only means something under one
    /// of five words is a rule every reader has to learn.
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
        // The probe did not run. A 4xx is still the server's own statement and outranks our
        // ignorance about the document.
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
        // Nothing to act on, almost no text, and nothing loaded from anywhere else. The last
        // condition is what a self-contained refusal notice has and a page whose content had
        // not arrived yet does not: measured on `amazon.fr`, whose shell reports zero controls
        // and zero links before it hydrates and 21 `<script src>` from the very first byte.
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
                self.challenge_from.as_deref().unwrap_or("an anti-bot vendor")
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

    /// What to do about it, under the three rules in `hints.rs`: one fact, one imperative
    /// command with this invocation's real values in it, and no invitation to repeat an
    /// action blind.
    ///
    /// Kept under 400 characters on purpose. `render.rs` carries a second, shorter table for
    /// verdict hints because the full ones wrap to seven terminal lines and bury the news;
    /// writing these short enough to serve JSON and a terminal alike avoids a second table
    /// that could fall out of step with the first.
    #[must_use]
    pub fn hint(&self, http_status: Option<u16>, url: &str, browser: &str) -> Option<String> {
        let run = invocation(browser);
        match self.serving {
            Serving::Page => None,
            Serving::Challenge => Some(format!(
                "A challenge frame from {} is the only thing here to act on, so the page asked \
                 for was not served. --stealth does not defeat these: they fingerprint bundled \
                 Chromium. Run `{run} --connect http://127.0.0.1:9222 goto {url}` against a \
                 Chrome started with --remote-debugging-port=9222. Navigating again unchanged \
                 gets the same challenge.",
                self.challenge_from.as_deref().unwrap_or("an anti-bot vendor")
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

/// One hint per class of status, because the recoveries are not the same shape.
///
/// A 404 is a fact about the URL, a 403 about who is asking, a 429 about how often, and a 5xx
/// about the server. Collapsing them into "the server answered {code}" would state a fact and
/// stop there, which is rule 1 without rule 2.
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
            crate::landing::origin_of(url).unwrap_or_else(|| url.to_string())
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

/// The invocation prefix that reaches THIS session's browser.
///
/// The same rule as `hints::invocation`, and duplicated rather than shared because that one is
/// private to a module about error messages and this one is about a successful response.
fn invocation(browser: &str) -> String {
    if browser == "default" {
        "chrome-agent".to_string()
    } else {
        format!("chrome-agent --browser {browser}")
    }
}

#[cfg(test)]
mod tests {
    use super::{assess, host_matches, PageShape, Serving, TEXT_FLOOR};

    /// Every shape below is written as it was measured on a real page, so a threshold that
    /// moves has to be argued against a site rather than against a number.
    struct Shape;
    impl Shape {
        /// `cnrs.fr`, F5 ASM: HTTP 200, ~150 characters, one `javascript:` anchor the probe
        /// does not count, and nothing loaded from anywhere.
        fn f5_refusal() -> PageShape {
            PageShape { resource_urls: vec![], controls: 0, links: 0, scripts: 0, text_length: 152 }
        }
        /// `nowsecure.nl`: the vendor URL on a `<script>`, an `<iframe>` with an empty `src`.
        fn cloudflare_interstitial() -> PageShape {
            PageShape {
                resource_urls: vec![CLOUDFLARE.into()],
                controls: 0,
                links: 0,
                scripts: 2,
                text_length: 38,
            }
        }
        /// `npmjs.com`: Cloudflare's full block page — two links of its own, no control.
        fn cloudflare_block_page() -> PageShape {
            PageShape {
                resource_urls: vec![CLOUDFLARE.into()],
                controls: 0,
                links: 2,
                scripts: 3,
                text_length: 250,
            }
        }
        /// `leboncoin.fr`: the vendor URL on the frame, HTTP 403 beside it.
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
        /// `amazon.fr` before it hydrates: nothing to act on, and 21 scripts from the first
        /// byte. Reported `nothing_actionable` once in three runs before the script condition.
        fn unhydrated_shell() -> PageShape {
            PageShape { resource_urls: vec![], controls: 0, links: 0, scripts: 21, text_length: 40 }
        }
        /// An ordinary page.
        fn usable(text_length: u32) -> PageShape {
            PageShape { resource_urls: vec![], controls: 6, links: 40, scripts: 8, text_length }
        }
    }

    /// What `nowsecure.nl` actually loads. The vendor URL is on a `<script>`, not on the
    /// interstitial's `<iframe>`, whose `src` is empty.
    const CLOUDFLARE: &str = "https://challenges.cloudflare.com/turnstile/v0/api.js";
    /// What `leboncoin.fr` actually loads: here the URL is on the frame.
    const DATADOME: &str = "https://geo.captcha-delivery.com/captcha/";

    /// The shape the tool used to report as an ordinary load.
    #[test]
    fn an_f5_refusal_served_with_200_is_nothing_actionable() {
        let assessment = assess(Some(200), Some(&Shape::f5_refusal()));
        assert_eq!(assessment.serving, Serving::NothingActionable);
        assert!(assessment.challenge_from.is_none());
    }

    /// `nowsecure.nl`: HTTP 200 and a challenge widget as the whole document.
    #[test]
    fn a_cloudflare_interstitial_is_a_challenge_and_names_its_host() {
        let assessment = assess(Some(200), Some(&Shape::cloudflare_interstitial()));
        assert_eq!(assessment.serving, Serving::Challenge);
        assert_eq!(assessment.challenge_from.as_deref(), Some("challenges.cloudflare.com"));
    }

    /// `npmjs.com`: the block page carries two links of Cloudflare's own. Counting links and
    /// controls as one total put this at `error`, whose 403 sends an agent after credentials.
    #[test]
    fn a_block_page_that_links_to_its_own_vendor_is_still_a_challenge() {
        let assessment = assess(Some(403), Some(&Shape::cloudflare_block_page()));
        assert_eq!(assessment.serving, Serving::Challenge, "two vendor links are not a page");
    }

    /// `leboncoin.fr`: both facts at once. The frame names the mechanism, the status names a
    /// symptom, and the status is still on the response.
    #[test]
    fn a_challenge_outranks_the_status_it_arrived_with() {
        let assessment = assess(Some(403), Some(&Shape::datadome()));
        assert_eq!(assessment.serving, Serving::Challenge);
        assert_eq!(assessment.challenge_from.as_deref(), Some("geo.captcha-delivery.com"));
    }

    /// The expensive false positive: a Turnstile widget on a login form. The page is usable
    /// and the vendor is still reported, because it is a true thing about the document.
    #[test]
    fn a_challenge_widget_beside_a_form_is_not_a_challenge_page() {
        let assessment = assess(Some(200), Some(&Shape::login_with_turnstile()));
        assert_eq!(assessment.serving, Serving::Page);
        assert_eq!(assessment.challenge_from.as_deref(), Some("challenges.cloudflare.com"));
        assert!(assessment.hint(Some(200), "https://x.com/login", "default").is_none());
    }

    /// The other one: a page whose content had not arrived when the settle probe stopped. It
    /// loads scripts from the first byte; a refusal notice generated at the edge does not.
    #[test]
    fn a_page_that_has_not_rendered_yet_is_not_reported_as_empty() {
        assert_eq!(assess(Some(200), Some(&Shape::unhydrated_shell())).serving, Serving::Page);
    }

    /// And prose with nothing to click, on the threshold itself.
    #[test]
    fn prose_with_no_links_is_still_a_page() {
        let mut prose = Shape::f5_refusal();
        prose.text_length = TEXT_FLOOR;
        assert_eq!(assess(Some(200), Some(&prose)).serving, Serving::Page);
        prose.text_length = TEXT_FLOOR - 1;
        assert_eq!(assess(Some(200), Some(&prose)).serving, Serving::NothingActionable);
    }

    /// A status is the server's own statement; a shape is our inference about one. A 404 with
    /// a nav bar and a 404 with nothing on it are both `error`.
    #[test]
    fn a_status_outranks_the_shape_of_the_document_it_came_with() {
        assert_eq!(assess(Some(404), Some(&Shape::usable(900))).serving, Serving::Error);
        assert_eq!(assess(Some(404), Some(&Shape::f5_refusal())).serving, Serving::Error);
        assert_eq!(assess(Some(503), Some(&Shape::f5_refusal())).serving, Serving::Error);
    }

    /// A status this module has no opinion about leaves the shape to answer.
    #[test]
    fn a_2xx_or_3xx_status_decides_nothing_by_itself() {
        assert_eq!(assess(Some(204), Some(&Shape::usable(700))).serving, Serving::Page);
        assert_eq!(assess(None, Some(&Shape::usable(700))).serving, Serving::Page);
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
            PageShape::from_probe(Some(&serde_json::json!({"controls": 0, "links": 0, "text": 3})))
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

    /// A vendor host is matched as a host, not as a substring. `nothcaptcha.com` is somebody
    /// else, and `www.google.com` only counts on the reCAPTCHA path.
    #[test]
    fn a_vendor_host_is_matched_as_a_host() {
        assert!(host_matches("newassets.hcaptcha.com", "hcaptcha.com"));
        assert!(host_matches("hcaptcha.com", "hcaptcha.com"));
        assert!(!host_matches("nothcaptcha.com", "hcaptcha.com"));
        assert!(!host_matches("hcaptcha.com.evil.test", "hcaptcha.com"));

        // `www.google.com` serves everything, so only the path tells a captcha from a map.
        let mut interstitial = Shape::cloudflare_interstitial();
        interstitial.resource_urls = vec!["https://www.google.com/recaptcha/api2/anchor".into()];
        assert_eq!(assess(Some(200), Some(&interstitial)).serving, Serving::Challenge);

        let mut map = Shape::f5_refusal();
        map.resource_urls = vec!["https://www.google.com/maps/embed".into()];
        let maps = assess(Some(200), Some(&map));
        assert_eq!(maps.serving, Serving::NothingActionable, "an embedded map is not a challenge");
        assert!(maps.challenge_from.is_none());
    }

    /// Rule 2 of the `hints.rs` contract: a command in a hint reaches the browser the caller
    /// is actually driving, and hands back no placeholder.
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
                assert!(!hint.contains(placeholder), "hint hands back {placeholder}: {hint}");
            }
            for forbidden in ["Try running the command again", "run the command again"] {
                assert!(!hint.contains(forbidden), "hint invites a blind retry: {hint}");
            }
            assert!(hint.ends_with('.'), "hint stops mid-sentence: {hint}");
            // The budget is on the wording, not on the caller's own URL: a 200-character URL
            // is the caller's string and it goes in whole. Four terminal lines is the
            // ceiling — the reason `render.rs` keeps a second, shorter hint table, and the
            // reason this module does not need one.
            let wording = hint.replace("https://site.test/a/b", "").len();
            assert!(wording < 400, "hint wording is {wording} characters, too long: {hint}");
        }
        // `unreadable` is reached only through an absent shape.
        let blind = assess(None, None);
        assert!(blind.hint(None, "https://site.test/", "agent-7").is_some());
    }

    /// The whole point of splitting them: one status class, one recovery.
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
            assert!(hint.contains(&[403, 404, 429, 500][i].to_string()), "{hint}");
            for (j, other) in hints.iter().enumerate() {
                assert!(i == j || hint != other, "two status classes share a hint: {hint}");
            }
        }
        assert!(hints[1].contains("https://site.test/"), "the 404 hint names the site root: {}", hints[1]);
    }

    /// Text mode prints a gloss beside the word, and nothing at all on a page that answered.
    #[test]
    fn the_gloss_states_the_measurement_and_stays_quiet_on_a_page() {
        assert!(assess(Some(200), Some(&Shape::usable(900))).gloss().is_none());
        let challenge = assess(Some(200), Some(&Shape::datadome())).gloss().expect("a gloss");
        assert!(challenge.contains("geo.captcha-delivery.com"), "{challenge}");
        let empty = assess(Some(200), Some(&Shape::f5_refusal())).gloss().expect("a gloss");
        assert!(empty.contains("152"), "the gloss carries the measurement: {empty}");
    }

    #[test]
    fn the_word_serialises_in_snake_case() {
        let assessment = assess(Some(200), Some(&Shape::f5_refusal()));
        let json = serde_json::to_value(&assessment).expect("serialises");
        assert_eq!(json["serving"], "nothing_actionable");
        assert!(json.get("challenge_from").is_none(), "absent when nothing was found");
        assert!(json.get("text_length").is_none(), "evidence for the hint, not a field");
    }
}
