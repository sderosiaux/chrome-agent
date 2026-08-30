// Detects repeating data records using MDR/DEPTA-inspired heuristics. `_scope` is a document
// or a scoped element; returns a JSON string, { items, count, pattern } or { items: [], hint }.

function extract(_scope, _limit) {
  function childSignature(el) {
    // A class carrying a digit, or longer than 30 chars, is per-instance noise not a type name.
    const classes = [...el.classList]
      .filter(c => !/\d/.test(c) && c.length < 30)
      .sort().join('.');
    // Child tags stay out: a featured card with extra badges is still the same record type.
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

  function isVisible(el) {
    return !el.closest('[hidden],[aria-hidden="true"]');
  }

  // Disqualifiers shared by both passes: the group sits in chrome, or is a strip of short links.
  function penalise(score, parent, rich) {
    const parentTag = parent ? parent.tagName : '';
    if (parentTag === 'BODY' || parentTag === 'HTML') score *= 0.5;
    if (parentTag === 'NAV' || (parent && parent.closest('nav,header,footer'))) score *= 0.3;

    // Link density means navigation only when the links are short labels. A listing row
    // whose headline is a link is legitimately link-heavy, hence the longest-link guard.
    const avgLinkRatio = rich.reduce((s, e) => s + linkTextRatio(e), 0) / rich.length;
    const avgLongestLink = rich.reduce((s, e) => s + longestLinkText(e), 0) / rich.length;
    if (avgLongestLink < HEADLINE_LINK_CHARS) {
      if (avgLinkRatio > 0.85) score *= 0.2;
      else if (avgLinkRatio > 0.7) score *= 0.5;
    }
    return score;
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
      // x2 for "this class name says data", but the nav rules still apply: a <nav> whose <li>s
      // carry "nav-item" matches DATA_CLASS_RE and otherwise outscores the real product list.
      const score = penalise(avgRich * rich.length * 2.0, rich[0].parentElement, rich);
      candidates.push({ parent: rich[0].parentElement, elements: rich, sig, score });
    }
  }

  // Phase 2: Structural pass
  const allParents = _scope.querySelectorAll('*');

  for (const parent of allParents) {
    if (!isVisible(parent)) continue;
    const kids = [...parent.children];
    if (kids.length < 3) continue;

    const groups = {};
    for (const kid of kids) {
      const sig = childSignature(kid);
      if (!groups[sig]) groups[sig] = [];
      groups[sig].push(kid);
    }
    // Merge only variants sharing a class ("item" vs "item featured"). Same tag with no class in
    // common is a different record type (HN story rows vs subtext rows) and must stay apart.
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
      let score = penalise(avgRich * rich.length, parent, rich);

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

  // Two lists can both look like data. The higher-scoring one wins; a runner-up within 60% of it
  // covering different nodes is named, so the caller knows a choice was made.
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

  function isSrOnly(el) {
    const cl = el.className || '';
    return /sr-only|visually-hidden|screen-reader/i.test(cl);
  }

  function cleanText(txt) {
    // Strips CSS rules that leak into textContent from a <style> descendant.
    let cleaned = txt.replace(/\.[a-zA-Z_-]+\{[^}]*\}/g, '').trim();
    cleaned = cleaned.replace(/\s+/g, ' ');
    return cleaned;
  }

  // Drops spacer rows: too little text, or no element children.
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

    // Title link preference: inside a heading or th, then a "title"-classed link, then the longest.
    const headingLink = el.querySelector('h1 a[href],h2 a[href],h3 a[href],h4 a[href],h5 a[href],h6 a[href],th a[href]');
    const titleClassLink = el.querySelector('[class*=title] > a[href],[class*=Title] > a[href],.titleline > a[href]');
    const links = [...el.querySelectorAll('a[href]')].filter(a => {
      const t = a.textContent.trim();
      if (t.length === 0) return false;
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
      // No computed style here, so display/visibility are matched textually on the attribute.
      const style = (child.getAttribute('style') || '').toLowerCase();
      if (style.includes('display:none') || style.includes('display: none') ||
          style.includes('visibility:hidden') || style.includes('visibility: hidden')) continue;
      if (child.hidden || child.getAttribute('aria-hidden') === 'true') continue;
      if (isSrOnly(child)) continue;
      if (child.tagName === 'SCRIPT' || child.tagName === 'STYLE') continue;
      let txt = cleanText(child.textContent);
      if (txt && txt.length > 2 && txt.length < 200) {
        if (item.title && txt === item.title) continue;
        if (item.price && txt === item.price) continue;
        // Single-word action labels are chrome, not fields.
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
