use std::collections::HashMap;

/// A comparison of two snapshots: the rendered text and the counts behind it.
///
/// The counts are produced by the comparison itself. Deriving them from the rendered
/// text instead would mean counting `+ ` / `- ` / `~ ` line prefixes, and accessibility
/// names go into the snapshot unescaped, so a name containing a newline splits a node
/// across two lines and the second half reads as its own record.
pub struct Diff {
    pub text: String,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    pub unchanged: usize,
    /// Which node lost focus, and which gained it. Either can be absent: a page with
    /// nothing focused yet has no `from`, and blurring without refocusing has no `to`.
    /// Kept out of `changed`, because focus rewrites two nodes on every click and says
    /// nothing about content, so counting it drowns the real signal.
    pub focus_from: Option<String>,
    pub focus_to: Option<String>,
    /// Nodes carrying a sequential `e{n}` uid. Those are renumbered on every snapshot, so
    /// they are never matched between two of them, only counted.
    pub anonymous: usize,
    /// Nodes present on both sides whose position in the document changed. Without this a
    /// drag-and-drop reorder reads as "No changes detected": every uid and every line is
    /// still there.
    pub moved: usize,
    /// Fields that held a value before the action and hold none after it.
    ///
    /// The evidence was always in the rendered text — the `value=` token simply stops
    /// appearing after the `->`. A diff line is prose, though: an agent reading the JSON saw
    /// `verdict:"changed"` and `ok:true` and never learnt that the field it had just filled
    /// was empty again. `tests/fixtures/form_value_reset_on_submit.html` is the archetype: the
    /// submit handler sets a status AND calls `form.reset()`, so both statements on the
    /// response were true and the loss was contractually invisible.
    pub values_lost: Vec<LostValue>,
}

/// A field that held a value before an action and holds none after it.
///
/// `was` is what the accessibility tree reported, which is not always what the field held: a
/// `type=password` value arrives here already masked by Chrome. Redaction is applied by the
/// caller, which can ask the page what kind of field it is — see `pipe_report::values_lost`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LostValue {
    pub uid: String,
    pub role: String,
    /// The accessible name, when the node has one.
    pub name: Option<String>,
    pub was: String,
}

/// Compare two snapshot texts and produce a compact diff.
///
/// Lines are matched by uid prefix. Output format:
/// ```text
/// + uid=n200 heading "Success"
/// - uid=n63 button "Submit"
/// ~ uid=n52 textbox value="" -> value="confirmed"
/// = 15 unchanged elements
/// ```
pub fn diff_snapshots(old: &str, new: &str) -> Diff {
    // Walk both snapshots in document order. Looking uids up in a map is fine, but
    // *iterating* one would order the output by hash, so the same page state would
    // produce a different diff on every run: no golden test, and no prompt-cache hit
    // for an agent that sees the same page twice.
    let old_lines = uid_lines(old);
    let new_lines = uid_lines(new);
    let anonymous = new_lines.iter().filter(|(uid, _)| is_anonymous(uid)).count();
    // `e{n}` uids are positional, not identities. Pairing them would compare unrelated nodes.
    let old_lines: Vec<_> = old_lines.into_iter().filter(|(uid, _)| !is_anonymous(uid)).collect();
    let new_lines: Vec<_> = new_lines.into_iter().filter(|(uid, _)| !is_anonymous(uid)).collect();
    let old_by_uid: HashMap<&str, &str> = old_lines.iter().copied().collect();
    let new_by_uid: HashMap<&str, &str> = new_lines.iter().copied().collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged: usize = 0;
    let mut focus_from: Option<String> = None;
    let mut focus_to: Option<String> = None;
    let mut values_lost = Vec::new();

    // Removed and changed, in the order they appeared on the old page.
    for (uid, old_line) in &old_lines {
        match new_by_uid.get(uid) {
            None => removed.push(format!("- {old_line}")),
            Some(new_line) => {
                if old_line == new_line {
                    unchanged += 1;
                } else if let Some(gained) = focus_only_change(old_line, new_line) {
                    if gained {
                        focus_to = Some((*uid).to_string());
                    } else {
                        focus_from = Some((*uid).to_string());
                    }
                    unchanged += 1;
                } else {
                    if let Some(lost) = lost_value(uid, old_line, new_line) {
                        values_lost.push(lost);
                    }
                    changed.push(render_change(old_line, new_line));
                }
            }
        }
    }

    // Added, in the order they appear on the new page.
    for (uid, new_line) in &new_lines {
        if !old_by_uid.contains_key(uid) {
            added.push(format!("+ {new_line}"));
        }
    }

    let moved = count_moved(&old_lines, &new_lines);

    // A focus move is not a content change, but it is something we observed, so the
    // summary must not go on to say nothing happened.
    let observed_something = !added.is_empty()
        || !removed.is_empty()
        || !changed.is_empty()
        || moved > 0
        || focus_from.is_some()
        || focus_to.is_some();
    let mut out = String::new();
    for line in added.iter().chain(&removed).chain(&changed) {
        out.push_str(line);
        out.push('\n');
    }
    if moved > 0 {
        out.push_str(&format!("> {moved} elements moved\n"));
    }
    if focus_from.is_some() || focus_to.is_some() {
        let f = focus_from.as_deref().unwrap_or("none");
        let t = focus_to.as_deref().unwrap_or("none");
        out.push_str(&format!("focus: {f} -> {t}\n"));
    }
    if anonymous > 0 {
        out.push_str(&format!("? {anonymous} nodes without stable ids (not compared)\n"));
    }
    if unchanged > 0 {
        out.push_str(&format!("= {unchanged} unchanged elements\n"));
    }
    if !observed_something {
        out.push_str("No changes detected.\n");
    }
    Diff {
        text: out,
        added: added.len(),
        removed: removed.len(),
        changed: changed.len(),
        unchanged,
        focus_from,
        focus_to,
        anonymous,
        moved,
        values_lost,
    }
}

