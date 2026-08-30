//! Every command line the documents an agent reads show must actually parse.
//!
//! `llm-guide.txt` is compiled into `--help` as `after_long_help`, and an agent is its
//! main reader: what it shows is what gets typed. It showed `chrome-agent pdf --out
//! page.pdf`, and `pdf` has no `--out` (that flag belongs to `download`), so following
//! the documentation verbatim produced a clap parse error. Nothing checked the text
//! against the parser.
//!
//! The guide was never the only document read that way, and it was the only one checked.
//! `npm/README.md` is what npmjs.com renders on the package's front page, and it showed
//! `chrome-agent --connect inspect` — `--connect` takes a value, so clap consumed
//! `inspect` as that value and the invocation died on a missing subcommand. Same class of
//! defect, on the more widely read file, for two releases. So the field is now the six
//! documents together: the guide, the three READMEs and the two skill files.
//!
//! Parsing only — no browser is launched: `CHROME_AGENT_PARSE_ONLY` makes the binary
//! return the moment clap has spoken. Synopsis lines using `[--flag name]` notation are
//! not invocations and are checked by the second test instead.

use std::process::Command;

/// A document whose command lines an agent copies.
///
/// `fenced` is the difference between a plain-text guide, every line of which is meant to
/// be readable as a command, and a markdown file, where only a ```` ```bash ```` block is.
/// Without that distinction README.md contributed two false failures: line 120 is inside an
/// unlabelled fence holding an ASCII diagram whose first line is the binary's name, and
/// line 316 is a sentence of prose that begins `chrome-agent depends on its own values…`.
struct Doc {
    name: &'static str,
    text: &'static str,
    fenced: bool,
}

const DOCS: &[Doc] = &[
    Doc { name: "llm-guide.txt", text: include_str!("../llm-guide.txt"), fenced: false },
    Doc { name: "README.md", text: include_str!("../README.md"), fenced: true },
    Doc { name: "README.cn.md", text: include_str!("../README.cn.md"), fenced: true },
    Doc { name: "npm/README.md", text: include_str!("../npm/README.md"), fenced: true },
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

fn binary() -> String {
    let mut path = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().to_path_buf();
    path.push("chrome-agent");
    path.to_string_lossy().into_owned()
}

/// Cut a line at the first `needle` that is outside quotes.
///
/// Two shapes need it and they are not the same shape. `#` starts a comment — but only
/// outside quotes: `--selector "#country"` is a CSS id, and cutting there turned a valid
/// example into a truncated one the parser then rejected for the wrong reason. `;` ends
/// the command and starts a second one: README.md:230 is
/// `chrome-agent assert value … --equals "SAVE10"; echo $?`, and `echo $?` is the point
/// being made about exit codes, not an argument.
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

/// The comment goes first, then the second command: the `;` this exists for is written
/// before the `#` on README.md:230, and every `;` in llm-guide.txt is inside a comment.
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

/// A line the parser cannot be asked about: help output, or a synopsis rather than a real
/// invocation.
///
/// `|` is in this list and is NOT treated as a shell pipe, which is the reading that would
/// be convenient: `chrome-agent scroll down|up|<uid>` and `chrome-agent press
/// Enter|Tab|Escape` are alternations, and cutting them at the bar would hand the parser
/// `scroll down` and `press Enter`, which parse — a synopsis silently promoted to a
/// verified invocation is precisely the false green this file exists to remove. No line in
/// any of the six documents pipes a chrome-agent invocation into another command, so
/// nothing is lost by reading the bar as alternation everywhere.
fn is_synopsis(line: &str) -> bool {
    line.contains('[')
        || line.contains(']')
        || line.contains('<')
        || line.contains('|')
        || line.contains("--help")
}

/// Every `chrome-agent …` line of one document, with continuations joined.
///
/// A `\` at the end of a line continues it (README.md:495 spreads one `emulate device`
/// over two lines), so the two halves are one command and only the whole thing can be
/// handed to a parser.
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
            out.push(Example { doc: doc.name, line: start, text });
        }
    }
    out
}

