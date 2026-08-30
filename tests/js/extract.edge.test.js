const { describe, it } = require('node:test');
const assert = require('node:assert/strict');

const { extractFromHTML, extractFromHTMLWithSelector, loadFixture } = require('./helpers.js');

describe('edge: empty page', () => {
  it('returns hint for completely empty body', () => {
    const r = extractFromHTML('<html><body></body></html>');
    assert.ok(r.hint);
    assert.deepEqual(r.items, []);
  });

  it('returns hint for empty string', () => {
    const r = extractFromHTML('');
    assert.ok(r.hint);
    assert.deepEqual(r.items, []);
  });
});

describe('edge: single item only', () => {
  it('returns hint when only 1 card exists', () => {
    const html = `<html><body>
      <div class="card"><h2><a href="/x">Title</a></h2><p>Description here is long enough to count</p></div>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.ok(r.hint, 'single item should produce hint');
    assert.deepEqual(r.items, []);
  });
});

describe('edge: 2 items (below threshold of 3)', () => {
  it('returns hint when only 2 similar items exist', () => {
    const html = `<html><body>
      <div class="list">
        <div class="card"><h3><a href="/a">Item A</a></h3><p>Some description text here</p></div>
        <div class="card"><h3><a href="/b">Item B</a></h3><p>Another description text here</p></div>
      </div>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.ok(r.hint, '2 items is below the threshold of 3');
    assert.deepEqual(r.items, []);
  });
});

describe('edge: deeply nested structures', () => {
  it('finds items even when wrapped in many containers', () => {
    const items = Array.from({ length: 4 }, (_, i) => `
      <div class="card">
        <div class="inner"><div class="wrap">
          <h3><a href="/item/${i}">Nested Item ${i}</a></h3>
          <p>This is a description that is long enough for richness scoring to count it as meaningful content</p>
        </div></div>
      </div>`).join('');
    const html = `<html><body>
      <div class="outer"><div class="mid"><div class="inner-list">${items}</div></div></div>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.equal(r.pattern, 'DIV.card');
    assert.ok(r.items[0].title.includes('Nested Item'));
  });
});

describe('edge: all-links page', () => {
  it('handles page that is only links (sitemap style)', () => {
    const links = Array.from({ length: 20 }, (_, i) =>
      `<li><a href="/p/${i}">Page ${i}</a></li>`).join('');
    const html = `<html><body><ul>${links}</ul></body></html>`;
    const r = extractFromHTML(html);
    // Was `assert.ok(r.items !== undefined)`, which a stub returning {items: []} also passes.
    // Link ratio 1.0 sinks every candidate, so the answer is the no-pattern hint.
    assert.deepEqual(r, { items: [], hint: 'No repeating pattern found. Try: extract --selector or eval --selector' });
  });
});

describe('edge: page with only images', () => {
  it('handles page with only images, no text', () => {
    const imgs = Array.from({ length: 5 }, (_, i) =>
      `<div><img src="/img/${i}.jpg"></div>`).join('');
    const html = `<html><body><div class="gallery">${imgs}</div></body></html>`;
    const r = extractFromHTML(html);
    // No text and no links fails richness >= 2, so nothing groups and the hint is the answer.
    assert.deepEqual(r, { items: [], hint: 'No repeating pattern found. Try: extract --selector or eval --selector' });
  });
});

describe('edge: unicode content', () => {
  it('handles unicode titles and text', () => {
    const items = [
      { title: '开源软件', desc: '这是一个开源项目的详细描述，为了测试多语言支持' },
      { title: 'プログラミング', desc: 'プログラミングに関する詳細な説明と例を示します' },
      { title: 'Программирование', desc: 'Подробное описание проекта для тестирования' },
      { title: 'Café Émoji ☕', desc: 'Un projet open source avec des caractères spéciaux pour tester' },
    ];
    const cards = items.map(i => `
      <div class="item"><h3><a href="/x">${i.title}</a></h3><p>${i.desc}</p></div>`).join('');
    const html = `<html><body><div class="list">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(r.items.map(i => i.title), items.map(i => i.title));
  });
});

