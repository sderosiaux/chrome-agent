//! Runs `vendor/extract.js`'s jsdom unit suite (`tests/js/`) inside `cargo test`. Skips when
//! node or the jsdom install is missing; that skip is fatal under `CHROME_AGENT_REQUIRE_CHROME`.

use std::path::PathBuf;
use std::process::Command;

mod common;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn the_extraction_engine_suite_passes() {
    let root = repo_root();
    let js_dir = root.join("tests/js");

    if Command::new("node").arg("--version").output().is_err() {
        common::unavailable("node not found — the extract.js suite cannot run");
        return;
    }
    if !js_dir.join("node_modules/jsdom").exists() {
        common::unavailable("tests/js/node_modules/jsdom missing — run `npm ci` in tests/js");
        return;
    }

    let tests: Vec<PathBuf> = std::fs::read_dir(&js_dir)
        .expect("read tests/js")
        .filter_map(|e| {
            let path = e.ok()?.path();
            path.to_str()?.ends_with(".test.js").then_some(path)
        })
        .collect();
    assert!(
        !tests.is_empty(),
        "no *.test.js under {} — an empty suite reports the same green as a passing one",
        js_dir.display()
    );

    let output = Command::new("node")
        .arg("--test")
        .args(&tests)
        .current_dir(&root)
        .output()
        .expect("run node --test");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "the extract.js suite failed:\n{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // A runner that silently matched nothing exits 0 too.
    let passed: usize = stdout
        .lines()
        .find_map(|l| l.strip_prefix("# pass ")?.trim().parse().ok())
        .expect("node --test should report a pass count");
    assert!(
        passed >= 100,
        "expected the full extraction suite (100+ tests), got {passed} — did the runner match the files?"
    );
}
