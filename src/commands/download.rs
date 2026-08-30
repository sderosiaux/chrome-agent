use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};

use crate::cdp::client::CdpClient;
use crate::element_ref::ElementRef;

// The URL mechanism and the naming both paths share live in `download_fetch`. Re-exported so
// every `commands::download::X` call site still resolves; only the three with callers, since a
// `pub use` nothing reads is a warning in a binary crate.
pub use super::download_fetch::{DEFAULT_MAX_BYTES, resolve_named_path, run};

/// What the caller pointed `download` at. Both mechanisms produce the same contract: a file at
/// a named path, 0600, with its size and the server's proposed name.
///
/// Exactly one of the three, refused rather than ranked — picking for a caller who gave two
/// would silently ignore half the invocation.
pub enum Target<'a> {
    /// Fetch this URL inside the page. Auth-preserving, and needs no click.
    Url(&'a str),
    /// Click this uid and capture whatever download it produces.
    Uid(&'a str),
    /// Click the element this selector resolves to, and capture the download.
    Selector(&'a str),
}

impl<'a> Target<'a> {
    pub fn parse(
        url: Option<&'a str>,
        uid: Option<&'a str>,
        selector: Option<&'a str>,
    ) -> Result<Self, crate::BoxError> {
        match (url, uid, selector) {
            (Some(url), None, None) => Ok(Self::Url(url)),
            (None, Some(uid), None) => Ok(Self::Uid(uid)),
            (None, None, Some(selector)) => Ok(Self::Selector(selector)),
            (None, None, None) => Err("download: provide a URL, --uid, or --selector".into()),
            _ => Err("download: provide only one of a URL, --uid, or --selector".into()),
        }
    }

    const fn is_click(&self) -> bool {
        matches!(self, Self::Uid(_) | Self::Selector(_))
    }
}

/// What one `download` did, in the shape all three modes render.
///
/// Branch on `downloaded`, not `ok`: a delivered click is never an error, because the only
/// recovery an error invites is a second click the page cannot distinguish from a deliberate one.
pub struct Outcome {
    /// `"fetch"` or `"click"`, so a caller reading a failure knows which mechanism it is about.
    pub via: &'static str,
    pub downloaded: bool,
    pub path: Option<String>,
    pub bytes: Option<u64>,
    pub mime: Option<String>,
    /// What `Browser.downloadWillBegin` proposed, before sanitising. Absent on the fetch path.
    pub suggested_filename: Option<String>,
    /// The address Chrome actually pulled from — often a `blob:` URL absent from the DOM.
    pub source_url: Option<String>,
    /// Where bytes this command could not account for were left, rather than deleted. Set only
    /// on the evidence-lost path; it is not a file this command claims to have downloaded.
    pub kept: Option<String>,
    /// The click's own report: `delivery`, `uid`, `role`, `name`, `aim`, `intercepted_by`.
    pub click: Option<Value>,
    pub message: String,
    pub waited_ms: Option<u64>,
    pub hint: Option<String>,
}

impl Outcome {
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut obj = json!({"ok": true, "via": self.via, "downloaded": self.downloaded});
        let map = obj.as_object_mut().expect("literal object");
        if let Some(path) = &self.path {
            map.insert("path".into(), json!(path));
        }
        if let Some(bytes) = self.bytes {
            map.insert("bytes".into(), json!(bytes));
        }
        if let Some(mime) = &self.mime {
            map.insert("mime".into(), json!(mime));
        }
        if let Some(name) = &self.suggested_filename {
            map.insert("suggested_filename".into(), json!(name));
        }
        if let Some(url) = &self.source_url {
            map.insert("source_url".into(), json!(url));
        }
        if let Some(kept) = &self.kept {
            map.insert("kept".into(), json!(kept));
        }
        if let Some(ms) = self.waited_ms {
            map.insert("observed_after_ms".into(), json!(ms));
        }
        map.insert("message".into(), json!(self.message));
        if let Some(fields) = self.click.as_ref().and_then(Value::as_object) {
            for (key, value) in fields {
                // `download` carries no verdict, so `verdict_hint` would name a vocabulary this
                // response does not have. What it had to say is folded into `hint` below.
                if key != "verdict_hint" {
                    map.insert(key.clone(), value.clone());
                }
            }
        }
        if let Some(hint) = &self.hint {
            map.insert("hint".into(), json!(hint));
        }
        obj
    }

    /// Non-`--json` output: the path on its own line when there is one, so a shell pipeline
    /// keeps working; otherwise the message and its hint, on stderr.
    pub fn print_text(&self) {
        if self.downloaded {
            let bytes = self.bytes.unwrap_or(0);
            match &self.mime {
                Some(mime) => out_line!(
                    "{} ({bytes} bytes, {mime})",
                    self.path.as_deref().unwrap_or("")
                ),
                None => out_line!("{} ({bytes} bytes)", self.path.as_deref().unwrap_or("")),
            }
            return;
        }
        eprintln!("{}", self.message);
        if let Some(receiver) = self
            .click
            .as_ref()
            .and_then(|c| c.get("intercepted_by"))
            .and_then(|r| r.get("uid"))
            .and_then(Value::as_str)
        {
            eprintln!("received by: {receiver}");
        }
        if let Some(hint) = &self.hint {
            eprintln!("hint: {hint}");
        }
    }
}

/// One `download`, however it was aimed: the six values a mode settles before dispatching.
/// Grouped because the click path threads all of them through two further frames.
pub struct Request<'a> {
    pub target: &'a Target<'a>,
    pub out: Option<&'a str>,
    pub timeout_secs: u64,
    pub max_bytes: usize,
    pub on_intercept: crate::hit_test::OnIntercept,
    pub browser: &'a str,
}

