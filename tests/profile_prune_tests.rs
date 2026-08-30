//! Orphaned profile directories are removed by the save path under the three-condition
//! predicate. These drive the real binary against a temporary `HOME`, so what is under test
//! is the sweep riding on `save_session`, not the predicate itself (`src/profiles.rs`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;

mod common;

/// Save the session without a browser: `close` on a name never opened loads and saves.
fn run_in(home: &Path, args: &[&str]) -> (String, i32) {
    let out = Command::new(common::binary())
        .args(args)
        .env("HOME", home)
        .output()
        .expect("run chrome-agent");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

fn tmp_home(tag: &str) -> PathBuf {
    let home = common::temp_path(&format!("prune-{tag}"), "d");
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join(".chrome-agent").join("browsers")).unwrap();
    home
}

fn browsers(home: &Path) -> PathBuf {
    home.join(".chrome-agent").join("browsers")
}

fn profile(home: &Path, name: &str) -> PathBuf {
    let root = browsers(home).join(name);
    let dir = root.join("chromium-profile");
    std::fs::create_dir_all(dir.join("Default")).unwrap();
    std::fs::write(dir.join("Local State"), "{}").unwrap();
    std::fs::write(dir.join("Default").join("Cookies"), "x").unwrap();
    root
}

/// Backdate every mtime the sweep reads. Children before parents: writing bumps the parent.
fn age(root: &Path) {
    let profile = root.join("chromium-profile");
    let mut paths = vec![root.to_path_buf(), profile.clone()];
    for dir in [profile.clone(), profile.join("Default")] {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            paths.extend(entries.filter_map(|e| e.ok().map(|e| e.path())));
        }
    }
    paths.reverse();
    for path in paths {
        // Two days back, past the one-day grace window.
        let status = Command::new("touch")
            .args(["-m", "-t", "202001010000"])
            .arg(&path)
            .status()
            .expect("touch");
        assert!(status.success(), "could not backdate {}", path.display());
    }
}

/// An entry the store references. `pid: null` is the `--connect` shape, the one the dead-pid
/// prune keeps.
fn reference(home: &Path, name: &str) {
    let store = format!(
        r#"{{"browsers":{{"{name}":{{"wsEndpoint":"ws://127.0.0.1:9222/x","pid":null,"headless":false,"proxyServer":null,"daemonPid":null,"pages":{{}}}}}}}}"#
    );
    std::fs::write(home.join(".chrome-agent").join("sessions.json"), store).unwrap();
}

fn present(home: &Path) -> HashSet<String> {
    std::fs::read_dir(browsers(home))
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .collect()
}

/// Four profiles differing in exactly one condition each; only the one that fails none may go.
#[test]
fn a_save_removes_only_the_unreferenced_unheld_and_idle_profile() {
    let home = tmp_home("predicate");

    age(&profile(&home, "in-store"));
    reference(&home, "in-store");
    age(&profile(&home, "orphan-old"));
    profile(&home, "orphan-fresh");
    // Orphaned and idle, but its SingletonLock names a running process.
    let locked = profile(&home, "orphan-locked");
    let mut child = Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("stand-in for a live Chrome");
    let host =
        String::from_utf8_lossy(&Command::new("hostname").output().expect("hostname").stdout)
            .trim()
            .to_string();
    std::os::unix::fs::symlink(
        format!("{host}-{}", child.id()),
        locked.join("chromium-profile").join("SingletonLock"),
    )
    .unwrap();
    age(&locked);

    // isolation-exempt: a name that does not exist, in this test's own temporary HOME.
    let (_, code) = run_in(&home, &["close", "--browser", "never-opened"]);
    assert_eq!(code, 0, "close should succeed");

    let left = present(&home);
    assert!(
        !left.contains("orphan-old"),
        "the idle orphan survived: {left:?}"
    );
    for kept in ["in-store", "orphan-fresh", "orphan-locked"] {
        assert!(left.contains(kept), "{kept} was removed; left = {left:?}");
    }

    let _ = child.kill();
    let _ = child.wait();
    std::fs::remove_dir_all(&home).ok();
}

/// A read-only command must not pay for the backlog: one save removes one profile.
#[test]
fn one_save_removes_at_most_one_profile() {
    let home = tmp_home("cap");
    for i in 0..12 {
        age(&profile(&home, &format!("orphan-{i}")));
    }

    // isolation-exempt: a name that does not exist, in this test's own temporary HOME.
    let (_, code) = run_in(&home, &["close", "--browser", "never-opened"]);
    assert_eq!(code, 0);
    assert_eq!(
        present(&home).len(),
        11,
        "the per-invocation removal cap was not honoured"
    );

    std::fs::remove_dir_all(&home).ok();
}

/// Each agent has a profile and no store entry yet, so the grace window is the only thing
/// preventing mutual deletion.
#[test]
fn concurrent_saves_do_not_delete_each_others_fresh_profiles() {
    let home = tmp_home("race");
    profile(&home, "agent-a");
    profile(&home, "agent-b");

    let mut running = Vec::new();
    for name in ["agent-a", "agent-b"] {
        running.push(
            Command::new(common::binary())
                .args(["close", "--browser", name])
                .env("HOME", &home)
                .spawn()
                .expect("spawn a concurrent agent"),
        );
    }
    for mut child in running {
        assert!(child.wait().expect("wait").success());
    }

    let left = present(&home);
    for fresh in ["agent-a", "agent-b"] {
        assert!(
            left.contains(fresh),
            "{fresh}'s fresh profile was deleted; left = {left:?}"
        );
    }

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn purge_orphans_sweeps_the_whole_backlog_at_once() {
    let home = tmp_home("backlog");
    for i in 0..12 {
        age(&profile(&home, &format!("orphan-{i}")));
    }
    age(&profile(&home, "in-store"));
    reference(&home, "in-store");
    profile(&home, "fresh");

    let (out, code) = run_in(&home, &["close", "--purge-orphans"]);
    assert_eq!(code, 0, "purge-orphans should succeed: {out}");
    assert!(out.contains("Purged 12"), "unexpected output: {out}");

    let left = present(&home);
    assert_eq!(
        left,
        ["in-store".to_string(), "fresh".to_string()]
            .into_iter()
            .collect::<HashSet<_>>(),
        "purge-orphans removed the wrong set"
    );

    std::fs::remove_dir_all(&home).ok();
}

#[test]
fn a_directory_that_is_not_a_profile_survives() {
    let home = tmp_home("foreign");
    // No `chromium-profile` inside.
    let notes = browsers(&home).join("notes");
    std::fs::create_dir_all(&notes).unwrap();
    std::fs::write(notes.join("keep.txt"), "mine").unwrap();
    Command::new("touch")
        .args(["-m", "-t", "202001010000"])
        .arg(&notes)
        .status()
        .unwrap();
    age(&profile(&home, "orphan-old"));

    let (_, code) = run_in(&home, &["close", "--purge-orphans"]);
    assert_eq!(code, 0);

    assert!(
        notes.join("keep.txt").exists(),
        "a foreign directory was deleted"
    );
    assert!(
        !browsers(&home).join("orphan-old").exists(),
        "the orphan survived"
    );

    std::fs::remove_dir_all(&home).ok();
}
