use std::collections::HashMap;

/// Compare two snapshot texts and produce a compact diff.
///
/// Lines are matched by uid prefix. Output format:
/// ```text
/// + uid=n200 heading "Success"
/// - uid=n63 button "Submit"
/// ~ uid=n52 textbox value="" -> value="confirmed"
/// = 15 unchanged elements
/// ```
pub fn diff_snapshots(old: &str, new: &str) -> String {
    // Walk both snapshots in document order. Looking uids up in a map is fine, but
    // *iterating* one would order the output by hash, so the same page state would
    // produce a different diff on every run: no golden test, and no prompt-cache hit
    // for an agent that sees the same page twice.
    let old_lines = uid_lines(old);
    let new_lines = uid_lines(new);
    let old_by_uid: HashMap<&str, &str> = old_lines.iter().copied().collect();
    let new_by_uid: HashMap<&str, &str> = new_lines.iter().copied().collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged: usize = 0;

    // Removed and changed, in the order they appeared on the old page.
    for (uid, old_line) in &old_lines {
        match new_by_uid.get(uid) {
            None => removed.push(format!("- {old_line}")),
            Some(new_line) => {
                if old_line == new_line {
                    unchanged += 1;
                } else {
                    changed.push(format!("~ {old_line} -> {new_line}"));
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

    let has_changes = !added.is_empty() || !removed.is_empty() || !changed.is_empty();
    let mut out = String::new();
    for line in &added {
        out.push_str(line);
        out.push('\n');
    }
    for line in &removed {
        out.push_str(line);
        out.push('\n');
    }
    for line in &changed {
        out.push_str(line);
        out.push('\n');
    }
    if unchanged > 0 {
        out.push_str(&format!("= {unchanged} unchanged elements\n"));
    }
    if !has_changes {
        out.push_str("No changes detected.\n");
    }
    out
}

/// Outcome of comparing the stored snapshot against the live page.
pub struct Comparison {
    /// Diff text, or the fresh snapshot when the document changed under us.
    pub text: String,
    pub stats: DiffStats,
    /// True when the live page is a different document from the stored snapshot.
    pub document_changed: bool,
    /// What the agent should do next, when that isn't obvious.
    pub hint: Option<&'static str>,
}

/// Compare a stored snapshot against a fresh one, refusing to diff across documents.
///
/// uids are Chrome `backendNodeId`s, and those counters overlap between documents. Diffing
/// two different pages therefore pairs unrelated nodes that happen to share a uid and
/// reports them as "changed" — on a real navigation that produced hundreds of bogus lines
/// and cost more tokens than simply re-reading the destination page. So when the document
/// changed, return the new snapshot and say so instead of pretending to diff.
///
/// An empty `old_url` means the snapshot predates URL tracking; we diff as before rather
/// than claim a change we cannot substantiate.
pub fn compare(old_url: Option<&str>, old_text: &str, new_url: &str, new_text: &str) -> Comparison {
    let changed_document = match old_url {
        Some(old) if !old.is_empty() && !new_url.is_empty() => old != new_url,
        _ => false,
    };

    if changed_document {
        return Comparison {
            text: new_text.to_string(),
            stats: DiffStats { added: 0, removed: 0, changed: 0 },
            document_changed: true,
            hint: Some("The page navigated, so uids from the previous snapshot no longer refer to anything. This is the new page; act on these uids."),
        };
    }

    let diff = diff_snapshots(old_text, new_text);
    let stats = diff_stats(&diff);
    Comparison { text: diff, stats, document_changed: false, hint: None }
}

/// Count of added, removed, changed elements.
pub struct DiffStats {
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
}

/// Parse diff output into counts.
pub fn diff_stats(diff: &str) -> DiffStats {
    let mut added = 0usize;
    let mut removed = 0usize;
    let mut changed = 0usize;
    for line in diff.lines() {
        if line.starts_with("+ ") {
            added += 1;
        } else if line.starts_with("- ") {
            removed += 1;
        } else if line.starts_with("~ ") {
            changed += 1;
        }
    }
    DiffStats { added, removed, changed }
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

    /// Output order follows the page, not the hash of a uid. Without this the same page
    /// state yields a different diff on every process, which defeats prompt caching and
    /// makes the output impossible to assert on.
    #[test]
    fn lines_follow_document_order() {
        let old = "uid=n1 heading \"A\"\nuid=n2 button \"B\"\nuid=n3 link \"C\"\nuid=n4 link \"D\"\n";
        let new = "uid=n1 heading \"A\"\nuid=n3 link \"C changed\"\nuid=n5 link \"E\"\nuid=n6 link \"F\"\n";
        let result = diff_snapshots(old, new);
        let lines: Vec<&str> = result.lines().filter(|l| !l.starts_with('=')).collect();
        assert_eq!(
            lines,
            vec![
                "+ uid=n5 link \"E\"",
                "+ uid=n6 link \"F\"",
                "- uid=n2 button \"B\"",
                "- uid=n4 link \"D\"",
                "~ uid=n3 link \"C\" -> uid=n3 link \"C changed\"",
            ],
            "added/removed/changed must each follow document order"
        );
    }

    #[test]
    fn no_changes() {
        let snap = "uid=n1 heading \"Hello\"\nuid=n2 button \"OK\"\n";
        let result = diff_snapshots(snap, snap);
        assert!(result.contains("No changes"));
    }

    #[test]
    fn added_element() {
        let old = "uid=n1 heading \"Hello\"\n";
        let new = "uid=n1 heading \"Hello\"\nuid=n2 button \"OK\"\n";
        let result = diff_snapshots(old, new);
        assert!(result.contains("+ uid=n2 button \"OK\""));
        assert!(result.contains("= 1 unchanged"));
        let stats = diff_stats(&result);
        assert_eq!(stats.added, 1);
        assert_eq!(stats.removed, 0);
        assert_eq!(stats.changed, 0);
    }

    #[test]
    fn removed_element() {
        let old = "uid=n1 heading \"Hello\"\nuid=n2 button \"OK\"\n";
        let new = "uid=n1 heading \"Hello\"\n";
        let result = diff_snapshots(old, new);
        assert!(result.contains("- uid=n2 button \"OK\""));
        let stats = diff_stats(&result);
        assert_eq!(stats.removed, 1);
    }

    #[test]
    fn changed_element() {
        let old = "uid=n1 textbox value=\"\"\n";
        let new = "uid=n1 textbox value=\"hello\"\n";
        let result = diff_snapshots(old, new);
        assert!(result.contains("~ uid=n1 textbox"));
        let stats = diff_stats(&result);
        assert_eq!(stats.changed, 1);
    }

    #[test]
    fn mixed_changes() {
        let old = "uid=n1 heading \"Title\"\nuid=n2 button \"Submit\"\nuid=n3 textbox value=\"\"\n";
        let new = "uid=n1 heading \"Title\"\nuid=n3 textbox value=\"done\"\nuid=n4 heading \"Success\"\n";
        let result = diff_snapshots(old, new);
        assert!(result.contains("+ uid=n4"));
        assert!(result.contains("- uid=n2"));
        assert!(result.contains("~ uid=n3"));
        assert!(result.contains("= 1 unchanged"));
    }

    #[test]
    fn indented_lines() {
        let old = "  uid=n1 heading \"Hello\"\n    uid=n2 button \"OK\"\n";
        let new = "  uid=n1 heading \"Hello\"\n    uid=n3 link \"New\"\n";
        let result = diff_snapshots(old, new);
        assert!(result.contains("+ uid=n3"));
        assert!(result.contains("- uid=n2"));
    }
}