describe('edge: huge page with 100+ items', () => {
  it('extracts from a page with 120 items', () => {
    const cards = Array.from({ length: 120 }, (_, i) => `
      <div class="product-card">
        <h3><a href="/p/${i}">Product #${i}</a></h3>
        <p>Description for product number ${i} with some extra text to boost richness.</p>
        <span class="price">$${(i * 10 + 9.99).toFixed(2)}</span>
      </div>`).join('');
    const html = `<html><body><div class="grid">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 120);
    assert.equal(r.items.length, 20, 'the default limit is 20');
  });
});

describe('edge: whitespace-heavy content', () => {
  it('trims whitespace from titles and fields', () => {
    const cards = Array.from({ length: 4 }, (_, i) => `
      <div class="entry">
        <h3>   <a href="/w/${i}">   Whitespace Title ${i}   </a>   </h3>
        <p>

          Lots   of   spaces   and   newlines   in   this   description   text

        </p>
      </div>`).join('');
    const html = `<html><body><div>${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(
      r.items.map(i => i.title),
      ['Whitespace Title 0', 'Whitespace Title 1', 'Whitespace Title 2', 'Whitespace Title 3'],
    );
  });
});

describe('limit parameter', () => {
  it('limit=2 caps items but preserves full count', () => {
    const cards = Array.from({ length: 6 }, (_, i) => `
      <div class="card">
        <h3><a href="/c/${i}">Card ${i}</a></h3>
        <p>Card description long enough for richness scoring to kick in properly</p>
      </div>`).join('');
    const html = `<html><body><div class="list">${cards}</div></body></html>`;
    const r = extractFromHTML(html, 2);
    assert.equal(r.items.length, 2, 'items should be capped at limit');
    assert.equal(r.count, 6, 'count should reflect all matched elements');
  });

  it('limit=1 returns exactly one item', () => {
    const html = loadFixture('extract_cards.html');
    const r = extractFromHTML(html, 1);
    assert.equal(r.items.length, 1);
    assert.equal(r.count, 4, 'count should be 4 (all cards)');
  });

  it('limit larger than item count returns all items', () => {
    const html = loadFixture('extract_cards.html');
    const r = extractFromHTML(html, 100);
    assert.equal(r.items.length, 4);
    assert.equal(r.count, 4);
  });
});

describe('selector scoping (extractFromHTMLWithSelector)', () => {
  it('scopes extraction to a specific container', () => {
    const html = `<html><body>
      <div id="sidebar">
        <div class="item"><h3><a href="/s/1">Sidebar 1</a></h3><p>Sidebar content for item one</p></div>
        <div class="item"><h3><a href="/s/2">Sidebar 2</a></h3><p>Sidebar content for item two</p></div>
        <div class="item"><h3><a href="/s/3">Sidebar 3</a></h3><p>Sidebar content for item three</p></div>
      </div>
      <div id="main">
        <div class="item"><h3><a href="/m/1">Main 1</a></h3><p>Main content for item one</p></div>
        <div class="item"><h3><a href="/m/2">Main 2</a></h3><p>Main content for item two</p></div>
        <div class="item"><h3><a href="/m/3">Main 3</a></h3><p>Main content for item three</p></div>
        <div class="item"><h3><a href="/m/4">Main 4</a></h3><p>Main content for item four</p></div>
      </div>
    </body></html>`;
    const r = extractFromHTMLWithSelector(html, '#main');
    assert.equal(r.count, 4);
    assert.deepEqual(r.items.map(i => i.title), ['Main 1', 'Main 2', 'Main 3', 'Main 4']);
  });

  it('returns hint when selector not found', () => {
    const html = '<html><body><p>Hello</p></body></html>';
    const r = extractFromHTMLWithSelector(html, '#nonexistent');
    assert.deepEqual(r.items, []);
    assert.ok(r.hint);
    assert.match(r.hint, /not found/i);
  });

  it('scoping a fixture to a sub-element', () => {
    const html = loadFixture('extract_ecommerce.html');
    const r = extractFromHTMLWithSelector(html, '.product-grid');
    assert.equal(r.count, 4);
  });
});

describe('pattern string format', () => {
  it('pattern has TAG.class format', () => {
    const html = loadFixture('extract_semantic_classes.html');
    const r = extractFromHTML(html);
    assert.equal(r.pattern, 'DIV.repo-card');
  });

  it('pattern class comes from element classList', () => {
    const html = loadFixture('extract_link_heavy_nav.html');
    const r = extractFromHTML(html);
    assert.equal(r.pattern, 'DIV.job-listing');
  });

  // The 40-char truncation only shows on a class list long enough to cross it, which no
  // realistic fixture has; without this the cap can be deleted with the suite green.
  it('truncates the class part of the pattern at 40 chars', () => {
    const cls = 'alpha beta delta epsilon gamma record zeta';
    const cards = Array.from({ length: 4 }, (_, i) =>
      `<div class="${cls}"><h3>Row ${i}</h3><p>Description for row ${i} with plenty of words to score well.</p></div>`).join('');
    const html = `<html><body><div class="grid">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.pattern, 'DIV.alpha.beta.delta.epsilon.gamma.record.ze...');
  });
});

// A class carrying a digit, or 30+ chars long, is per-instance noise and must leave the
// signature — otherwise four identical cards become four groups of one and nothing groups.
describe('grouping: the class-name length filter', () => {
  it('groups cards whose only difference is a 30+ char generated class', () => {
    const long = ['alpha', 'bravo', 'charlie', 'delta'].map(w => `emitted-utility-classname-${w}`);
    assert.ok(long.every(c => c.length >= 30 && !/\d/.test(c)), 'the filter under test is length, not digits');
    const cards = long.map((c, i) =>
      `<div class="card ${c}"><h3><a href="/p/${i}">Card ${i}</a></h3>` +
      `<p>Description for card ${i} with plenty of words to score.</p></div>`).join('');
    const html = `<html><body><div class="grid">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.equal(r.pattern, 'DIV.card');
  });
});

// Reached only when both passes come up empty; every table fixture in the suite wins in phase 2.
describe('fallback: <tr> rows when nothing else groups', () => {
  it('returns rows with 2+ cells when no candidate survives either pass', () => {
    const rows = ['alpha', 'bravo', 'charlie'].map((c, i) =>
      `<tr class="${c}"><td><a href="/row/${i}">Row ${i}</a></td><td>Value ${i}</td></tr>`).join('');
    const r = extractFromHTML(`<html><body><table>${rows}</table></body></html>`);
    // A unique class per row keeps every signature group at one element, so phase 2 finds nothing.
    assert.equal(r.count, 3);
    assert.equal(r.pattern, 'TR.table');
    assert.deepEqual(r.items.map(i => i.title), ['Row 0', 'Row 1', 'Row 2']);
  });
});

describe('field extraction: fields array', () => {
  it('fields contain child text that is not title or price', () => {
    const html = `<html><body>
      <div class="list">
        <div class="row"><span>Alpha</span><span>Beta</span><span>Gamma</span></div>
        <div class="row"><span>Delta</span><span>Epsilon</span><span>Zeta</span></div>
        <div class="row"><span>Eta</span><span>Theta</span><span>Iota</span></div>
      </div>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 3);
    assert.deepEqual(r.items.map(i => i.fields), [
      ['Alpha', 'Beta', 'Gamma'],
      ['Delta', 'Epsilon', 'Zeta'],
      ['Eta', 'Theta', 'Iota'],
    ]);
  });

  it('fields are capped at 8', () => {
    const cells = Array.from({ length: 12 }, (_, i) => `<span>Field${i}</span>`).join('');
    const rows = Array.from({ length: 4 }, () =>
      `<div class="row">${cells}</div>`).join('');
    const html = `<html><body><div class="table">${rows}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.items.length, 4);
    for (const item of r.items) {
      assert.deepEqual(item.fields, ['Field0', 'Field1', 'Field2', 'Field3',
        'Field4', 'Field5', 'Field6', 'Field7']);
    }
  });

  // Single-word action labels are chrome, not data.
  it('drops chrome labels from fields', () => {
    const recs = Array.from({ length: 4 }, (_, i) =>
      `<div class="alpha"><h3>Record ${i}</h3><span>Star</span><span>Built by</span>` +
      `<span>Real field ${i}</span></div>`).join('');
    const r = extractFromHTML(`<html><body><div class="ga">${recs}</div></body></html>`);
    assert.equal(r.count, 4);
    assert.deepEqual(r.items.map(i => i.fields), [
      ['Real field 0'], ['Real field 1'], ['Real field 2'], ['Real field 3'],
    ]);
  });

  // A <style> one level down is not caught by the SCRIPT/STYLE skip, so its rules land in
  // the child's textContent and only cleanText's regex keeps them out of the field.
  it('strips CSS that leaks in from a nested <style>', () => {
    const recs = Array.from({ length: 4 }, (_, i) =>
      `<div class="alpha"><h3>Record ${i}</h3>` +
      `<div class="wrap"><style>.leak{color:red}</style>Visible label ${i}</div></div>`).join('');
    const r = extractFromHTML(`<html><body><div class="ga">${recs}</div></body></html>`);
    assert.equal(r.count, 4);
    assert.deepEqual(r.items.map(i => i.fields), [
      ['Visible label 0'], ['Visible label 1'], ['Visible label 2'], ['Visible label 3'],
    ]);
  });
});