/// The value a node held before the action and no longer holds, if that is what changed.
///
/// Only a value that went from something to nothing counts. A value that was REPLACED is not a
/// loss — that is a mask, a normaliser or a fresh write, and the `~` line already carries both
/// sides. A node that disappeared entirely is not one either: it is reported as `removed`,
/// which is not silent, and the value went with a node the caller can no longer act on.
fn lost_value(uid: &str, old_line: &str, new_line: &str) -> Option<LostValue> {
    let old_tokens = tokenize(old_line)?;
    let was = value_token(&old_tokens)?;
    if was.is_empty() {
        return None;
    }
    let new_tokens = tokenize(new_line)?;
    if !value_token(&new_tokens).is_none_or(str::is_empty) {
        return None;
    }
    // `uid=n11 textbox "Email" …`: role is the token after the uid, the name the quoted one
    // after it. Both are best effort — a node with neither still reports the loss.
    Some(LostValue {
        uid: uid.to_string(),
        role: old_tokens.get(1).copied().unwrap_or_default().to_string(),
        name: old_tokens
            .get(2)
            .and_then(|t| t.strip_prefix('"'))
            .and_then(|t| t.strip_suffix('"'))
            .map(str::to_string),
        was: was.to_string(),
    })
}

/// The inside of a `value="…"` token, if the line has one.
fn value_token<'a>(tokens: &[&'a str]) -> Option<&'a str> {
    tokens
        .iter()
        .find_map(|t| t.strip_prefix("value=\""))
        .and_then(|t| t.strip_suffix('"'))
}

/// `e{n}` uids come from nodes with no backendDOMNodeId and are numbered per snapshot.
fn is_anonymous(uid: &str) -> bool {
    uid.starts_with('e') && uid[1..].chars().all(|c| c.is_ascii_digit()) && uid.len() > 1
}

/// `Some(true)` when the node gained focus, `Some(false)` when it lost it, `None` when
/// anything else about the node also changed.
fn focus_only_change(old_line: &str, new_line: &str) -> Option<bool> {
    let (old_tokens, new_tokens) = (tokenize(old_line)?, tokenize(new_line)?);
    let had = old_tokens.contains(&"focused");
    let has = new_tokens.contains(&"focused");
    if had == has {
        return None;
    }
    let strip = |t: &Vec<&str>| -> Vec<String> {
        t.iter().filter(|x| **x != "focused").map(|x| (*x).to_string()).collect()
    };
    if strip(&old_tokens) == strip(&new_tokens) { Some(has) } else { None }
}