/// Run one `download`, whichever way it was aimed. The single entry point for CLI, pipe and
/// batch, so the two mechanisms cannot drift into two response shapes.
pub async fn dispatch(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    req: &Request<'_>,
) -> Result<Outcome, crate::BoxError> {
    let target = req.target;
    if !target.is_click() {
        let Target::Url(url) = target else {
            unreachable!("guarded by is_click")
        };
        let result = run(client, url, req.out, req.timeout_secs, req.max_bytes).await?;
        return Ok(Outcome {
            via: "fetch",
            downloaded: true,
            message: format!("{} ({} bytes, {})", result.path, result.bytes, result.mime),
            path: Some(result.path),
            bytes: Some(result.bytes as u64),
            mime: Some(result.mime),
            suggested_filename: None,
            source_url: Some((*url).to_string()),
            kept: None,
            click: None,
            waited_ms: None,
            hint: None,
        });
    }
    click_download(client, uid_map, req).await
}

/// The click half: arm, click, wait, place. The click is `element::click`'s — same hit test,
/// same `--on-intercept`, same refusal messages.
async fn click_download(
    client: &CdpClient,
    uid_map: &HashMap<String, ElementRef>,
    req: &Request<'_>,
) -> Result<Outcome, crate::BoxError> {
    // Before the click, and may fail the command: clicking unarmed forces a second click.
    let mut armed = super::download_click::arm(client).await?;

    let (dispatched, described) = match req.target {
        Target::Uid(uid) => (
            crate::element::click(client, uid_map, uid, req.on_intercept).await,
            format!("uid={uid}"),
        ),
        Target::Selector(selector) => (
            crate::element::click_selector(client, selector, req.on_intercept).await,
            format!("selector '{selector}'"),
        ),
        Target::Url(_) => unreachable!("guarded by is_click"),
    };
    let dispatched = match dispatched {
        Ok(dispatched) => dispatched,
        Err(error) => {
            let message = error.to_string();
            super::download_click::disarm(client).await;
            super::download_click::clean_up(&armed).await;
            return Err(message.into());
        }
    };

    // Nothing was sent (aim never settled, or `--on-intercept refuse`). The only branch where a
    // retry is safe: the page never saw an event.
    if !dispatched.sent {
        super::download_click::disarm(client).await;
        super::download_click::clean_up(&armed).await;
        let message = dispatched
            .refusal_message("click", &described)
            .unwrap_or_else(|| format!("Did not click {described}."));
        return Ok(Outcome {
            via: "click",
            downloaded: false,
            path: None,
            bytes: None,
            mime: None,
            suggested_filename: None,
            source_url: None,
            kept: None,
            hint: Some(crate::hints::undispatched_download_hint(req.browser)),
            click: Some(dispatched.report()),
            message,
            waited_ms: None,
        });
    }

    let transfer = super::download_click::collect(
        client,
        &mut armed,
        Duration::from_secs(req.timeout_secs),
        req.max_bytes as u64,
    )
    .await;
    super::download_click::disarm(client).await;
    // Move BEFORE the sweep, which deletes the directory the file is still in. The error is
    // reduced to a `String`: `BoxError` is not `Send` and would poison the await below.
    let placed = match &transfer {
        super::download_click::Transfer::Completed {
            began, temp_path, ..
        } => Some(
            super::download_click::place(temp_path, &began.suggested_filename, req.out)
                .map_err(|error| error.to_string()),
        ),
        _ => None,
    };
    super::download_click::clean_up(&armed).await;
    let placed = placed
        .transpose()
        .map_err(|error| -> crate::BoxError { error.into() })?;
    Ok(settle(transfer, placed, &described, req, &dispatched))
}

