//! Every command line the embedded guide shows must actually parse.
//!
//! `llm-guide.txt` is compiled into `--help` as `after_long_help`, and an agent is its
//! main reader: what it shows is what gets typed. It showed `chrome-agent pdf --out
//! page.pdf`, and `pdf` has no `--out` (that flag belongs to `download`), so following
//! the documentation verbatim produced a clap parse error. Nothing checked the text
//! against the parser.
//!
//! Parsing only — no browser is launched. `--help` is the one thing that cannot be
//! parsed this way (clap exits), so those lines are skipped explicitly.

use std::process::Command;

fn binary() -> String {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

/// Split a documented example into argv, honouring the quotes the guide uses, and
/// dropping the trailing `# comment`.
fn argv(line: &str) -> Vec<String> {
    let line = line.split_once('#').map_or(line, |(before, _)| before).trim();
    let mut args = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started || !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            Some(_) | None => current.push(c),
        }
    }
    if started || !current.is_empty() {
        args.push(current);
    }
    args
}

/// A line the parser cannot be asked about: help output, or a synopsis using the
/// `[--flag name]` optional-argument notation rather than a real invocation.
fn is_synopsis(line: &str) -> bool {
    line.contains('[') || line.contains(']') || line.contains("--help") || line.contains('<')
}

#[test]
fn every_example_in_the_embedded_guide_parses() {
    let guide = include_str!("../llm-guide.txt");
    let examples: Vec<&str> = guide
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("chrome-agent ") && !is_synopsis(l))
        .collect();
    assert!(
        examples.len() > 30,
        "expected the guide's examples to be found, got {} — did the format change?",
        examples.len()
    );

    let mut broken = Vec::new();
    for example in &examples {
        let args = argv(example);
        // `--dry-parse` does not exist; `--help` on the subcommand makes clap validate
        // the path and exit 0 without running anything, which is exactly the check.
        let mut with_help = args[1..].to_vec();
        with_help.push("--help".into());
        let output = Command::new(binary())
            .args(&with_help)
            .output()
            .expect("run chrome-agent");
        if !output.status.success() {
            broken.push(format!(
                "{example}\n    -> {}",
                String::from_utf8_lossy(&output.stderr).lines().next().unwrap_or("(no stderr)")
            ));
        }
    }

    assert!(
        broken.is_empty(),
        "the embedded guide documents {} command line(s) the parser rejects:\n{}",
        broken.len(),
        broken.join("\n")
    );
}

/// The synopsis lines name flags too; those flag names must exist on that command.
#[test]
fn every_flag_named_in_a_synopsis_exists_on_its_command() {
    let guide = include_str!("../llm-guide.txt");
    let mut broken = Vec::new();
    for line in guide.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix("chrome-agent ") else { continue };
        let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
        let mut words = rest.split_whitespace();
        let Some(command) = words.next() else { continue };
        if command.starts_with('-') || command.starts_with('<') || command.starts_with('[') {
            continue;
        }
        let help = Command::new(binary())
            .args([command, "--help"])
            .output()
            .expect("run chrome-agent");
        if !help.status.success() {
            continue; // not a subcommand (e.g. a bare URL example)
        }
        let help_text = String::from_utf8_lossy(&help.stdout).to_string();
        for word in rest.split(|c: char| c.is_whitespace() || c == '[' || c == ']') {
            let flag = word.trim_matches(|c| c == ',' || c == '.');
            if !flag.starts_with("--") || flag.len() < 4 {
                continue;
            }
            if !help_text.contains(flag) {
                broken.push(format!("`chrome-agent {command}` has no {flag} (line: {line})"));
            }
        }
    }
    assert!(broken.is_empty(), "the guide names flags that do not exist:\n{}", broken.join("\n"));
}