/// How many nodes present on both sides sit at a different position in the document.
fn count_moved(old_lines: &[(&str, &str)], new_lines: &[(&str, &str)]) -> usize {
    let new_set: std::collections::HashSet<&str> = new_lines.iter().map(|(u, _)| *u).collect();
    let old_order: Vec<&str> = old_lines.iter().map(|(u, _)| *u).filter(|u| new_set.contains(u)).collect();
    let old_set: std::collections::HashSet<&str> = old_lines.iter().map(|(u, _)| *u).collect();
    let new_order: Vec<&str> = new_lines.iter().map(|(u, _)| *u).filter(|u| old_set.contains(u)).collect();
    old_order.iter().zip(&new_order).filter(|(a, b)| a != b).count()
}

/// Whether the live page is the same document the stored snapshot came from.
///
/// Tri-state on purpose. The URL comparison this replaces had no way to say "I don't know",
/// so an unreadable signal took the confident branch and diffed two unrelated uid spaces.
/// A URL is also the wrong signal twice over: it changes on a fragment jump and on
/// `history.pushState` where the document and every uid survive, and it stays put across a
/// reload or a form GET back to the same address where nothing survives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Identity {
    /// Same frame, same loader: uids from the stored snapshot still refer to the same nodes.
    Same,
    /// The document was replaced. Every stored uid is dead.
    Different,
    /// We could not read it. Treated as "cannot diff", never as "same".
    Unknown,
}

impl Identity {
    /// Build from the stored and live `(frame_id, loader_id)` pairs.
    pub fn from_loader(stored: Option<(&str, &str)>, live: Option<(&str, &str)>) -> Self {
        match (stored, live) {
            (Some(a), Some(b)) if a == b => Self::Same,
            (Some(_), Some(_)) => Self::Different,
            _ => Self::Unknown,
        }
    }
}

/// Outcome of comparing the stored snapshot against the live page.
#[derive(Debug)]
pub struct Comparison {
    /// Diff text, or the fresh snapshot when the document changed under us.
    pub text: String,
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub moved: usize,
    pub anonymous: usize,
    pub focus_from: Option<String>,
    pub focus_to: Option<String>,
    /// True when the live page is a different document from the stored snapshot.
    pub document_changed: bool,
    /// False when we could not establish which document we are looking at.
    pub identity_known: bool,
    /// What the agent should do next, when that isn't obvious.
    pub hint: Option<&'static str>,
    /// Fields this action emptied. Only ever populated on a real diff: across two documents
    /// there is no "before" to have lost anything from.
    pub values_lost: Vec<LostValue>,
}

/// Compare a stored snapshot against a fresh one, refusing to diff unless we know the
/// document is the same one.
///
/// uids are Chrome `backendNodeId`s, and those counters overlap between documents. Diffing
/// two different pages therefore pairs unrelated nodes that happen to share a uid and
/// reports them as "changed" — on a real navigation that produced hundreds of bogus lines
/// and cost more tokens than simply re-reading the destination page. So when the document
/// changed, return the new snapshot and say so instead of pretending to diff.
///
pub fn compare(identity: Identity, old_text: &str, new_text: &str) -> Comparison {
    if identity != Identity::Same {
        let hint = if identity == Identity::Different {
            "The page navigated, so uids from the previous snapshot no longer refer to anything. This is the new page; act on these uids."
        } else {
            "Could not read which document this is, so the previous snapshot cannot be compared against it. This is the page as it stands; act on these uids."
        };
        return Comparison {
            text: new_text.to_string(),
            added: 0,
            removed: 0,
            changed: 0,
            unchanged: 0,
            moved: 0,
            anonymous: 0,
            focus_from: None,
            focus_to: None,
            document_changed: identity == Identity::Different,
            identity_known: identity != Identity::Unknown,
            hint: Some(hint),
            values_lost: Vec::new(),
        };
    }

    let diff = diff_snapshots(old_text, new_text);
    Comparison {
        text: diff.text,
        added: diff.added,
        removed: diff.removed,
        changed: diff.changed,
        unchanged: diff.unchanged,
        moved: diff.moved,
        anonymous: diff.anonymous,
        focus_from: diff.focus_from,
        focus_to: diff.focus_to,
        document_changed: false,
        identity_known: true,
        hint: None,
        values_lost: diff.values_lost,
    }
}

