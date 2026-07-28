// chrome-agent extract algorithm
// Detects repeating data records in a page using MDR/DEPTA-inspired heuristics.
// Called with: extract(_scope, _limit) where _scope is document or a scoped element.
// Returns JSON string: { items, count, pattern } or { items: [], hint: "..." }

function extract(_scope, _limit) {
  function childSignature(el) {
    // Filter out classes that look unique/dynamic (contain digits, hashes, or UUIDs)
    const classes = [...el.classList]
      .filter(c => !/\d/.test(c) && c.length < 30)
      .sort().join('.');
    // Don't include childTags in signature — items with same tag+class but different
    // internal structure (e.g. featured card with extra badges) should still group together
    return el.tagName + '|' + classes;
  }

  function richness(el) {
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
  }

  function heterogeneity(el) {
    const tags = new Set([...el.children].map(c => c.tagName));
    return tags.size;
  }

  function linkTextRatio(el) {
    const totalText = el.textContent.trim().length;
    if (totalText === 0) return 1;
    const linkText = [...el.querySelectorAll('a')].reduce((s, a) => s + a.textContent.trim().length, 0);
    return linkText / totalText;
  }

  // Shortest link text we treat as a headline rather than a nav label.
  const HEADLINE_LINK_CHARS = 25;

  function longestLinkText(el) {
    let max = 0;
    for (const a of el.querySelectorAll('a[href]')) {
      const len = a.textContent.trim().length;
      if (len > max) max = len;
    }
    return max;
  }

  function subtreeDepth(el) {
    if (!el.children.length) return 1;
    let max = 0;
    for (const child of el.children) {
      const d = subtreeDepth(child);
      if (d > max) max = d;
    }
    return 1 + max;
  }

  const DATA_CLASS_RE = /item|card|product|result|row|entry|record|listing|post|article|story|repo|thread|comment/i;

  // Skip elements that are hidden or inside a hidden ancestor
  function isVisible(el) {
    return !el.closest('[hidden],[aria-hidden="true"]');
  }

  // Phase 1: Semantic fast-pass
  const candidates = [];
  const semanticHits = [..._scope.querySelectorAll('*')].filter(el =>
    isVisible(el) && DATA_CLASS_RE.test(el.className) && el.children.length >= 1 && el.textContent.trim().length > 10
  );

  if (semanticHits.length >= 3) {
    const semGroups = {};
    for (const el of semanticHits) {
      const sig = childSignature(el);
      if (!semGroups[sig]) semGroups[sig] = [];
      semGroups[sig].push(el);
    }
    for (const [sig, els] of Object.entries(semGroups)) {
      if (els.length < 3) continue;
      const rich = els.filter(e => richness(e) >= 1);
      if (rich.length < 3) continue;
      const avgRich = rich.reduce((s, e) => s + richness(e), 0) / rich.length;
      candidates.push({ parent: rich[0].parentElement, elements: rich, sig, score: avgRich * rich.length * 2.0 });
    }
  }

  // Phase 2: Structural pass
  const allParents = _scope.querySelectorAll('*');

  for (const parent of allParents) {
    if (!isVisible(parent)) continue;
    const kids = [...parent.children];
    if (kids.length < 3) continue;

    // Two-pass grouping: first by tagName only, then merge groups
    // whose signatures differ only by modifier classes (e.g. "featured")
    const groups = {};
    for (const kid of kids) {
      const sig = childSignature(kid);
      if (!groups[sig]) groups[sig] = [];
      groups[sig].push(kid);
    }
    // Merge groups with same tagName — a "featured" variant should join the base group
    // but skip hidden elements during merge
    // A modifier variant shares the base class and adds to it ("item" vs "item featured").
    // Rows that merely share a tag with no class in common are different record types
    // (HN's story rows vs its subtext rows) and must stay apart, or the merged group wins
    // on sheer count and mixes two kinds of record into one list.
    const classesOf = (sig) => new Set(sig.split('|')[1].split('.').filter(Boolean));
    const tagGroups = {};
    for (const [sig, els] of Object.entries(groups)) {
      const tag = sig.split('|')[0];
      const visible = els.filter(e => isVisible(e));
      if (!visible.length) continue;
      if (!tagGroups[tag]) tagGroups[tag] = [];
      tagGroups[tag].push({ sig, els: visible });
    }
    for (const variants of Object.values(tagGroups)) {
      if (variants.length < 2) continue;
      const base = variants.reduce((a, b) => (b.els.length > a.els.length ? b : a));
      const baseClasses = classesOf(base.sig);
      if (!baseClasses.size) continue;
      const merged = [];
      for (const v of variants) {
        const shares = [...classesOf(v.sig)].some((c) => baseClasses.has(c));
        if (shares) merged.push(...v.els);
      }
      if (merged.length < 3 || merged.length === base.els.length) continue;
      groups[base.sig + '|merged'] = merged;
    }

    for (const [sig, els] of Object.entries(groups)) {
      if (els.length < 3) continue;
      const rich = els.filter(e => richness(e) >= 2);
      if (rich.length < 3) continue;

      const parentTag = parent.tagName;
      const elTag = rich[0].tagName;

      const avgRich = rich.reduce((s, e) => s + richness(e), 0) / rich.length;
      let score = avgRich * rich.length;

      if (parentTag === 'BODY' || parentTag === 'HTML') score *= 0.5;
      if (parentTag === 'NAV' || parent.closest('nav,header,footer')) score *= 0.3;

      // Link density flags navigation, where every link is a short label. A listing row
      // whose headline is itself a link is legitimately link-heavy, so only penalise
      // groups where no record carries a link with real text in it.
      const avgLinkRatio = rich.reduce((s, e) => s + linkTextRatio(e), 0) / rich.length;
      const avgLongestLink = rich.reduce((s, e) => s + longestLinkText(e), 0) / rich.length;
      if (avgLongestLink < HEADLINE_LINK_CHARS) {
        if (avgLinkRatio > 0.85) score *= 0.2;
        else if (avgLinkRatio > 0.7) score *= 0.5;
      }

      const avgHetero = rich.reduce((s, e) => s + heterogeneity(e), 0) / rich.length;
      if (avgHetero >= 3) score *= 1.3;
      else if (avgHetero >= 2) score *= 1.1;

      const avgDepth = rich.reduce((s, e) => s + subtreeDepth(e), 0) / rich.length;
      if (avgDepth >= 3) score *= 1.2;

      if (['ARTICLE','LI','TR','SECTION'].includes(elTag)) score *= 1.2;
      if (rich.some(e => DATA_CLASS_RE.test(e.className))) score *= 1.3;

      candidates.push({ parent, elements: rich, sig, score });
    }
  }

  if (candidates.length === 0) {
    const rows = [..._scope.querySelectorAll('tr')].filter(r => r.querySelectorAll('td').length >= 2);
    if (rows.length >= 3) {
      candidates.push({ parent: rows[0].parentElement, elements: rows, sig: 'TR|table', score: rows.length * 3 });
    }
  }

  if (candidates.length === 0) return JSON.stringify({ items: [], hint: "No repeating pattern found. Try: extract --selector or eval --selector" });

  candidates.sort((a, b) => b.score - a.score);
  const best = candidates[0];

  // A page can hold two lists that both look like data: products and posts, results and
  // related items. We return the higher-scoring one, and when the runner-up is close and
  // covers different nodes, the caller has to be told — otherwise it silently receives one
  // of two plausible answers with no way to know a choice was made.
  function selectorFor(el) {
    if (!el || !el.tagName) return null;
    if (el.id) return '#' + el.id;
    const cls = [...(el.classList || [])].filter(c => !/\d/.test(c));
    if (cls.length) return el.tagName.toLowerCase() + '.' + cls[0];
    return el.tagName.toLowerCase();
  }

  const bestNodes = new Set(best.elements);
  const alternatives = [];
  for (const c of candidates.slice(1)) {
    if (c.score < best.score * 0.6) break;
    const overlaps = c.elements.some(e => bestNodes.has(e));
    if (overlaps) continue;
    const sel = selectorFor(c.parent);
    if (sel && !alternatives.some(a => a.selector === sel)) {
      alternatives.push({ selector: sel, count: c.elements.length });
    }
    if (alternatives.length >= 3) break;
  }

  // Helper: check if element or ancestor is sr-only/visually-hidden
  function isSrOnly(el) {
    const cl = el.className || '';
    return /sr-only|visually-hidden|screen-reader/i.test(cl);
  }

  // Helper: clean text by removing CSS leaks and UI chrome noise
  function cleanText(txt) {
    // Remove inline CSS that leaks from style elements
    let cleaned = txt.replace(/\.[a-zA-Z_-]+\{[^}]*\}/g, '').trim();
    // Collapse whitespace
    cleaned = cleaned.replace(/\s+/g, ' ');
    return cleaned;
  }

  // Filter to elements with actual extractable content (skip spacer rows)
  const meaningfulElements = best.elements.filter(el => {
    const text = el.textContent.trim();
    if (text.length < 3) return false;
    if (el.children.length === 0) return false;
    return true;
  });

  const items = meaningfulElements.slice(0, _limit).map(el => {
    const item = {};
    const heading = el.querySelector('h1,h2,h3,h4,h5,h6,[role=heading]');
    if (heading) item.title = heading.textContent.trim().replace(/\s+/g, ' ');

    // Prefer link inside heading/th, then link with class containing "title",
    // then first link that isn't a short domain-only link, then longest link
    const headingLink = el.querySelector('h1 a[href],h2 a[href],h3 a[href],h4 a[href],h5 a[href],h6 a[href],th a[href]');
    const titleClassLink = el.querySelector('[class*=title] > a[href],[class*=Title] > a[href],.titleline > a[href]');
    const links = [...el.querySelectorAll('a[href]')].filter(a => {
      const t = a.textContent.trim();
      if (t.length === 0) return false;
      // Skip sr-only links
      if (isSrOnly(a) || a.closest('[aria-hidden="true"]')) return false;
      return true;
    });
    const longestLink = links.sort((a, b) => b.textContent.trim().length - a.textContent.trim().length)[0];
    const link = headingLink || titleClassLink || longestLink;
    if (link) {
      if (!item.title) item.title = link.textContent.trim().replace(/\s+/g, ' ');
      item.url = link.href;
    }

    const price = el.querySelector('[class*=price],[class*=Price],[data-price]');
    if (price) {
      const priceText = price.textContent.trim();
      item.price = priceText || price.getAttribute('data-price') || '';
    }

    const img = el.querySelector('img[src]');
    if (img) item.image = img.src;

    const time = el.querySelector('time,[datetime]');
    if (time) item.date = time.getAttribute('datetime') || time.textContent.trim();

    const fields = [];
    for (const child of el.children) {
      // Skip hidden/sr-only children
      const style = (child.getAttribute('style') || '').toLowerCase();
      if (style.includes('display:none') || style.includes('display: none') ||
          style.includes('visibility:hidden') || style.includes('visibility: hidden')) continue;
      if (child.hidden || child.getAttribute('aria-hidden') === 'true') continue;
      if (isSrOnly(child)) continue;
      // Skip script/style tags
      if (child.tagName === 'SCRIPT' || child.tagName === 'STYLE') continue;
      let txt = cleanText(child.textContent);
      if (txt && txt.length > 2 && txt.length < 200) {
        if (item.title && txt === item.title) continue;
        if (item.price && txt === item.price) continue;
        // Skip UI chrome noise (single-word button labels, etc.)
        if (/^(Star|Sponsor|Share|Like|Save|Follow|Built by|Unstar)$/i.test(txt)) continue;
        fields.push(txt);
      }
    }
    if (fields.length > 0) {
      item.fields = fields.slice(0, 8);
    }

    if (Object.keys(item).length === 0) {
      item.text = cleanText(el.textContent).substring(0, 200);
    }

    return item;
  });

  // Only count items that produced at least one useful field
  const nonEmpty = items.filter(i => i.title || i.url || i.text || i.fields);

  const patternParts = best.sig.split('|');
  let patternClasses = patternParts[1] || '';
  if (patternClasses.length > 40) patternClasses = patternClasses.substring(0, 40) + '...';
  const patternLabel = patternParts[0] + (patternClasses ? '.' + patternClasses : '');
  const out = { items: nonEmpty, count: meaningfulElements.length, pattern: patternLabel };
  if (alternatives.length) {
    out.alternatives = alternatives;
    out.hint = 'More than one repeating pattern on this page. Scope with --selector ' +
      alternatives.map(a => '"' + a.selector + '"').join(' or ') + ' to pick a different one.';
  }
  return JSON.stringify(out);
}

if (typeof module !== 'undefined') module.exports = extract;
