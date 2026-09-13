use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::common;

pub fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(common::binary())
        .args(args)
        .output()
        .expect("run chrome-agent");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

/// Feed a pipe session and hand back one parsed response per line.
pub fn run_pipe(browser: &str, commands: &[Value]) -> Vec<Value> {
    use std::io::Write as _;
    let mut child = Command::new(common::binary())
        .args(["--browser", browser, "pipe"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn pipe");
    {
        let stdin = child.stdin.as_mut().expect("stdin");
        for cmd in commands {
            writeln!(stdin, "{cmd}").expect("write");
        }
    }
    let output = child.wait_with_output().expect("pipe output");
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap_or_else(|e| panic!("not JSON ({e}): {line}")))
        .collect()
}

/// A macro file this test owns, removed when it ends.
pub struct TestMacro(String);

impl TestMacro {
    pub fn new(label: &str) -> Self {
        Self(common::unique_name(label))
    }
    pub fn name(&self) -> &str {
        &self.0
    }
    pub fn path(&self) -> std::path::PathBuf {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .expect("HOME");
        home.join(".chrome-agent")
            .join("macros")
            .join(format!("{}.json", self.0))
    }
    /// Write one by hand, so a test can exercise `macro run` without the recorder.
    pub fn write(&self, body: Value) {
        let path = self.path();
        std::fs::create_dir_all(path.parent().expect("parent")).expect("macro dir");
        let mut file = body;
        file["name"] = json!(self.0);
        std::fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).expect("write macro");
    }
}

impl Drop for TestMacro {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.path());
    }
}