describe('field extraction: dates', () => {
  it('falls back to the text of a <time> with no datetime attribute', () => {
    const recs = Array.from({ length: 4 }, (_, i) =>
      `<div class="alpha"><h3><a href="/r/${i}">Record ${i}</a></h3><time>March ${i + 1}</time>` +
      `<p>Description ${i} with enough words for scoring here.</p></div>`).join('');
    const r = extractFromHTML(`<html><body><div class="ga">${recs}</div></body></html>`);
    assert.deepEqual(r.items.map(i => i.date), ['March 1', 'March 2', 'March 3', 'March 4']);
  });
});

describe('field extraction: text fallback', () => {
  it('items with no link/heading/price get text fallback', () => {
    const rows = Array.from({ length: 4 }, (_, i) => `
      <div class="entry">
        <p>This is entry number ${i} with some text content in a single child paragraph</p>
      </div>`).join('');
    const html = `<html><body><div class="feed">${rows}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(r.items.map(i => i.fields), Array.from({ length: 4 }, (_, i) =>
      [`This is entry number ${i} with some text content in a single child paragraph`]));
  });

  // The 200-char cap only runs when no child yields a field, which needs a child whose own
  // text is already over the 200-char field limit.
  it('text fallback is capped at 200 chars', () => {
    const longText = 'A'.repeat(500);
    const rows = Array.from({ length: 4 }, () =>
      `<div class="entry"><span>${longText}</span></div>`).join('');
    const html = `<html><body><div class="list">${rows}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.equal(r.items.length, 4);
    for (const item of r.items) {
      assert.equal(item.text, 'A'.repeat(200));
      assert.ok(!item.fields, 'the 500-char child is over the field limit, so no field is emitted');
    }
  });
});

