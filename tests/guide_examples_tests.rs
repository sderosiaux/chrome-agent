//! Every command line in the six documents an agent reads must parse: the guide, the three
//! READMEs and the two skill files.
//!
//! Parsing only, no browser: `CHROME_AGENT_PARSE_ONLY` returns as soon as clap has spoken.
//! Synopsis lines (`[--flag name]`) are not invocations and are covered by the second test.

use std::process::Command;

mod common;

/// `fenced` separates a plain-text guide, where every line may be a command, from markdown,
/// where only a ```` ```bash ```` block is.
struct Doc {
    name: &'static str,
    text: &'static str,
    fenced: bool,
}

const DOCS: &[Doc] = &[
    Doc {
        name: "llm-guide.txt",
        text: include_str!("../llm-guide.txt"),
        fenced: false,
    },
    Doc {
        name: "README.md",
        text: include_str!("../README.md"),
        fenced: true,
    },
    Doc {
        name: "README.cn.md",
        text: include_str!("../README.cn.md"),
        fenced: true,
    },
    Doc {
        name: "npm/README.md",
        text: include_str!("../npm/README.md"),
        fenced: true,
    },
    Doc {
        name: "skills/chrome-agent/SKILL.md",
        text: include_str!("../skills/chrome-agent/SKILL.md"),
        fenced: true,
    },
    Doc {
        name: "skills/scrape-structured-data/SKILL.md",
        text: include_str!("../skills/scrape-structured-data/SKILL.md"),
        fenced: true,
    },
];

/// One command line, and where it is written.
struct Example {
    doc: &'static str,
    line: usize,
    text: String,
}

/// Cut at the first `needle` outside quotes: `--selector "#country"` is a CSS id, not a
/// comment.
fn cut_outside_quotes(line: &str, needle: char) -> &str {
    let mut quote: Option<char> = None;
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == needle => return line[..i].trim_end(),
            Some(_) | None => {}
        }
    }
    line.trim()
}

/// Drop a trailing `# comment`, ignoring `#` inside quotes.
fn strip_comment(line: &str) -> &str {
    cut_outside_quotes(line, '#')
}

/// The comment is stripped first: every `;` in llm-guide.txt is inside one.
fn command_only(line: &str) -> &str {
    cut_outside_quotes(strip_comment(line), ';')
}

/// Split a documented example into argv, honouring the quotes the guide uses.
fn argv(line: &str) -> Vec<String> {
    let line = command_only(line);
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

/// A line the parser cannot be asked about: help output, or a synopsis.
///
/// `|` counts as alternation, not a shell pipe: `press Enter|Tab|Escape` is a synopsis, and
/// no line in the six documents pipes a chrome-agent invocation into another command.
fn is_synopsis(line: &str) -> bool {
    line.contains('[')
        || line.contains(']')
        || line.contains('<')
        || line.contains('|')
        || line.contains("--help")
}

/// Every `chrome-agent …` line of one document, with `\` continuations joined.
fn command_lines(doc: &Doc) -> Vec<Example> {
    let mut out: Vec<Example> = Vec::new();
    let mut in_block = !doc.fenced;
    let mut pending: Option<(usize, String)> = None;
    for (index, raw) in doc.text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if doc.fenced && trimmed.starts_with("```") {
            in_block = trimmed == "```bash";
            pending = None;
            continue;
        }
        if !in_block {
            continue;
        }
        let (piece, continues) = match trimmed.strip_suffix('\\') {
            Some(head) => (head.trim_end(), true),
            None => (trimmed, false),
        };
        let (start, text) = match pending.take() {
            Some((start, mut acc)) => {
                acc.push(' ');
                acc.push_str(piece);
                (start, acc)
            }
            None => (line, piece.to_string()),
        };
        if continues {
            pending = Some((start, text));
        } else {
            out.push(Example {
                doc: doc.name,
                line: start,
                text,
            });
        }
    }
    out
}

/// Every `chrome-agent …` line across the six documents that is a command, not a synopsis.
fn invocations() -> Vec<Example> {
    let mut out = Vec::new();
    for doc in DOCS {
        for example in command_lines(doc) {
            let text = command_only(&example.text).to_string();
            if text.starts_with("chrome-agent ") && !is_synopsis(&text) {
                out.push(Example {
                    doc: example.doc,
                    line: example.line,
                    text,
                });
            }
        }
    }
    out
}

#[test]
fn every_command_line_the_documents_show_parses() {
    let examples = invocations();

    // A per-document floor, not one total: a fence that stops being recognised in one file
    // would take that file out of the field, and a total would hide it.
    for doc in DOCS {
        let found = examples.iter().filter(|e| e.doc == doc.name).count();
        assert!(
            found >= 10,
            "only {found} invocation(s) extracted from {} — did its ```bash fences change?",
            doc.name
        );
    }

    let mut broken = Vec::new();
    for example in &examples {
        let args = argv(&example.text);
        // CHROME_AGENT_PARSE_ONLY returns right after `Cli::parse()`, so the exit code is
        // clap's full verdict, missing arguments included.
        let output = Command::new(common::binary())
            .args(&args[1..])
            .env("CHROME_AGENT_PARSE_ONLY", "1")
            .output()
            .expect("run chrome-agent");
        if !output.status.success() {
            broken.push(format!(
                "{}:{}: {}\n    -> {}",
                example.doc,
                example.line,
                example.text,
                String::from_utf8_lossy(&output.stderr)
                    .lines()
                    .next()
                    .unwrap_or("(no stderr)")
            ));
        }
    }

    assert!(
        broken.is_empty(),
        "{} of the {} documented command line(s) are rejected by the parser:\n{}",
        broken.len(),
        examples.len(),
        broken.join("\n")
    );
}

