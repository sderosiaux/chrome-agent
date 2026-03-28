use serde::Serialize;
use serde_json::{json, Value};

use crate::cdp::client::CdpClient;

#[derive(Debug, Serialize)]
pub struct ExtractResult {
    pub items: Vec<Value>,
    pub count: usize,
    pub pattern: String,
}

pub async fn run(
    client: &CdpClient,
    selector: Option<&str>,
    limit: usize,
) -> Result<ExtractResult, Box<dyn std::error::Error>> {
    // Build the JS expression, injecting `limit` and optional `selector` scope.
    let scope_js = if let Some(sel) = selector {
        let escaped = serde_json::to_string(sel).unwrap_or_default();
        format!(
            "const _scope = document.querySelector({escaped}); if (!_scope) return JSON.stringify({{ items: [], hint: 'Selector ' + {escaped} + ' not found' }});"
        )
    } else {
        "const _scope = document;".to_string()
    };

    let js = format!(
        r#"(() => {{
  {scope_js}
  const _limit = {limit};
  const elements = _scope.querySelectorAll('*');
  const groups = {{}};
  for (const el of elements) {{
    if (!el.children.length && !el.textContent.trim()) continue;
    const sig = el.tagName + '.' + [...el.classList].sort().join('.');
    if (!groups[sig]) groups[sig] = [];
    groups[sig].push(el);
  }}

  let bestGroup = null;
  let bestSize = 0;
  for (const [sig, els] of Object.entries(groups)) {{
    const withText = els.filter(e => e.textContent.trim().length > 10);
    if (withText.length >= 3 && withText.length > bestSize) {{
      bestGroup = withText;
      bestSize = withText.length;
    }}
  }}

  if (!bestGroup) return JSON.stringify({{ items: [], hint: "No repeating pattern found. Try: eval --selector" }});

  const items = bestGroup.slice(0, _limit).map(el => {{
    const item = {{}};
    const heading = el.querySelector('h1,h2,h3,h4,h5,h6,[role=heading]');
    if (heading) item.title = heading.textContent.trim();

    const link = el.querySelector('a[href]');
    if (link) {{
      if (!item.title) item.title = link.textContent.trim();
      item.url = link.href;
    }}

    const price = el.querySelector('[class*=price],[class*=Price],[data-price]');
    if (price) item.price = price.textContent.trim();

    const img = el.querySelector('img[src]');
    if (img) item.image = img.src;

    const time = el.querySelector('time,[datetime]');
    if (time) item.date = time.getAttribute('datetime') || time.textContent.trim();

    if (Object.keys(item).length === 0) {{
      item.text = el.textContent.trim().substring(0, 200);
    }}

    return item;
  }});

  return JSON.stringify({{ items, count: bestGroup.length, pattern: bestGroup[0].tagName + '.' + [...bestGroup[0].classList].join('.') }});
}})()"#
    );

    let raw = crate::commands::eval::run_raw(client, &js).await?;

    // The JS returns a JSON string; parse it.
    let parsed: Value = match &raw {
        Value::String(s) => serde_json::from_str(s)?,
        other => other.clone(),
    };

    let items = parsed
        .get("items")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let count = parsed
        .get("count")
        .and_then(Value::as_u64)
        .unwrap_or(items.len() as u64) as usize;
    let pattern = parsed
        .get("pattern")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    // If there's a hint (no pattern found), propagate as error.
    if let Some(hint) = parsed.get("hint").and_then(Value::as_str) {
        if items.is_empty() {
            return Err(hint.into());
        }
    }

    Ok(ExtractResult {
        items,
        count,
        pattern,
    })
}

/// Format the extract result as human-readable text.
pub fn format_text(result: &ExtractResult) -> String {
    let mut out = format!(
        "Found {} items (pattern: {})\n",
        result.count, result.pattern
    );
    for (i, item) in result.items.iter().enumerate() {
        let mut parts: Vec<String> = Vec::new();
        if let Some(title) = item.get("title").and_then(Value::as_str) {
            parts.push(format!("Title: \"{title}\""));
        }
        if let Some(price) = item.get("price").and_then(Value::as_str) {
            parts.push(format!("Price: \"{price}\""));
        }
        if let Some(date) = item.get("date").and_then(Value::as_str) {
            parts.push(format!("Date: \"{date}\""));
        }
        if let Some(url) = item.get("url").and_then(Value::as_str) {
            parts.push(format!("URL: {url}"));
        }
        if let Some(image) = item.get("image").and_then(Value::as_str) {
            parts.push(format!("Image: {image}"));
        }
        if let Some(text) = item.get("text").and_then(Value::as_str) {
            parts.push(format!("Text: \"{text}\""));
        }
        out.push_str(&format!("{}. {}\n", i + 1, parts.join(" | ")));
    }
    out
}

/// Build the JSON output for the extract command.
pub fn to_json(result: &ExtractResult) -> Value {
    json!({
        "ok": true,
        "items": result.items,
        "count": result.count,
        "pattern": result.pattern,
    })
}
