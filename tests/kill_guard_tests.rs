//! One place decides whether a pid may be signalled.
//!
//! `kill::kill_pid` asks two questions before it signals — is the pid alive, and is what holds
//! it a browser — because a stored pid may have been recycled (the incident in `src/kill.rs`:
//! pid 80548, by then `git fsmonitor--daemon`). A call site that spawns `kill` itself answers
//! neither, and `pipe.rs` was such a site: the headless/headed mismatch arm ran
//! `Command::new("kill")` on the stored pid directly.
//!
//! Nothing in the type system stops the fourth one being written, so it is pinned here. A
//! source scan rather than a behaviour test: the defect is a bypass, and a bypass is visible
//! only in the shape of the code.

/// Every `.rs` file under `dir`, at any depth, as `(path relative to the crate root, contents)`.
fn collect_rs(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let name =
                path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            out.push((name, std::fs::read_to_string(&path).unwrap_or_default()));
        }
    }
}

/// The one module allowed to spawn the killer, being the one that decides whether to.
const GUARD: &str = "src/kill.rs";

#[test]
fn only_the_guard_signals_a_pid() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut sources = Vec::new();
    collect_rs(&root, &root.join("src"), &mut sources);
    assert!(
        sources.len() > 40,
        "the scan found {} sources under src/, so the walk is broken and proves nothing",
        sources.len()
    );

    // Written split so this file is not its own offender.
    let spawn = format!("Command::new({})", "\"kill\"");
    let offenders: Vec<&str> = sources
        .iter()
        .filter(|(name, text)| name != GUARD && text.contains(&spawn))
        .map(|(name, _)| name.as_str())
        .collect();

    assert!(
        offenders.is_empty(),
        "{offenders:?} signal a pid without asking whether it is still a browser. \
         Route the kill through `kill::kill_pid` (or `browser::kill_and_await_exit`, which \
         also waits for the exit before a relaunch reads the profile again)."
    );

    let guard = sources
        .iter()
        .find(|(name, _)| name == GUARD)
        .map(|(_, text)| text.clone())
        .unwrap_or_default();
    assert!(
        guard.contains(&spawn),
        "{GUARD} no longer signals anything: this scan would then pass over a codebase that \
         kills through some other route entirely"
    );
}