/// Every invocation across the six documents: a `chrome-agent …` line that is a command and
/// not a synopsis.
fn invocations() -> Vec<Example> {
    let mut out = Vec::new();
    for doc in DOCS {
        for example in command_lines(doc) {
            let text = command_only(&example.text).to_string();
            if text.starts_with("chrome-agent ") && !is_synopsis(&text) {
                out.push(Example { doc: example.doc, line: example.line, text });
            }
        }
    }
    out
}

#[test]
fn every_command_line_the_documents_show_parses() {
    let examples = invocations();

    // A per-document floor, not one total: a fence that stops being recognised in ONE file
    // takes that file's whole contribution out of the field, and a total large enough to
    // pass would hide it. Each number is comfortably below what the file holds today.
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
        // CHROME_AGENT_PARSE_ONLY returns right after Cli::parse(), so clap's full
        // verdict is the exit code — including missing required arguments, which
        // appending `--help` would have short-circuited past.
        let output = Command::new(binary())
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
                String::from_utf8_lossy(&output.stderr).lines().next().unwrap_or("(no stderr)")
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
    let out = Command::new(binary()).args(&args).output().expect("run chrome-agent");
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

/// The synopsis lines name flags too; those flag names must exist on that command.
///
/// The command is a *path*, not a word: `assert value --equals x` puts the flag on the leaf,
/// and `assert --help` lists only its subcommands. A following word joins the path only when
/// its help differs from its parent's — otherwise `goto https://example.com --help`, which
/// clap happily answers with `goto`'s help, would read the URL as a subcommand.
#[test]
fn every_flag_named_in_a_synopsis_exists_on_its_command() {
    let guide = &DOCS[0];
    let mut broken = Vec::new();
    for example in command_lines(guide) {
        let Some(rest) = example.text.strip_prefix("chrome-agent ") else { continue };
        let rest = strip_comment(rest);
        let mut words = rest.split_whitespace();
        let Some(command) = words.next() else { continue };
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
    assert!(broken.is_empty(), "the guide names flags that do not exist:\n{}", broken.join("\n"));
}

/// The size of this codebase is published as a competitive argument, in four places, and two
/// different values of it were in print at once — neither of them the measurement.
///
/// README.md, README.cn.md and npm/README.md all said `~10.2K lines` in their comparison
/// tables, and npm/README.md's own architecture diagram said `~11.5K lines` eleven lines above
/// one of them. The measurement was 22.2K, a factor of 2.2 on the number that carries the
/// argument. A bare number in a table is exactly the shape that drifts: nothing names what it
/// counts, so nothing can re-count it.
///
/// The metric is defined here and the documents state it in those words: lines of Rust
/// under `src/`, blank lines and comment-only lines excluded, in-source tests included.
/// The tolerance is deliberately wide — 5%, so ordinary growth does not turn a release into
/// a documentation edit, while the factor of 2.2 this test was written for cannot survive a
/// single run. Tightening it would make the guard expensive enough to be disabled, which is
/// how the last one stopped being true.
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
            let Some(claim) = published_line_count(line) else { continue };
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
/// The number is read BACKWARDS from the phrase rather than forwards from the first `~` on
/// the line: both comparison tables put a competitor's figure in the next cell, and taking
/// the first tilde would let a column reorder silently measure the wrong project. The
/// metric has to be named against the number it qualifies — a bare `~10.2K lines` is
/// unverifiable by construction, and matching it here would let this test bless it.
fn published_line_count(line: &str) -> Option<usize> {
    // One metric, written once per language. The marker has to appear in the translated file
    // too: README.cn.md carried a fourth copy of the wrong number (`~10.2K 行`), and a guard
    // that only reads English would have left it there.
    const MARKERS: [&str; 2] = ["K lines of Rust in src/", "K 行 Rust 代码（src/"];
    let line = line.replace('`', "");
    let end = MARKERS.iter().find_map(|marker| line.find(marker))?;
    let reversed: String =
        line[..end].chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    if !line[..end - reversed.len()].ends_with('~') {
        return None;
    }
    let digits: String = reversed.chars().rev().collect();
    // Read in integers rather than through an f64: a float here costs two lossy casts to
    // compare it with a line count, and 22.2K is a fixed-point number written by hand.
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
    walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut total);
    total
}