/// Turn what Chrome reported, and where the file ended up, into the response.
fn settle(
    transfer: super::download_click::Transfer,
    placed: Option<(String, u64)>,
    described: &str,
    req: &Request<'_>,
    dispatched: &crate::hit_test::Dispatched,
) -> Outcome {
    use super::download_click::{Cancelled, Transfer};

    let Request {
        timeout_secs,
        max_bytes,
        browser,
        ..
    } = *req;

    let base = |downloaded: bool, message: String, hint: Option<String>| Outcome {
        via: "click",
        downloaded,
        path: None,
        bytes: None,
        mime: None,
        suggested_filename: None,
        source_url: None,
        kept: None,
        click: Some(dispatched.report()),
        message,
        waited_ms: None,
        hint,
    };

    match transfer {
        Transfer::Completed { began, bytes, .. } => {
            let (path, on_disk) = placed.expect("a completed transfer is placed by the caller");
            let message = format!("{path} ({on_disk} bytes, from clicking {described})");
            Outcome {
                downloaded: true,
                path: Some(path),
                bytes: Some(if on_disk > 0 { on_disk } else { bytes }),
                suggested_filename: Some(began.suggested_filename),
                source_url: Some(began.url),
                ..base(true, message, None)
            }
        }
        Transfer::NeverBegan { waited_ms } => Outcome {
            waited_ms: Some(waited_ms),
            ..base(
                false,
                format!(
                    "Clicked {described} and no download began in the {timeout_secs}s that \
                     followed, so nothing was written."
                ),
                Some(crate::hints::no_download_hint(
                    browser,
                    timeout_secs,
                    dispatched,
                )),
            )
        },
        Transfer::EvidenceLost {
            began,
            dropped,
            kept,
            waited_ms,
        } => {
            // Never `downloaded: true` — no `completed` was seen, and a file on disk is not a
            // finished one. Never "no download began" either: that is the claim the drops made
            // unavailable. The response says what was lost and where the bytes are.
            let subject = began.as_ref().map_or_else(
                || "whether a download began cannot be told".to_string(),
                |began| {
                    format!(
                        "what became of the download it started ({}) cannot be told",
                        began.suggested_filename
                    )
                },
            );
            let where_ = kept.as_ref().map_or_else(
                || " Nothing had been written into the transfer directory.".to_string(),
                |path| format!(" What Chrome had written was kept at {}.", path.display()),
            );
            let message = format!(
                "Clicked {described} and {dropped} CDP event(s) were dropped before this command \
                 could read them, so {subject}.{where_}"
            );
            Outcome {
                waited_ms: Some(waited_ms),
                suggested_filename: began.as_ref().map(|b| b.suggested_filename.clone()),
                source_url: began.map(|b| b.url),
                kept: kept.as_ref().map(|path| path.display().to_string()),
                ..base(false, message, Some(evidence_lost_hint(kept.as_ref())))
            }
        }
        Transfer::Canceled { began, why } => {
            let message = match why {
                Cancelled::ExceededCap => format!(
                    "Clicked {described} and the download it started ({}) exceeded {max_bytes} \
                     bytes, so it was cancelled and nothing was written.",
                    began.suggested_filename
                ),
                Cancelled::ByBrowser => format!(
                    "Clicked {described} and Chrome cancelled the download it started ({}), so \
                     nothing was written.",
                    began.suggested_filename
                ),
            };
            let hint = match why {
                Cancelled::ExceededCap => Some(crate::hints::download_cap_hint(browser, max_bytes)),
                Cancelled::ByBrowser => Some(crate::hints::download_cancelled_hint(browser)),
            };
            Outcome {
                suggested_filename: Some(began.suggested_filename),
                source_url: Some(began.url),
                ..base(false, message, hint)
            }
        }
        Transfer::Unfinished {
            began,
            received,
            total,
            waited_ms,
        } => {
            let message = format!(
                "Clicked {described} and the download it started ({}) had {received} of {total} \
                 bytes after {timeout_secs}s, so it is incomplete and nothing was written.",
                began.suggested_filename
            );
            Outcome {
                waited_ms: Some(waited_ms),
                suggested_filename: Some(began.suggested_filename),
                source_url: Some(began.url),
                ..base(
                    false,
                    message,
                    Some(crate::hints::download_unfinished_hint(browser)),
                )
            }
        }
    }
}

