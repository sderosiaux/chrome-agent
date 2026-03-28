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
    // Build maps: uid -> full line (trimmed)
    let old_by_uid = uid_line_map(old);
    let new_by_uid = uid_line_map(new);

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();
    let mut unchanged: usize = 0;

    // Detect removed and changed
    for (&uid, old_line) in &old_by_uid {
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

    // Detect added
    for (&uid, new_line) in &new_by_uid {
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
fn uid_line_map(text: &str) -> HashMap<&str, &str> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("uid=") {
            // uid is the token before the first space
            if let Some(space_idx) = rest.find(' ') {
                let uid = &rest[..space_idx];
                map.insert(uid, trimmed);
            } else {
                // Line is just "uid=xxx" with no attributes
                map.insert(rest, trimmed);
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

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