describe('anti-pattern: nav links', () => {
  it('a <nav> with many links should NOT be the main data pattern', () => {
    const navLinks = Array.from({ length: 15 }, (_, i) =>
      `<a href="/nav/${i}">Nav Item ${i}</a>`).join('');
    const cards = Array.from({ length: 5 }, (_, i) => `
      <article class="post">
        <h2><a href="/post/${i}">Blog Post ${i}</a></h2>
        <p>This is the content of blog post ${i} with enough detail to be rich</p>
        <time datetime="2025-01-0${i + 1}">Jan ${i + 1}</time>
      </article>`).join('');
    const html = `<html><body>
      <nav>${navLinks}</nav>
      <main>${cards}</main>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 5);
    assert.deepEqual(r.items.map(i => i.title),
      ['Blog Post 0', 'Blog Post 1', 'Blog Post 2', 'Blog Post 3', 'Blog Post 4']);
  });
});

describe('anti-pattern: footer links', () => {
  it('footer links should not override main content', () => {
    const cards = Array.from({ length: 4 }, (_, i) => `
      <div class="card">
        <h3><a href="/c/${i}">Content Card ${i}</a></h3>
        <p>Detailed description for content card number ${i} which is the main data</p>
      </div>`).join('');
    const footerLinks = Array.from({ length: 10 }, (_, i) =>
      `<a href="/footer/${i}">Footer Link ${i}</a>`).join('');
    const html = `<html><body>
      <main><div class="grid">${cards}</div></main>
      <footer><nav>${footerLinks}</nav></footer>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(r.items.map(i => i.title),
      ['Content Card 0', 'Content Card 1', 'Content Card 2', 'Content Card 3']);
  });
});

