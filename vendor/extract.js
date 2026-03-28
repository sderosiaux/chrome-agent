// aibrowsr extract algorithm
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
    const tagGroups = {};
    for (const [sig, els] of Object.entries(groups)) {
      const tag = sig.split('|')[0];
      const visible = els.filter(e => isVisible(e));
      if (!visible.length) continue;
      if (!tagGroups[tag]) tagGroups[tag] = { sig, els: [] };
      tagGroups[tag].els.push(...visible);
      if (visible.length > (tagGroups[tag].bestCount || 0)) {
        tagGroups[tag].sig = sig;
        tagGroups[tag].bestCount = visible.length;
      }
    }
    for (const { sig, els } of Object.values(tagGroups)) {
      if (els.length < 3 || groups[sig]?.length === els.length) continue;
      groups[sig + '|merged'] = els;
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

      const avgLinkRatio = rich.reduce((s, e) => s + linkTextRatio(e), 0) / rich.length;
      if (avgLinkRatio > 0.85) score *= 0.2;
      else if (avgLinkRatio > 0.7) score *= 0.5;

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

  const items = best.elements.slice(0, _limit).map(el => {
    const item = {};
    const heading = el.querySelector('h1,h2,h3,h4,h5,h6,[role=heading]');
    if (heading) item.title = heading.textContent.trim().replace(/\s+/g, ' ');

    // Prefer the link inside a heading or row header (primary link), else longest-text link
    const headingLink = el.querySelector('h1 a[href],h2 a[href],h3 a[href],h4 a[href],h5 a[href],h6 a[href],th a[href]');
    const links = [...el.querySelectorAll('a[href]')].filter(a => a.textContent.trim().length > 0);
    const longestLink = links.sort((a, b) => b.textContent.trim().length - a.textContent.trim().length)[0];
    const link = headingLink || longestLink;
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
      // Skip hidden children (display:none, visibility:hidden, hidden attr, aria-hidden)
      const style = (child.getAttribute('style') || '').toLowerCase();
      if (style.includes('display:none') || style.includes('display: none') ||
          style.includes('visibility:hidden') || style.includes('visibility: hidden')) continue;
      if (child.hidden || child.getAttribute('aria-hidden') === 'true') continue;
      const txt = child.textContent.trim();
      if (txt && txt.length > 0 && txt.length < 200) {
        if (item.title && txt === item.title) continue;
        if (item.price && txt === item.price) continue;
        fields.push(txt);
      }
    }
    if (fields.length > 0) {
      item.fields = fields.slice(0, 8);
    }

    if (Object.keys(item).length === 0) {
      item.text = el.textContent.trim().substring(0, 200);
    }

    return item;
  });

  const patternParts = best.sig.split('|');
  const patternLabel = patternParts[0] + (patternParts[1] ? '.' + patternParts[1] : '');
  return JSON.stringify({ items, count: best.elements.length, pattern: patternLabel });
}

if (typeof module !== 'undefined') module.exports = extract;