/// The recovery when the events that would have answered were dropped.
///
/// Built here rather than in `hints`, because the one resolved thing it can name is a path this
/// invocation just created. It holds to the same three rules: one fact, one thing to do, and an
/// explicit refusal of the retry — the click already reached the page.
fn evidence_lost_hint(kept: Option<&PathBuf>) -> String {
    let what = kept.map_or_else(
        || {
            "Nothing reached the transfer directory, which is what an unstarted download looks \
             like, though the dropped events are why that cannot be stated as a fact."
                .to_string()
        },
        |path| {
            format!(
                "Read {} — Chrome wrote it, and chrome-agent cannot say whether it is the whole \
                 file, so check it against the source before using it.",
                path.display()
            )
        },
    );
    format!(
        "{what} Do not click again: the first click reached the page, and the page has no way to \
         tell a retry from a second deliberate action."
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::download_click::{Began, Transfer};

    /// A click request aimed at a selector, with the small numbers the settle tests read back.
    fn request<'a>(target: &'a Target<'a>) -> Request<'a> {
        Request {
            target,
            out: None,
            timeout_secs: 5,
            max_bytes: 64,
            on_intercept: crate::hit_test::OnIntercept::Dispatch,
            browser: "b1",
        }
    }

    fn lost(began: Option<Began>, kept: Option<PathBuf>) -> Outcome {
        let target = Target::Selector("#export");
        settle(
            Transfer::EvidenceLost {
                began,
                dropped: 300,
                kept,
                waited_ms: 1200,
            },
            None,
            "selector '#export'",
            &request(&target),
            &crate::hit_test::Dispatched::js(),
        )
    }

    /// The whole point of the outcome: a download whose evidence was dropped must not be
    /// reported with the sentence a download that never started gets. The old loop discarded
    /// `Lagged`, `began` stayed `None`, and a completed file was announced as "no download
    /// began … nothing was written" — one word before the cleanup deleted it.
    #[test]
    fn a_download_whose_events_were_dropped_is_never_reported_as_one_that_never_began() {
        let outcome = lost(None, Some(PathBuf::from("/tmp/kept-1-2")));

        assert!(
            !outcome.downloaded,
            "no `completed` was seen, so nothing may be claimed"
        );
        assert!(
            !outcome.message.contains("no download began"),
            "the claim the drops made unavailable: {}",
            outcome.message
        );
        assert!(
            outcome.message.contains("300 CDP event(s) were dropped"),
            "{}",
            outcome.message
        );
        assert!(
            outcome.message.contains("kept at /tmp/kept-1-2"),
            "{}",
            outcome.message
        );
        assert!(
            outcome.path.is_none(),
            "no file is claimed either: a success is not invented"
        );

        let json = outcome.to_json();
        assert_eq!(
            json["ok"], true,
            "the click was delivered, so this is not an error"
        );
        assert_eq!(json["downloaded"], false, "{json}");
        assert_eq!(
            json["kept"], "/tmp/kept-1-2",
            "the bytes are addressable: {json}"
        );
        assert_eq!(json["observed_after_ms"], 1200, "{json}");

        let hint = outcome.hint.expect("a hint");
        assert!(
            hint.contains("/tmp/kept-1-2"),
            "the hint names the one thing to read: {hint}"
        );
        assert!(
            hint.contains("Do not click again"),
            "rule 3, in words: {hint}"
        );
    }

    /// With the directory empty, the honest reading is still "unproven", not "nothing began":
    /// the events that would have said so are the ones that were lost.
    #[test]
    fn an_empty_transfer_directory_after_a_drop_is_unproven_and_not_a_denial() {
        let outcome = lost(None, None);
        assert!(
            !outcome.message.contains("no download began"),
            "{}",
            outcome.message
        );
        assert!(
            outcome.message.contains("cannot be told"),
            "{}",
            outcome.message
        );
        assert!(outcome.kept.is_none());
        assert!(
            outcome.to_json().get("kept").is_none(),
            "no path is invented"
        );
        assert!(
            outcome
                .hint
                .expect("a hint")
                .contains("cannot be stated as a fact"),
            "the absence is described as unproven, not asserted"
        );
    }

    /// What was known before the drops still travels: the file Chrome named and where it came
    /// from are facts, and only the ending is missing.
    #[test]
    fn a_drop_after_the_download_began_keeps_what_was_already_known() {
        let began = Began {
            guid: "g-1".into(),
            suggested_filename: "report.csv".into(),
            url: "blob:null/x".into(),
        };
        let outcome = lost(Some(began), None);
        assert_eq!(outcome.suggested_filename.as_deref(), Some("report.csv"));
        assert_eq!(outcome.source_url.as_deref(), Some("blob:null/x"));
        assert!(
            outcome.message.contains("report.csv"),
            "{}",
            outcome.message
        );
        assert!(
            !outcome.message.contains("incomplete"),
            "a lost `completed` is not proof of an unfinished transfer: {}",
            outcome.message
        );
    }

    /// The control: with nothing dropped, the four outcomes still say what they said.
    #[test]
    fn a_wait_that_dropped_nothing_still_answers_plainly() {
        let target = Target::Selector("#inert");
        let outcome = settle(
            Transfer::NeverBegan { waited_ms: 2000 },
            None,
            "selector '#inert'",
            &request(&target),
            &crate::hit_test::Dispatched::js(),
        );
        assert!(!outcome.downloaded);
        assert!(
            outcome.message.contains("no download began"),
            "{}",
            outcome.message
        );
        assert!(outcome.kept.is_none());
    }
}