describe('anti-pattern: ad banners should not be main pattern', () => {
  it('interleaved ads should not become the detected pattern', () => {
    const html = loadFixture('extract_ads_interleaved.html');
    const r = extractFromHTML(html);
    // Exact, not `!includes('ad')`: a bare substring would also reject TR.load-row.
    assert.equal(r.pattern, 'ARTICLE.story');
  });
});

describe('scoring: link-heavy items penalized', () => {
  it('items where >85% of text is links score lower', () => {
    const linkItems = Array.from({ length: 5 }, (_, i) =>
      `<div class="item"><a href="/${i}">Link text ${i}</a></div>`).join('');
    const richItems = Array.from({ length: 5 }, (_, i) => `
      <div class="entry">
        <h3><a href="/e/${i}">Entry ${i}</a></h3>
        <p>Non-link descriptive text that is substantial enough to lower the link-text ratio</p>
        <span>Extra detail</span>
      </div>`).join('');
    const html = `<html><body>
      <div class="links">${linkItems}</div>
      <div class="entries">${richItems}</div>
    </body></html>`;
    const r = extractFromHTML(html);
    const titles = r.items.map(i => i.title);
    assert.ok(titles.some(t => t && t.includes('Entry')),
      'Rich entries should be preferred over link-only items');
  });
});

// Both link-density tiers in `penalise` can be deleted with the rest of this suite green:
// the pages above are ones the content group already wins on raw score. These two are sized
// so the link strip out-scores the content and only the penalty puts it back in its place.
describe('scoring: the link-density penalty is what suppresses a link strip', () => {
  const posts = Array.from({ length: 3 }, (_, i) => `
    <article class="post">
      <h2><a href="/post/${i}">Story ${i}</a></h2>
      <p>A body paragraph for story ${i} with enough words to be worth extracting.</p>
    </article>`).join('');

  it('a 50-item strip of all-link labels loses to 3 real posts (ratio > 0.85, x0.2)', () => {
    const tags = Array.from({ length: 50 }, (_, i) =>
      `<li class="item"><a href="/tag/${i}">Category ${i}0</a></li>`).join('');
    const html = `<html><body><main>
      <div class="listing">${posts}</div>
      <div class="tags"><ul>${tags}</ul></div>
    </main></body></html>`;
    const r = extractFromHTML(html);
    assert.match(r.pattern, /ARTICLE/i, `the link strip won on count alone: pattern=${r.pattern}`);
    assert.equal(r.count, 3);
  });

  it('a mostly-link strip loses too when its labels leave a little plain text (ratio > 0.7, x0.5)', () => {
    const tags = Array.from({ length: 8 }, (_, i) =>
      `<li class="item"><a href="/tag/${i}">Category ${i}0</a><span>new</span></li>`).join('');
    const html = `<html><body><main>
      <div class="listing">${posts}</div>
      <div class="tags"><ul>${tags}</ul></div>
    </main></body></html>`;
    const r = extractFromHTML(html);
    assert.match(r.pattern, /ARTICLE/i, `the link strip won on count alone: pattern=${r.pattern}`);
    assert.equal(r.count, 3);
  });
});

// The four rungs below are each worth one multiplier. Every other fixture in this suite is
// won on raw score, so each rung can be deleted with the suite green unless a page is sized
// to be decided by that rung alone: the group that should win trails on raw score and only
// its own multiplier puts it ahead.
describe('scoring: the heterogeneity boost', () => {
  const body = n => `Body text for this record, long enough to clear both the twenty and eighty ` +
    `character richness thresholds without any links at all. ${n}`;

  it('3 mixed-tag records beat 3 same-tag records that carry one more child (x1.3)', () => {
    const mixed = Array.from({ length: 3 }, (_, i) =>
      `<div class="alpha"><h3>Alpha ${i}</h3><p>${body(i)}</p><span>tail ${i}</span></div>`).join('');
    const uniform = Array.from({ length: 3 }, (_, i) =>
      `<div class="beta"><p>Beta ${i}</p><p>${body(i)}</p><p>more ${i}</p><p>even more ${i}</p></div>`).join('');
    const r = extractFromHTML(`<html><body><main>
      <div class="ga">${mixed}</div><div class="gb">${uniform}</div></main></body></html>`);
    // 15.60 vs 15.00; without the boost 12.00 vs 15.00 and DIV.beta wins.
    assert.equal(r.pattern, 'DIV.alpha');
    assert.equal(r.count, 3);
  });
});