/// Render a changed node, writing the part that stayed the same only once.
///
/// A node that only gained a value goes from repeating ~60 characters twice to stating the
/// attribute that moved, and changed lines are the bulk of a form-filling flow. When the
/// two lines share no identity (different role, or a name we can't tokenize) the whole line
/// is the honest rendering, because there is nothing meaningful to hoist.
fn render_change(old_line: &str, new_line: &str) -> String {
    let whole = || format!("~ {old_line} -> {new_line}");
    let (Some(old_tokens), Some(new_tokens)) = (tokenize(old_line), tokenize(new_line)) else {
        return whole();
    };
    let shared = old_tokens
        .iter()
        .zip(&new_tokens)
        .take_while(|(a, b)| a == b)
        .count();
    // Fewer than uid + role in common means the node changed identity, not just state.
    if shared < 2 || shared == old_tokens.len() && shared == new_tokens.len() {
        return whole();
    }
    let prefix = old_tokens[..shared].join(" ");
    let old_rest = old_tokens[shared..].join(" ");
    let new_rest = new_tokens[shared..].join(" ");
    // One side can be empty when the shared prefix covers the whole shorter line; writing
    // it as an empty field would leave a stray double space.
    match (old_rest.is_empty(), new_rest.is_empty()) {
        (true, _) => format!("~ {prefix} -> {new_rest}"),
        (_, true) => format!("~ {prefix} {old_rest} ->"),
        _ => format!("~ {prefix} {old_rest} -> {new_rest}"),
    }
}

