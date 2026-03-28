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
) -> Result<ExtractResult, crate::BoxError> {
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

  // Strategy: find parent containers whose direct children form repeating groups.
  // A "good" repeating group has >=3 similar children, each with rich content
  // (multiple child elements, text+link, etc). This finds semantic rows/cards,
  // not individual leaf elements like all <a> tags.

  function childSignature(el) {{
    const classes = [...el.classList]
      .filter(c => !/\d/.test(c) && c.length < 30)
      .sort().join('.');
    return el.tagName + '|' + classes;
  }}

  function richness(el) {{
    // How "rich" is this element? Based on MDR/DEPTA heuristics:
    // - More child elements = richer structure
    // - More text = more content
    // - Mixed content types = heterogeneous data record (not just links)
    const childCount = el.children.length;
    const textLen = el.textContent.trim().length;
    const hasLink = !!el.querySelector('a[href]');
    const hasImg = !!el.querySelector('img[src]');
    let score = 0;
    if (childCount >= 2) score += 2;
    if (childCount >= 4) score += 1;
    if (textLen > 20) score += 1;
    if (textLen > 80) score += 1;
    if (hasLink) score += 1;
    if (hasImg) score += 1;
    return score;
  }}

  // Content heterogeneity: how many distinct tag types in direct children?
  // Data records have mixed content (text+img+link+span); nav has just <a>.
  function heterogeneity(el) {{
    const tags = new Set([...el.children].map(c => c.tagName));
    return tags.size;
  }}

  // Text-to-link ratio: what fraction of text is inside <a> tags?
  // Nav regions have >0.8; data regions have <0.5
  function linkTextRatio(el) {{
    const totalText = el.textContent.trim().length;
    if (totalText === 0) return 1;
    const linkText = [...el.querySelectorAll('a')].reduce((s, a) => s + a.textContent.trim().length, 0);
    return linkText / totalText;
  }}

  // Subtree depth: deeper elements = richer data records
  function subtreeDepth(el) {{
    if (!el.children.length) return 1;
    let max = 0;
    for (const child of el.children) {{
      const d = subtreeDepth(child);
      if (d > max) max = d;
    }}
    return 1 + max;
  }}

  // Semantic class match: classes containing common data record keywords
  const DATA_CLASS_RE = /item|card|product|result|row|entry|record|listing|post|article|story|repo|thread|comment/i;

  function isVisible(el) {{
    return !el.closest('[hidden],[aria-hidden="true"]');
  }}

  const candidates = [];
  const semanticHits = [..._scope.querySelectorAll('*')].filter(el =>
    isVisible(el) && DATA_CLASS_RE.test(el.className) && el.children.length >= 1 && el.textContent.trim().length > 10
  );

  if (semanticHits.length >= 3) {{
    // Group semantic hits by their className signature
    const semGroups = {{}};
    for (const el of semanticHits) {{
      const sig = childSignature(el);
      if (!semGroups[sig]) semGroups[sig] = [];
      semGroups[sig].push(el);
    }}
    for (const [sig, els] of Object.entries(semGroups)) {{
      if (els.length < 3) continue;
      const rich = els.filter(e => richness(e) >= 1);
      if (rich.length < 3) continue;
      const avgRich = rich.reduce((s, e) => s + richness(e), 0) / rich.length;
      // Semantic class matches get a big bonus
      candidates.push({{ parent: rich[0].parentElement, elements: rich, sig, score: avgRich * rich.length * 2.0 }});
    }}
  }}

  // Phase 2: Structural pass — sibling similarity (MDR algorithm inspired)
  const allParents = _scope.querySelectorAll('*');

  for (const parent of allParents) {{
    if (!isVisible(parent)) continue;
    const kids = [...parent.children];
    if (kids.length < 3) continue;

    const groups = {{}};
    for (const kid of kids) {{
      const sig = childSignature(kid);
      if (!groups[sig]) groups[sig] = [];
      groups[sig].push(kid);
    }}
    // Merge groups with same tagName (handles modifier classes like "featured")
    const tagGroups = {{}};
    for (const [sig, els] of Object.entries(groups)) {{
      const tag = sig.split('|')[0];
      const visible = els.filter(e => isVisible(e));
      if (!visible.length) continue;
      if (!tagGroups[tag]) tagGroups[tag] = {{ sig, els: [], bestCount: 0 }};
      tagGroups[tag].els.push(...visible);
      if (visible.length > tagGroups[tag].bestCount) {{
        tagGroups[tag].sig = sig;
        tagGroups[tag].bestCount = visible.length;
      }}
    }}
    for (const {{ sig, els }} of Object.values(tagGroups)) {{
      if (els.length < 3 || groups[sig]?.length === els.length) continue;
      groups[sig + '|merged'] = els;
    }}

    for (const [sig, els] of Object.entries(groups)) {{
      if (els.length < 3) continue;
      const rich = els.filter(e => richness(e) >= 2);
      if (rich.length < 3) continue;

      const parentTag = parent.tagName;
      const elTag = rich[0].tagName;

      // --- Composite scoring inspired by MDR/DEPTA ---
      // Base: richness × count
      const avgRich = rich.reduce((s, e) => s + richness(e), 0) / rich.length;
      let score = avgRich * rich.length;

      // Penalty: overly broad parent (BODY/HTML)
      if (parentTag === 'BODY' || parentTag === 'HTML') score *= 0.5;

      // Penalty: nav/header/footer region (text-to-link ratio based)
      if (parentTag === 'NAV' || parent.closest('nav,header,footer')) score *= 0.3;

      // Penalty: high link-text ratio = likely navigation, not data
      const avgLinkRatio = rich.reduce((s, e) => s + linkTextRatio(e), 0) / rich.length;
      if (avgLinkRatio > 0.85) score *= 0.2;  // Almost all text is links = nav
      else if (avgLinkRatio > 0.7) score *= 0.5;

      // Bonus: content heterogeneity (mixed tag types = real data record)
      const avgHetero = rich.reduce((s, e) => s + heterogeneity(e), 0) / rich.length;
      if (avgHetero >= 3) score *= 1.3;
      else if (avgHetero >= 2) score *= 1.1;

      // Bonus: subtree depth (deeper = richer structure)
      const avgDepth = rich.reduce((s, e) => s + subtreeDepth(e), 0) / rich.length;
      if (avgDepth >= 3) score *= 1.2;

      // Bonus: semantic tag names for container elements
      if (['ARTICLE','LI','TR','SECTION'].includes(elTag)) score *= 1.2;

      // Bonus: semantic class name match on the elements themselves
      if (rich.some(e => DATA_CLASS_RE.test(e.className))) score *= 1.3;

      candidates.push({{ parent, elements: rich, sig, score }});
    }}
  }}

  if (candidates.length === 0) {{
    // Fallback: try table rows
    const rows = [..._scope.querySelectorAll('tr')].filter(r => r.querySelectorAll('td').length >= 2);
    if (rows.length >= 3) {{
      candidates.push({{ parent: rows[0].parentElement, elements: rows, sig: 'TR|table', score: rows.length * 3 }});
    }}
  }}

  if (candidates.length === 0) return JSON.stringify({{ items: [], hint: "No repeating pattern found. Try: extract --selector or eval --selector" }});

  candidates.sort((a, b) => b.score - a.score);
  const best = candidates[0];

  function isSrOnly(el) {{ return /sr-only|visually-hidden|screen-reader/i.test(el.className || ''); }}
  function cleanText(txt) {{ return txt.replace(/\.[a-zA-Z_-]+\{{[^}}]*\}}/g, '').trim().replace(/\s+/g, ' '); }}

  const meaningful = best.elements.filter(el => {{
    const text = el.textContent.trim();
    return text.length >= 3 && el.children.length >= 1;
  }});

  const items = meaningful.slice(0, _limit).map(el => {{
    const item = {{}};
    const heading = el.querySelector('h1,h2,h3,h4,h5,h6,[role=heading]');
    if (heading) item.title = heading.textContent.trim().replace(/\s+/g, ' ');

    const headingLink = el.querySelector('h1 a[href],h2 a[href],h3 a[href],h4 a[href],h5 a[href],h6 a[href],th a[href]');
    const titleClassLink = el.querySelector('[class*=title] > a[href],[class*=Title] > a[href],.titleline > a[href]');
    const links = [...el.querySelectorAll('a[href]')].filter(a => {{
      const t = a.textContent.trim();
      return t.length > 0 && !isSrOnly(a) && !a.closest('[aria-hidden="true"]');
    }});
    const longestLink = links.sort((a, b) => b.textContent.trim().length - a.textContent.trim().length)[0];
    const link = headingLink || titleClassLink || longestLink;
    if (link) {{
      if (!item.title) item.title = link.textContent.trim().replace(/\s+/g, ' ');
      item.url = link.href;
    }}

    const price = el.querySelector('[class*=price],[class*=Price],[data-price]');
    if (price) {{ item.price = price.textContent.trim() || price.getAttribute('data-price') || ''; }}

    const img = el.querySelector('img[src]');
    if (img) item.image = img.src;

    const time = el.querySelector('time,[datetime]');
    if (time) item.date = time.getAttribute('datetime') || time.textContent.trim();

    const fields = [];
    for (const child of el.children) {{
      const cStyle = (child.getAttribute('style') || '').toLowerCase();
      if (cStyle.includes('display:none') || cStyle.includes('display: none') ||
          cStyle.includes('visibility:hidden') || cStyle.includes('visibility: hidden')) continue;
      if (child.hidden || child.getAttribute('aria-hidden') === 'true') continue;
      if (isSrOnly(child)) continue;
      if (child.tagName === 'SCRIPT' || child.tagName === 'STYLE') continue;
      const txt = cleanText(child.textContent);
      if (txt && txt.length > 2 && txt.length < 200) {{
        if (item.title && txt === item.title) continue;
        if (item.price && txt === item.price) continue;
        if (/^(Star|Sponsor|Share|Like|Save|Follow|Built by|Unstar)$/i.test(txt)) continue;
        fields.push(txt);
      }}
    }}
    if (fields.length > 0) {{ item.fields = fields.slice(0, 8); }}

    if (Object.keys(item).length === 0) {{ item.text = cleanText(el.textContent).substring(0, 200); }}

    return item;
  }});

  const patternParts = best.sig.split('|');
  let patternClasses = patternParts[1] || '';
  if (patternClasses.length > 40) patternClasses = patternClasses.substring(0, 40) + '...';
  const patternLabel = patternParts[0] + (patternClasses ? '.' + patternClasses : '');
  const nonEmpty = items.filter(i => i.title || i.url || i.text || i.fields);
  return JSON.stringify({{ items: nonEmpty, count: meaningful.length, pattern: patternLabel }});
}})()"#
    );

    let raw = crate::commands::eval::run_raw(client, &js).await?;

    // The JS returns a JSON string; parse it.
    let parsed: Value = match raw {
        Value::String(s) => serde_json::from_str(&s)?,
        other => other,
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
    if let Some(hint) = parsed.get("hint").and_then(Value::as_str)
        && items.is_empty() {
            return Err(hint.into());
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
        if let Some(fields) = item.get("fields").and_then(Value::as_array) {
            let texts: Vec<&str> = fields.iter().filter_map(Value::as_str).collect();
            if !texts.is_empty() {
                parts.push(format!("Fields: [{}]", texts.join(", ")));
            }
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