describe('scoring: the subtree-depth boost', () => {
  const body = n => `Body text for record ${n}, long enough to clear both the twenty and the ` +
    `eighty character richness thresholds on its own.`;

  it('6 three-deep records beat 7 flat ones (x1.2)', () => {
    const deep = Array.from({ length: 6 }, (_, i) =>
      `<div class="alpha"><h3>Alpha ${i}</h3><div class="wrap"><p>${body(i)}</p></div></div>`).join('');
    const flat = Array.from({ length: 7 }, (_, i) =>
      `<div class="beta"><h3>Beta ${i}</h3><p>${body(i)}</p></div>`).join('');
    const r = extractFromHTML(`<html><body><main>
      <div class="ga">${deep}</div><div class="gb">${flat}</div></main></body></html>`);
    // 31.68 vs 30.80; without the boost 26.40 vs 30.80 and DIV.beta wins.
    assert.equal(r.pattern, 'DIV.alpha');
    assert.equal(r.count, 6);
  });
});

describe('scoring: the record-tag boost', () => {
  const body = n => `Body text for record ${n}, long enough to clear both the twenty and the ` +
    `eighty character richness thresholds on its own.`;

  it('6 <article> records beat 7 identical <div> records (x1.2)', () => {
    const arts = Array.from({ length: 6 }, (_, i) =>
      `<article class="alpha"><h3>Alpha ${i}</h3><p>${body(i)}</p></article>`).join('');
    const divs = Array.from({ length: 7 }, (_, i) =>
      `<div class="beta"><h3>Beta ${i}</h3><p>${body(i)}</p></div>`).join('');
    const r = extractFromHTML(`<html><body><main>
      <div class="ga">${arts}</div><div class="gb">${divs}</div></main></body></html>`);
    // 31.68 vs 30.80; without the boost 26.40 vs 30.80 and DIV.beta wins.
    assert.equal(r.pattern, 'ARTICLE.alpha');
    assert.equal(r.count, 6);
  });
});

describe('scoring: the body-parent penalty', () => {
  const body = n => `Body text for record ${n}, long enough to clear both the twenty and the ` +
    `eighty character richness thresholds on its own.`;

  it('6 records in a container beat 7 loose in <body> (x0.5)', () => {
    const contained = Array.from({ length: 6 }, (_, i) =>
      `<div class="alpha"><h3>Alpha ${i}</h3><p>${body(i)}</p></div>`).join('');
    const loose = Array.from({ length: 7 }, (_, i) =>
      `<div class="beta"><h3>Beta ${i}</h3><p>${body(i)}</p></div>`).join('');
    const r = extractFromHTML(`<html><body><div class="ga">${contained}</div>${loose}</body></html>`);
    // 26.40 vs 15.40; without the penalty 26.40 vs 30.80 and DIV.beta wins.
    assert.equal(r.pattern, 'DIV.alpha');
    assert.equal(r.count, 6);
  });
});