/// Split a snapshot line into space-separated tokens, keeping quoted runs whole.
///
/// Returns `None` when the quotes don't balance. Accessibility names are written into
/// snapshots unescaped, so a name containing a quote makes the token boundaries ambiguous,
/// and guessing at them would mangle the output. Callers fall back to the whole line.
fn tokenize(line: &str) -> Option<Vec<&str>> {
    let mut tokens = Vec::new();
    let mut in_quotes = false;
    let mut start = 0usize;
    for (i, ch) in line.char_indices() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ' ' if !in_quotes => {
                if i > start {
                    tokens.push(&line[start..i]);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if in_quotes {
        return None;
    }
    if start < line.len() {
        tokens.push(&line[start..]);
    }
    Some(tokens)
}

/// Extract uid -> trimmed line from snapshot text.
/// `(uid, line)` pairs in the order they appear in the snapshot.
fn uid_lines(text: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("uid=") {
            // uid is the token before the first space
            let uid = rest.find(' ').map_or(rest, |i| &rest[..i]);
            out.push((uid, trimmed));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moving focus rewrites two nodes and means nothing about content. Left in the counts
    /// it is the single loudest source of noise: every click on a page reports "2 changed".
    #[test]
    fn moving_focus_is_reported_separately_from_content() {
        let old = "uid=n1 link \"A\" focused\nuid=n2 button \"B\"\n";
        let new = "uid=n1 link \"A\"\nuid=n2 button \"B\" focused\n";
        let d = diff_snapshots(old, new);
        assert_eq!(d.changed, 0, "focus moving is not a content change: {}", d.text);
        assert_eq!(d.focus_from.as_deref(), Some("n1"), "{}", d.text);
        assert_eq!(d.focus_to.as_deref(), Some("n2"), "{}", d.text);
    }

    /// A node with no backendDOMNodeId falls back to a sequential `e{n}` uid, renumbered on
    /// every snapshot. Matching `e1` to `e1` pairs two unrelated nodes, which is the same
    /// defect as matching uids across documents, one scope down.
    #[test]
    fn sequential_uids_are_never_matched_between_snapshots() {
        let old = "uid=n1 heading \"Same\"\nuid=e1 generic \"first pass\"\n";
        let new = "uid=n1 heading \"Same\"\nuid=e1 generic \"totally different node\"\n";
        let d = diff_snapshots(old, new);
        assert_eq!(d.changed, 0, "e-uids carry no identity, so nothing can be said to have changed: {}", d.text);
        assert_eq!(d.anonymous, 1, "but their presence is worth reporting: {}", d.text);
    }

    /// Reordering keeps every uid and every line, so pairing by uid alone reports a
    /// drag-and-drop as "No changes detected".
    #[test]
    fn a_reorder_does_not_read_as_no_change() {
        let old = "uid=n1 listitem \"A\"\nuid=n2 listitem \"B\"\nuid=n3 listitem \"C\"\n";
        let new = "uid=n3 listitem \"C\"\nuid=n1 listitem \"A\"\nuid=n2 listitem \"B\"\n";
        let d = diff_snapshots(old, new);
        assert!(d.moved > 0, "a reorder must not be invisible: {}", d.text);
        assert!(!d.text.contains("No changes"), "{}", d.text);
    }

    /// The renderer hoists the shared prefix; when one side has nothing left it must not
    /// leave a double space behind.
    #[test]
    fn a_changed_line_has_no_empty_side() {
        let old = "uid=n1 link \"A\"\n";
        let new = "uid=n1 link \"A\" focusable\n";
        let d = diff_snapshots(old, new);
        assert!(!d.text.contains("  ->"), "empty left side leaves a double space: {:?}", d.text);
    }

    /// A changed line repeats the node twice today. Only the part that moved matters, and
    /// changed lines dominate a form-filling flow, so the shared prefix is written once.
    #[test]
    fn a_changed_line_states_only_what_moved() {
        let old = "uid=n11 textbox \"Email\" focusable value=\"\"\n";
        let new = "uid=n11 textbox \"Email\" focusable value=\"a@b.c\"\n";
        let d = diff_snapshots(old, new);
        assert_eq!(
            d.text.lines().next().unwrap(),
            "~ uid=n11 textbox \"Email\" focusable value=\"\" -> value=\"a@b.c\"",
            "shared tokens appear once"
        );
    }

    /// Accessibility names are written unescaped, so a name can carry a stray quote and
    /// make the token split ambiguous. When that happens, fall back to the whole line
    /// rather than guess at token boundaries.
    #[test]
    fn an_unbalanced_quote_falls_back_to_the_whole_line() {
        let old = "uid=n7 link \"\"WCAG 2.1\" ref\n";
        let new = "uid=n7 link \"\"WCAG 2.2\" ref\n";
        let d = diff_snapshots(old, new);
        let line = d.text.lines().next().unwrap();
        assert!(
            line.contains("uid=n7 link \"\"WCAG 2.1\" ref -> uid=n7 link \"\"WCAG 2.2\" ref"),
            "expected whole-line form, got {line}"
        );
    }

    /// When every token differs there is no shared prefix to hoist, so the whole line is
    /// the honest rendering.
    #[test]
    fn a_wholly_different_node_keeps_the_whole_line() {
        let old = "uid=n3 button \"Save\"\n";
        let new = "uid=n3 link \"Cancel\"\n";
        let d = diff_snapshots(old, new);
        let line = d.text.lines().next().unwrap();
        assert_eq!(line, "~ uid=n3 button \"Save\" -> uid=n3 link \"Cancel\"");
    }

    /// The S3 shape: a submit handler sets a status and calls `form.reset()`. The `~` line
    /// carried the evidence all along — the `value=` token simply stops appearing after the
    /// arrow — but a diff line is prose, and an agent reading JSON never saw it.
    #[test]
    fn a_field_this_action_emptied_is_reported_as_a_lost_value() {
        let old = "uid=n2 textbox \"Email\" value=\"hello@example.com\" focused\nuid=n5 status \"\"\n";
        let new = "uid=n2 textbox \"Email\" focused\nuid=n5 status \"sent\"\n";
        let d = diff_snapshots(old, new);
        assert_eq!(d.values_lost.len(), 1, "{}", d.text);
        let lost = &d.values_lost[0];
        assert_eq!(lost.uid, "n2");
        assert_eq!(lost.role, "textbox");
        assert_eq!(lost.name.as_deref(), Some("Email"));
        assert_eq!(lost.was, "hello@example.com");
    }

    /// `value=""` is the same absence written differently, and every empty text input in a
    /// snapshot has it. Treating it as a loss would fire on every form on the web.
    #[test]
    fn an_emptied_value_token_counts_the_same_as_a_missing_one() {
        let old = "uid=n2 textbox \"Email\" value=\"a@b.c\"\n";
        let new = "uid=n2 textbox \"Email\" value=\"\"\n";
        assert_eq!(diff_snapshots(old, new).values_lost.len(), 1);
        // And the reverse is not a loss: nothing was there to lose.
        let d = diff_snapshots(new, old);
        assert!(d.values_lost.is_empty(), "a field being filled is not a field being emptied");
    }

    /// A value that was REPLACED is not a value that was lost. A mask, a normaliser and a
    /// fresh write all land here, and the `~` line already carries both sides.
    #[test]
    fn a_rewritten_value_is_not_a_lost_one() {
        let old = "uid=n2 textbox \"Phone\" value=\"5551234567\"\n";
        let new = "uid=n2 textbox \"Phone\" value=\"(555) 123-4567\"\n";
        assert!(diff_snapshots(old, new).values_lost.is_empty());
    }

    /// A node that disappeared entirely is reported as `removed`, which is not silent, and its
    /// value went with a node the caller can no longer act on.
    #[test]
    fn a_node_that_vanished_is_a_removal_not_a_lost_value() {
        let old = "uid=n2 textbox \"Email\" value=\"a@b.c\"\n";
        let new = "uid=n9 heading \"Thanks\"\n";
        let d = diff_snapshots(old, new);
        assert_eq!(d.removed, 1, "{}", d.text);
        assert!(d.values_lost.is_empty(), "{}", d.text);
    }

    /// Across two documents there is no "before" to have lost anything from: the uids belong
    /// to different spaces, so a pairing would be an accident.
    #[test]
    fn no_value_is_claimed_lost_across_a_document_change() {
        let old = "uid=n2 textbox \"Email\" value=\"a@b.c\"\n";
        let new = "uid=n2 heading \"Other page\"\n";
        for identity in [Identity::Different, Identity::Unknown] {
            assert!(compare(identity, old, new).values_lost.is_empty(), "for {identity:?}");
        }
    }

    /// A value containing spaces survives tokenizing; a name with an unbalanced quote makes
    /// the split ambiguous and must yield no claim rather than a mangled one.
    #[test]
    fn lost_values_handle_spaces_and_refuse_ambiguous_lines() {
        let old = "uid=n2 textbox \"Address\" value=\"12 Rue de la Paix\"\n";
        let new = "uid=n2 textbox \"Address\"\n";
        assert_eq!(diff_snapshots(old, new).values_lost[0].was, "12 Rue de la Paix");

        let old = "uid=n7 textbox \"\"odd\" value=\"x\"\n";
        let new = "uid=n7 textbox \"\"odd\"\n";
        assert!(diff_snapshots(old, new).values_lost.is_empty(), "no guess at token boundaries");
    }

    /// An identity we could not read must not be reported as "same document". uids are only
    /// comparable within one document, so guessing wrong here fabricates a diff between two
    /// unrelated pages — the exact failure the URL check was added to prevent.
    #[test]
    fn an_unreadable_identity_does_not_claim_the_document_is_the_same() {
        let old = "uid=n1 heading \"Old\"\n";
        let new = "uid=n1 heading \"New\"\n";
        let c = compare(Identity::Unknown, old, new);
        assert!(!c.identity_known, "we could not tell: {c:?}");
        assert_eq!(c.changed, 0, "so nothing may be reported as changed: {}", c.text);
        assert_eq!(c.text, new, "the caller gets the page instead of a guess");
        assert!(c.hint.is_some(), "and is told why");
    }

    /// The ordinary path is unaffected.
    #[test]
    fn a_same_document_identity_still_diffs() {
        let old = "uid=n1 heading \"Old\"\n";
        let new = "uid=n1 heading \"New\"\n";
        let c = compare(Identity::Same, old, new);
        assert!(c.identity_known);
        assert!(!c.document_changed);
        assert_eq!(c.changed, 1, "{}", c.text);
    }

    /// Same URL, different document: a reload, or a form GET that lands back on itself.
    /// The URL comparison called these "same" and diffed two unrelated uid spaces.
    #[test]
    fn a_reload_to_the_same_url_is_a_different_document() {
        let old = "uid=n1 heading \"Before\"\n";
        let new = "uid=n1 heading \"After\"\n";
        let c = compare(Identity::Different, old, new);
        assert!(c.document_changed);
        assert_eq!((c.added, c.removed, c.changed), (0, 0, 0));
    }

    /// When the document changed, `text` carries a whole snapshot rather than a diff.
    /// Accessibility names go in unescaped (snapshot.rs writes `name` raw), so a name
    /// containing a newline puts a line starting with "- " into that payload. Counts are
    /// reported by the comparison itself, so such a line cannot be read back as a removal.
    #[test]
    fn a_changed_document_reports_no_edits_whatever_the_snapshot_contains() {
        let old = "uid=n1 heading \"Old page\"\n";
        let new = "uid=n1 heading \"Save\n- and exit\"\nuid=n2 button \"Go\"\n";
        let c = compare(Identity::Different, old, new);
        assert!(c.document_changed);
        assert_eq!((c.added, c.removed, c.changed), (0, 0, 0), "no edit can be claimed across documents");
        assert_eq!(c.text, new, "the caller gets the destination page");
    }

    /// Counts come from the comparison, not from re-reading the rendered text.
    #[test]
    fn counts_match_the_rendered_lines() {
        let old = "uid=n1 heading \"A\"\nuid=n2 button \"B\"\n";
        let new = "uid=n1 heading \"A changed\"\nuid=n3 link \"C\"\n";
        let d = diff_snapshots(old, new);
        assert_eq!((d.added, d.removed, d.changed, d.unchanged), (1, 1, 1, 0));
        assert_eq!(d.text.lines().filter(|l| l.starts_with("+ ")).count(), d.added);
        assert_eq!(d.text.lines().filter(|l| l.starts_with("- ")).count(), d.removed);
        assert_eq!(d.text.lines().filter(|l| l.starts_with("~ ")).count(), d.changed);
    }

    /// Output order follows the page, not the hash of a uid. Without this the same page
    /// state yields a different diff on every process, which defeats prompt caching and
    /// makes the output impossible to assert on.
    #[test]
    fn lines_follow_document_order() {
        let old = "uid=n1 heading \"A\"\nuid=n2 button \"B\"\nuid=n3 link \"C\"\nuid=n4 link \"D\"\n";
        let new = "uid=n1 heading \"A\"\nuid=n3 link \"C changed\"\nuid=n5 link \"E\"\nuid=n6 link \"F\"\n";
        let result = diff_snapshots(old, new);
        let lines: Vec<&str> = result.text.lines().filter(|l| !l.starts_with('=')).collect();
        assert_eq!(
            lines,
            vec![
                "+ uid=n5 link \"E\"",
                "+ uid=n6 link \"F\"",
                "- uid=n2 button \"B\"",
                "- uid=n4 link \"D\"",
                "~ uid=n3 link \"C\" -> \"C changed\"",
            ],
            "added/removed/changed must each follow document order"
        );
    }

    #[test]
    fn no_changes() {
        let snap = "uid=n1 heading \"Hello\"\nuid=n2 button \"OK\"\n";
        let result = diff_snapshots(snap, snap);
        assert!(result.text.contains("No changes"));
    }

    #[test]
    fn added_element() {
        let old = "uid=n1 heading \"Hello\"\n";
        let new = "uid=n1 heading \"Hello\"\nuid=n2 button \"OK\"\n";
        let result = diff_snapshots(old, new);
        assert!(result.text.contains("+ uid=n2 button \"OK\""));
        assert!(result.text.contains("= 1 unchanged"));
                assert_eq!(result.added, 1);
        assert_eq!(result.removed, 0);
        assert_eq!(result.changed, 0);
    }

    #[test]
    fn removed_element() {
        let old = "uid=n1 heading \"Hello\"\nuid=n2 button \"OK\"\n";
        let new = "uid=n1 heading \"Hello\"\n";
        let result = diff_snapshots(old, new);
        assert!(result.text.contains("- uid=n2 button \"OK\""));
                assert_eq!(result.removed, 1);
    }

    #[test]
    fn changed_element() {
        let old = "uid=n1 textbox value=\"\"\n";
        let new = "uid=n1 textbox value=\"hello\"\n";
        let result = diff_snapshots(old, new);
        assert!(result.text.contains("~ uid=n1 textbox"));
                assert_eq!(result.changed, 1);
    }

    #[test]
    fn mixed_changes() {
        let old = "uid=n1 heading \"Title\"\nuid=n2 button \"Submit\"\nuid=n3 textbox value=\"\"\n";
        let new = "uid=n1 heading \"Title\"\nuid=n3 textbox value=\"done\"\nuid=n4 heading \"Success\"\n";
        let result = diff_snapshots(old, new);
        assert!(result.text.contains("+ uid=n4"));
        assert!(result.text.contains("- uid=n2"));
        assert!(result.text.contains("~ uid=n3"));
        assert!(result.text.contains("= 1 unchanged"));
    }

    #[test]
    fn indented_lines() {
        let old = "  uid=n1 heading \"Hello\"\n    uid=n2 button \"OK\"\n";
        let new = "  uid=n1 heading \"Hello\"\n    uid=n3 link \"New\"\n";
        let result = diff_snapshots(old, new);
        assert!(result.text.contains("+ uid=n3"));
        assert!(result.text.contains("- uid=n2"));
    }
}
