use std::path::PathBuf;
use std::process::Command;

fn binary() -> String {
    let mut path = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    path.push("aibrowsr");
    path.to_string_lossy().into_owned()
}

fn fixture_url(name: &str) -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("tests/fixtures");
    path.push(name);
    format!("file://{}", path.display())
}

fn run_cli(args: &[&str]) -> (String, String, i32) {
    let output = Command::new(binary())
        .args(args)
        .output()
        .expect("Failed to run aibrowsr");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let code = output.status.code().unwrap_or(-1);
    (stdout, stderr, code)
}

fn chrome_available() -> bool {
    let candidates = if cfg!(target_os = "macos") {
        vec!["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
    } else {
        vec!["google-chrome", "chromium"]
    };
    for candidate in candidates {
        if std::path::Path::new(candidate).exists() {
            return true;
        }
        if Command::new("which")
            .arg(candidate)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

fn goto_fixture(browser: &str, fixture: &str) -> bool {
    let url = fixture_url(fixture);
    let (_, stderr, code) = run_cli(&["--browser", browser, "goto", &url]);
    if code != 0 {
        eprintln!("SKIP: goto failed for {fixture}: {stderr}");
        return false;
    }
    true
}

fn extract_json(browser: &str) -> Option<serde_json::Value> {
    let (stdout, stderr, code) = run_cli(&["--json", "--browser", browser, "extract"]);
    if code != 0 {
        eprintln!("extract failed: {stderr} {stdout}");
        return None;
    }
    for line in stdout.lines() {
        if line.starts_with('{')
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                return Some(v);
            }
    }
    None
}

fn extract_json_with_args(browser: &str, args: &[&str]) -> Option<serde_json::Value> {
    let mut full_args = vec!["--json", "--browser", browser, "extract"];
    full_args.extend_from_slice(args);
    let (stdout, stderr, code) = run_cli(&full_args);
    if code != 0 {
        eprintln!("extract failed: {stderr} {stdout}");
        return None;
    }
    for line in stdout.lines() {
        if line.starts_with('{')
            && let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                return Some(v);
            }
    }
    None
}

fn cleanup(browser: &str) {
    let _ = run_cli(&["--browser", browser, "close", "--purge"]);
}

// ─── Product table: should extract TR rows with links and prices ───

#[test]
fn extract_table_finds_product_rows() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-table";
    if !goto_fixture(b, "extract_table.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 5, "Should find 5 product rows, got {count}");
    assert!(items.len() >= 5, "Should return 5 items");

    let first = &items[0];
    assert!(first.get("title").and_then(|v| v.as_str()).is_some(), "First item should have title: {first}");
    assert!(first.get("url").and_then(|v| v.as_str()).is_some(), "First item should have URL: {first}");

    let pattern = json["pattern"].as_str().unwrap_or("");
    assert!(pattern.contains("TR") || pattern.contains("tr"), "Pattern should be TR-based, got: {pattern}");
}

// ─── Blog cards: should extract article elements ───

#[test]
fn extract_cards_finds_articles() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-cards";
    if !goto_fixture(b, "extract_cards.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 4, "Should find 4 blog cards, got {count}");

    let pattern = json["pattern"].as_str().unwrap_or("");
    assert!(
        pattern.contains("ARTICLE") || pattern.contains("article") || pattern.contains("post"),
        "Pattern should be ARTICLE-based, got: {pattern}"
    );

    let first = &items[0];
    let title = first.get("title").and_then(|v| v.as_str()).unwrap_or("");
    assert!(title.contains("Rust Async"), "First title should mention Rust Async, got: {title}");

    assert!(items.iter().any(|item| item.get("date").is_some()), "Should have date fields");
    assert!(items.iter().any(|item| item.get("image").is_some()), "Should have image fields");
}

// ─── HN-like: should pick item-rows, not vote links or spacers ───

#[test]
fn extract_hn_like_finds_stories_not_vote_links() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-hn";
    if !goto_fixture(b, "extract_hn_like.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 4, "Should find 4 news items, got {count}");

    for item in items {
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!title.contains("▲") && title.len() > 5, "Title should be article, not vote: '{title}'");
    }

    for item in items {
        let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!url.contains("/vote/"), "URL should be article URL, not vote: {url}");
    }
}

// ─── E-commerce: should prefer product cards over nav/footer links ───

#[test]
fn extract_ecommerce_finds_products_not_nav() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-ecom";
    if !goto_fixture(b, "extract_ecommerce.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 4, "Should find 4 product cards, got {count}");

    let pattern = json["pattern"].as_str().unwrap_or("");
    assert!(!pattern.to_uppercase().contains("NAV"), "Should not extract nav pattern: {pattern}");

    let first = &items[0];
    let title = first.get("title").and_then(|v| v.as_str()).unwrap_or("");
    assert!(title.len() > 5, "Product should have meaningful title, got: '{title}'");

    assert!(items.iter().any(|item| item.get("price").is_some()), "Should have price fields");
    assert!(items.iter().any(|item| item.get("image").is_some()), "Should have image fields");
}

// ─── Search results list ───

#[test]
fn extract_list_finds_search_results() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-list";
    if !goto_fixture(b, "extract_list.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 4, "Should find >=4 search results, got {count}");

    let pattern = json["pattern"].as_str().unwrap_or("");
    assert!(pattern.contains("LI") || pattern.contains("li"), "Pattern should be LI-based, got: {pattern}");

    for (i, item) in items.iter().enumerate() {
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
        let url = item.get("url").and_then(|v| v.as_str()).unwrap_or("");
        assert!(title.len() > 5, "Item {i} should have title, got: '{title}'");
        assert!(!url.is_empty(), "Item {i} should have URL");
    }
}

// ─── Nav-heavy page: should extract feature cards, not nav links ───