/// Ask clap for a command's help text, or `None` when that path is not a command.
fn help_for(path: &[String]) -> Option<String> {
    let mut args: Vec<&str> = path.iter().map(String::as_str).collect();
    args.push("--help");
    let out = Command::new(common::binary())
        .args(&args)
        .output()
        .expect("run chrome-agent");
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// A flag named in a synopsis must exist on that command. The command is a *path*, not a word
/// (`assert value --equals`), and a word joins it only when its help differs from its parent's.
#[test]
fn every_flag_named_in_a_synopsis_exists_on_its_command() {
    let guide = &DOCS[0];
    let mut broken = Vec::new();
    for example in command_lines(guide) {
        let Some(rest) = example.text.strip_prefix("chrome-agent ") else {
            continue;
        };
        let rest = strip_comment(rest);
        let mut words = rest.split_whitespace();
        let Some(command) = words.next() else {
            continue;
        };
        if command.starts_with('-') || command.starts_with('<') || command.starts_with('[') {
            continue;
        }
        let mut path = vec![command.to_string()];
        let Some(mut help_text) = help_for(&path) else {
            continue; // not a subcommand (e.g. a bare URL example)
        };
        for word in words {
            if word.starts_with('-') || word.starts_with('<') || word.starts_with('[') {
                break;
            }
            let mut candidate = path.clone();
            candidate.push(word.to_string());
            match help_for(&candidate) {
                Some(deeper) if deeper != help_text => {
                    path = candidate;
                    help_text = deeper;
                }
                _ => break,
            }
        }
        let named = path.join(" ");
        for word in rest.split(|c: char| c.is_whitespace() || c == '[' || c == ']') {
            let flag = word.trim_matches(|c| c == ',' || c == '.');
            if !flag.starts_with("--") || flag.len() < 4 {
                continue;
            }
            if !help_text.contains(flag) {
                broken.push(format!(
                    "`chrome-agent {named}` has no {flag} ({}:{})",
                    example.doc, example.line
                ));
            }
        }
    }
    assert!(
        broken.is_empty(),
        "the guide names flags that do not exist:\n{}",
        broken.join("\n")
    );
}

/// The published codebase size matches the measurement, in every document that states it.
///
/// The metric: lines of Rust under `src/`, blank and comment-only lines excluded, in-source
/// tests included. The 5% tolerance is wide so ordinary growth is not a documentation edit.
#[test]
fn the_published_size_of_this_codebase_is_the_measured_one() {
    let measured = measure_source_lines();
    let published = [
        ("README.md", include_str!("../README.md")),
        ("README.cn.md", include_str!("../README.cn.md")),
        ("npm/README.md", include_str!("../npm/README.md")),
    ];
    let mut found = 0;
    for (name, text) in published {
        for (index, line) in text.lines().enumerate() {
            let Some(claim) = published_line_count(line) else {
                continue;
            };
            found += 1;
            let drift = claim.abs_diff(measured);
            assert!(
                drift * 100 <= measured * 5,
                "{name}:{} publishes {claim} lines of Rust; src/ holds {measured} ({}% off). \
                 Re-measure it.",
                index + 1,
                drift * 100 / measured
            );
        }
    }
    assert!(
        found >= 4,
        "expected every published size to state its metric, found {found} — a bare number in a \
         table is the shape this test exists to refuse"
    );
}

/// A published `~NN.NK lines of Rust in src/` claim, as a line count.
///
/// Read BACKWARDS from the phrase, not forwards from the first `~`: the comparison tables put
/// a competitor's figure in the next cell. A bare `~10.2K lines` is deliberately not matched.
fn published_line_count(line: &str) -> Option<usize> {
    // One marker per language: a guard that only reads English leaves the translated file
    // stale.
    const MARKERS: [&str; 2] = ["K lines of Rust in src/", "K 行 Rust 代码（src/"];
    let line = line.replace('`', "");
    let end = MARKERS.iter().find_map(|marker| line.find(marker))?;
    let reversed: String = line[..end]
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if !line[..end - reversed.len()].ends_with('~') {
        return None;
    }
    let digits: String = reversed.chars().rev().collect();
    // Parsed as integers, not an f64: the published figure is fixed-point.
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits.as_str(), ""));
    let mut value: usize = whole.parse::<usize>().ok()? * 1000;
    let mut place = 100;
    for digit in fraction.chars() {
        value += usize::try_from(digit.to_digit(10)?).ok()? * place;
        place /= 10;
    }
    Some(value)
}

/// Lines of Rust under `src/`, blank and comment-only lines excluded.
fn measure_source_lines() -> usize {
    fn walk(dir: &std::path::Path, total: &mut usize) {
        for entry in std::fs::read_dir(dir).expect("read src/") {
            let path = entry.expect("read src/ entry").path();
            if path.is_dir() {
                walk(&path, total);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let text = std::fs::read_to_string(&path).expect("read a source file");
                *total += text
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with("//"))
                    .count();
            }
        }
    }
    let mut total = 0;
    walk(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
        &mut total,
    );
    total
}