describe('scoring: semantic classes boosted', () => {
  it('items with DATA_CLASS_RE matching classes are preferred', () => {
    const semantic = Array.from({ length: 4 }, (_, i) => `
      <div class="product-item">
        <h3><a href="/p/${i}">Product ${i}</a></h3>
        <p>Product description with some detail</p>
      </div>`).join('');
    const generic = Array.from({ length: 4 }, (_, i) => `
      <div class="box">
        <h3><a href="/b/${i}">Box ${i}</a></h3>
        <p>Box description with some detail</p>
      </div>`).join('');
    const html = `<html><body>
      <div class="products">${semantic}</div>
      <div class="boxes">${generic}</div>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.pattern, 'DIV.product-item');
  });
});

describe('return shape', () => {
  it('successful extraction returns { items, count, pattern }', () => {
    const html = loadFixture('extract_cards.html');
    const r = extractFromHTML(html);
    assert.ok(Array.isArray(r.items));
    assert.equal(typeof r.count, 'number');
    assert.equal(typeof r.pattern, 'string');
    assert.ok(!r.hint, 'should not have hint on success');
  });

  it('no-pattern returns { items: [], hint: string }', () => {
    const html = loadFixture('extract_no_pattern.html');
    const r = extractFromHTML(html);
    assert.deepEqual(r.items, []);
    assert.equal(typeof r.hint, 'string');
    assert.ok(!r.count, 'should not have count on failure');
    assert.ok(!r.pattern, 'should not have pattern on failure');
  });
});

// A second pattern within 60% of the winner has to be named: silently returning one of two
// plausible lists is the failure an agent cannot detect on its own.
describe('ambiguous pages', () => {
  const twoLists = `
    <main>
      <section id="products">
        <div class="row"><h3>Widget A</h3><p>10 EUR</p><a href="/a">buy</a></div>
        <div class="row"><h3>Widget B</h3><p>20 EUR</p><a href="/b">buy</a></div>
        <div class="row"><h3>Widget C</h3><p>30 EUR</p><a href="/c">buy</a></div>
        <div class="row"><h3>Widget D</h3><p>40 EUR</p><a href="/d">buy</a></div>
      </section>
      <section id="posts">
        <article><h3>Post one</h3><p>body one</p><a href="/1">read</a></article>
        <article><h3>Post two</h3><p>body two</p><a href="/2">read</a></article>
        <article><h3>Post three</h3><p>body three</p><a href="/3">read</a></article>
        <article><h3>Post four</h3><p>body four</p><a href="/4">read</a></article>
      </section>
    </main>`;

  it('names the runner-up when two patterns are comparable', () => {
    const r = extractFromHTML(twoLists);
    assert.equal(r.count, 4);
    assert.deepEqual(r.alternatives, [{ selector: '#posts', count: 4 }]);
    assert.ok(r.hint && /selector/.test(r.hint), `and say how to disambiguate: ${r.hint}`);
  });

  it('stays quiet when there is only one plausible pattern', () => {
    const single = `<main><ul>
      <li><h3>Only A</h3><p>text a</p><a href="/a">go</a></li>
      <li><h3>Only B</h3><p>text b</p><a href="/b">go</a></li>
      <li><h3>Only C</h3><p>text c</p><a href="/c">go</a></li>
    </ul></main>`;
    const r = extractFromHTML(single);
    assert.ok(!r.alternatives, `no ambiguity to report: ${JSON.stringify(r.alternatives)}`);
  });

  // Five comparable lists: the cap is the only thing that stops all four runners-up being named.
  it('names at most 3 runners-up', () => {
    const list = id => `<ul id="${id}">${Array.from({ length: 3 }, (_, i) =>
      `<li><h3><a href="/${id}/${i}">${id} item ${i}</a></h3>` +
      `<p>Description ${i} for list ${id} with enough words to score.</p></li>`).join('')}</ul>`;
    const ids = ['la', 'lb', 'lc', 'ld', 'le'];
    const r = extractFromHTML(`<html><body><main>${ids.map(list).join('')}</main></body></html>`);
    assert.deepEqual(r.alternatives, [
      { selector: '#la', count: 3 },
      { selector: '#lb', count: 3 },
      { selector: '#lc', count: 3 },
    ]);
  });

  // selectorFor's id branch is covered above; these two are the fallbacks under it.
  describe('the selector named for a container with no id', () => {
    const recs = p => Array.from({ length: 3 }, (_, i) =>
      `<div class="rec"><h3><a href="/${p}/${i}">${p} record ${i}</a></h3>` +
      `<p>Description ${i} in ${p} with enough words to score.</p></div>`).join('');
    const page = `<html><body><main>
      <section id="primary">${recs('primary')}</section>
      <div class="secondary">${recs('secondary')}</div>
      <div class="col-2">${recs('third')}</div>
    </main></body></html>`;

    it('uses tag.class when the container has a digit-free class', () => {
      const r = extractFromHTML(page);
      assert.equal(r.alternatives[0].selector, 'div.secondary');
    });

    it('falls back to the bare tag when every class carries a digit', () => {
      const r = extractFromHTML(page);
      assert.equal(r.alternatives[1].selector, 'div');
    });
  });
});