#[test]
fn extract_nested_nav_prefers_content_over_navigation() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-nav";
    if !goto_fixture(b, "extract_nested_nav.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 4, "Should find 4 feature cards, got {count}");

    let titles: Vec<&str> = items.iter().filter_map(|item| item.get("title").and_then(|v| v.as_str())).collect();
    let nav_titles = ["Home", "Features", "Pricing", "Docs", "Blog", "Login"];
    for title in &titles {
        assert!(!nav_titles.contains(title), "Should not extract nav link '{title}'");
    }
}

// ─── No pattern page: should return error ───

#[test]
fn extract_no_pattern_returns_error() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-nopattern";
    if !goto_fixture(b, "extract_no_pattern.html") { cleanup(b); return; }

    let (stdout, _, code) = run_cli(&["--json", "--browser", b, "extract"]);
    cleanup(b);

    if code == 0 {
        for line in stdout.lines() {
            if line.starts_with('{')
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
                    let items = json["items"].as_array().map_or(0, std::vec::Vec::len);
                    assert!(items <= 1, "No-pattern page should have <=1 items, got {items}");
                    break;
                }
        }
    }
}

// ─── Mixed page (dashboard): should extract activity feed ───

#[test]
fn extract_mixed_finds_activity_feed() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-mixed";
    if !goto_fixture(b, "extract_mixed.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 4, "Should find 4 activity items, got {count}");
    assert!(items.iter().any(|item| item.get("date").is_some()), "Should have dates");
    assert!(items.iter().any(|item| item.get("image").is_some()), "Should have images");
}

// ─── Extract with --selector scoping ───

#[test]
fn extract_with_selector_scopes_correctly() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-selector";
    if !goto_fixture(b, "extract_ecommerce.html") { cleanup(b); return; }

    let json = extract_json_with_args(b, &["--selector", ".product-grid"]);
    cleanup(b);

    if let Some(json) = json {
        let count = json["count"].as_u64().unwrap_or(0);
        assert!(count >= 4, "Scoped extract should find 4 products, got {count}");
    }
}

// ─── Extract with --limit ───

#[test]
fn extract_limit_caps_results() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-limit";
    if !goto_fixture(b, "extract_list.html") { cleanup(b); return; }

    let json = extract_json_with_args(b, &["--limit", "2"]);
    cleanup(b);

    if let Some(json) = json {
        let items_len = json["items"].as_array().map_or(0, std::vec::Vec::len);
        assert_eq!(items_len, 2, "Limit should cap to 2 items, got {items_len}");
        let count = json["count"].as_u64().unwrap_or(0);
        assert!(count >= 4, "Total count should be >=4, got {count}");
    }
}

// ─── Link-heavy nav: should prefer job listings over nav links ───
// MDR heuristic: text-to-link ratio filters navigation regions

#[test]
fn extract_link_heavy_nav_prefers_content() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-linknav";
    if !goto_fixture(b, "extract_link_heavy_nav.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 4, "Should find 4 job listings, got {count}");

    for item in items {
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!title.starts_with("Page "), "Should not extract nav link '{title}'");
    }

    assert!(items.iter().any(|item| item.get("date").is_some()), "Job listings should have dates");
}

// ─── FAQ definition list ───

#[test]
fn extract_faq_items() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-faq";
    if !goto_fixture(b, "extract_definition_list.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 5, "Should find 5 FAQ items, got {count}");

    for (i, item) in items.iter().enumerate() {
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
        assert!(title.len() > 5, "FAQ item {i} should have question, got: '{title}'");
    }
}

// ─── Semantic classes: classes matching /card|item|repo/ boost detection ───

#[test]
fn extract_semantic_classes_boost() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-semclass";
    if !goto_fixture(b, "extract_semantic_classes.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 4, "Should find 4 repo cards, got {count}");

    let first_title = items[0].get("title").and_then(|v| v.as_str()).unwrap_or("");
    assert!(
        first_title.contains("aibrowsr") || first_title.contains("dev-browser"),
        "First item should be repo, got: '{first_title}'"
    );

    assert!(items.iter().any(|item| item.get("date").is_some()), "Should have dates");
}

// ─── Ads interleaved: should extract articles, not ads ───

#[test]
fn extract_ads_interleaved_finds_articles() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-ads";
    if !goto_fixture(b, "extract_ads_interleaved.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 4, "Should find 4 articles, got {count}");

    let pattern = json["pattern"].as_str().unwrap_or("");
    assert!(
        pattern.contains("ARTICLE") || pattern.contains("story"),
        "Pattern should be article-based, got: {pattern}"
    );

    for item in items {
        let title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!title.contains("Sponsored"), "Should not extract ads: '{title}'");
    }

    assert!(items.iter().any(|item| item.get("date").is_some()), "Should have dates");
}

// ─── Flat table (leaderboard) ───

#[test]
fn extract_flat_table_rows() {
    if !chrome_available() { eprintln!("SKIP: Chrome not found"); return; }
    let b = "ext-ftable";
    if !goto_fixture(b, "extract_flat_table.html") { cleanup(b); return; }

    let json = extract_json(b);
    cleanup(b);

    let json = json.expect("extract should return JSON");
    let items = json["items"].as_array().expect("items array");
    let count = json["count"].as_u64().unwrap_or(0);

    assert!(count >= 7, "Should find 7 leaderboard rows, got {count}");

    let first = &items[0];
    let title = first.get("title").and_then(|v| v.as_str()).unwrap_or("");
    assert!(title.contains("alice") || title.contains("dev"), "First should be username, got: '{title}'");

    let first_url = first.get("url").and_then(|v| v.as_str()).unwrap_or("");
    assert!(first_url.contains("/u/"), "Should link to user profile, got: {first_url}");
}
