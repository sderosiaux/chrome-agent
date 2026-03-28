const { describe, it } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');

const { extractFromHTML, extractFromHTMLWithSelector } = require('./helpers.js');

const FIXTURES = path.resolve(__dirname, '..', 'fixtures');

function loadFixture(name) {
  return fs.readFileSync(path.join(FIXTURES, name), 'utf-8');
}


// ---------------------------------------------------------------------------
// Edge cases: dynamically generated HTML
// ---------------------------------------------------------------------------

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
    assert.ok(r.count >= 4, `Expected >=4, got ${r.count}`);
    assert.ok(r.items[0].title.includes('Nested Item'));
  });
});

describe('edge: all-links page', () => {
  it('handles page that is only links (sitemap style)', () => {
    const links = Array.from({ length: 20 }, (_, i) =>
      `<li><a href="/p/${i}">Page ${i}</a></li>`).join('');
    const html = `<html><body><ul>${links}</ul></body></html>`;
    const r = extractFromHTML(html);
    // The link-text ratio is 1.0 so score should be penalized heavily.
    // Might still extract or might return hint -- either is acceptable.
    // Key: should NOT crash.
    assert.ok(r.items !== undefined);
  });
});

describe('edge: page with only images', () => {
  it('handles page with only images, no text', () => {
    const imgs = Array.from({ length: 5 }, (_, i) =>
      `<div><img src="/img/${i}.jpg"></div>`).join('');
    const html = `<html><body><div class="gallery">${imgs}</div></body></html>`;
    const r = extractFromHTML(html);
    // Images with no text and no links likely won't pass richness >= 2,
    // so likely a hint. Either way should not crash.
    assert.ok(r.items !== undefined);
  });
});

describe('edge: unicode content', () => {
  it('handles unicode titles and text', () => {
    const items = [
      { title: '\u5F00\u6E90\u8F6F\u4EF6', desc: '\u8FD9\u662F\u4E00\u4E2A\u5F00\u6E90\u9879\u76EE\u7684\u8BE6\u7EC6\u63CF\u8FF0\uFF0C\u4E3A\u4E86\u6D4B\u8BD5\u591A\u8BED\u8A00\u652F\u6301' },
      { title: '\u30D7\u30ED\u30B0\u30E9\u30DF\u30F3\u30B0', desc: '\u30D7\u30ED\u30B0\u30E9\u30DF\u30F3\u30B0\u306B\u95A2\u3059\u308B\u8A73\u7D30\u306A\u8AAC\u660E\u3068\u4F8B\u3092\u793A\u3057\u307E\u3059' },
      { title: '\u041F\u0440\u043E\u0433\u0440\u0430\u043C\u043C\u0438\u0440\u043E\u0432\u0430\u043D\u0438\u0435', desc: '\u041F\u043E\u0434\u0440\u043E\u0431\u043D\u043E\u0435 \u043E\u043F\u0438\u0441\u0430\u043D\u0438\u0435 \u043F\u0440\u043E\u0435\u043A\u0442\u0430 \u0434\u043B\u044F \u0442\u0435\u0441\u0442\u0438\u0440\u043E\u0432\u0430\u043D\u0438\u044F' },
      { title: 'Caf\u00E9 \u00C9moji \u2615', desc: 'Un projet open source avec des caract\u00E8res sp\u00E9ciaux pour tester' },
    ];
    const cards = items.map(i => `
      <div class="item"><h3><a href="/x">${i.title}</a></h3><p>${i.desc}</p></div>`).join('');
    const html = `<html><body><div class="list">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.ok(r.count >= 3, `Expected >=3, got ${r.count}`);
    assert.ok(r.items.some(i => i.title && i.title.includes('\u5F00\u6E90')));
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
    assert.ok(r.count >= 100, `Expected >=100, got ${r.count}`);
    // Default limit is 20
    assert.ok(r.items.length <= 20, `Items should be capped by default limit, got ${r.items.length}`);
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
    assert.ok(r.count >= 3);
    for (const item of r.items) {
      if (item.title) {
        assert.ok(!item.title.startsWith(' '), 'title should be trimmed');
        assert.ok(!item.title.endsWith(' '), 'title should be trimmed');
        assert.ok(!item.title.includes('  '), 'title should collapse whitespace');
      }
    }
  });
});

// ---------------------------------------------------------------------------
// Limit parameter
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Selector scoping
// ---------------------------------------------------------------------------

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
    assert.ok(r.count >= 3, `Expected >=3, got ${r.count}`);
    const titles = r.items.map(i => i.title);
    assert.ok(titles.some(t => t && t.includes('Main')));
    assert.ok(!titles.some(t => t && t.includes('Sidebar')));
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

// ---------------------------------------------------------------------------
// Pattern string format
// ---------------------------------------------------------------------------

describe('pattern string format', () => {
  it('pattern has TAG.class format', () => {
    const html = loadFixture('extract_semantic_classes.html');
    const r = extractFromHTML(html);
    // pattern should be like "DIV.repo-card"
    assert.ok(r.pattern);
    const parts = r.pattern.split('.');
    assert.ok(parts.length >= 2, `Expected TAG.class format, got: ${r.pattern}`);
    // First part should be an HTML tag
    assert.match(parts[0], /^[A-Z]+$/, `Tag should be uppercase, got: ${parts[0]}`);
  });

  it('pattern class comes from element classList', () => {
    const html = loadFixture('extract_link_heavy_nav.html');
    const r = extractFromHTML(html);
    assert.match(r.pattern, /listing/i);
  });
});

// ---------------------------------------------------------------------------
// Field extraction details
// ---------------------------------------------------------------------------

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
    // These simple rows with no links/headings should get text or fields
    assert.ok(r.count >= 3);
    // Each item should have either text or fields
    for (const item of r.items) {
      assert.ok(item.text || item.fields, 'item should have text or fields');
    }
  });

  it('fields are capped at 8', () => {
    const cells = Array.from({ length: 12 }, (_, i) => `<span>Field${i}</span>`).join('');
    const rows = Array.from({ length: 4 }, () =>
      `<div class="row">${cells}</div>`).join('');
    const html = `<html><body><div class="table">${rows}</div></body></html>`;
    const r = extractFromHTML(html);
    if (r.items && r.items.length > 0) {
      for (const item of r.items) {
        if (item.fields) {
          assert.ok(item.fields.length <= 8, `fields should cap at 8, got ${item.fields.length}`);
        }
      }
    }
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
    // With only one child and no links, items should fall back to text
    if (r.items && r.items.length > 0) {
      for (const item of r.items) {
        assert.ok(item.text || item.title || item.fields,
          'item must have text, title, or fields');
      }
    }
  });

  it('text fallback is capped at 200 chars', () => {
    const longText = 'A'.repeat(500);
    const rows = Array.from({ length: 4 }, () =>
      `<div class="entry">${longText}</div>`).join('');
    const html = `<html><body><div class="list">${rows}</div></body></html>`;
    const r = extractFromHTML(html);
    if (r.items && r.items.length > 0) {
      for (const item of r.items) {
        if (item.text) {
          assert.ok(item.text.length <= 200, `text should be <=200 chars, got ${item.text.length}`);
        }
      }
    }
  });
});

// ---------------------------------------------------------------------------
// Anti-patterns
// ---------------------------------------------------------------------------

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
    const titles = r.items.map(i => i.title);
    assert.ok(!titles.some(t => t && t.startsWith('Nav Item')),
      'Nav links should not be extracted as main data');
    assert.ok(titles.some(t => t && t.includes('Blog Post')));
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
    const titles = r.items.map(i => i.title);
    assert.ok(titles.some(t => t && t.includes('Content Card')));
  });
});

describe('anti-pattern: ad banners should not be main pattern', () => {
  it('interleaved ads should not become the detected pattern', () => {
    const html = loadFixture('extract_ads_interleaved.html');
    const r = extractFromHTML(html);
    assert.ok(r.pattern);
    assert.ok(!r.pattern.includes('ad'), `Pattern should not be ads: ${r.pattern}`);
  });
});

// ---------------------------------------------------------------------------
// Algorithm internals: scoring heuristics
// ---------------------------------------------------------------------------

describe('scoring: link-heavy items penalized', () => {
  it('items where >85% of text is links score lower', () => {
    // Build a page where all content is links vs a page with mixed content
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
    // The rich items should win
    const titles = r.items.map(i => i.title);
    assert.ok(titles.some(t => t && t.includes('Entry')),
      'Rich entries should be preferred over link-only items');
  });
});

describe('scoring: semantic classes boosted', () => {
  it('items with DATA_CLASS_RE matching classes are preferred', () => {
    // Two groups of items, one with semantic class names, one without
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
    assert.match(r.pattern, /product/i, 'semantic class items should be preferred');
  });
});

// ---------------------------------------------------------------------------
// Return shape validation
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tricky / adversarial HTML
// ---------------------------------------------------------------------------

describe('adversarial: identical siblings with different content', () => {
  it('detects items that share structure but have different text', () => {
    const cards = Array.from({ length: 5 }, (_, i) => `
      <div class="result">
        <h3><a href="/r/${i}">${'X'.repeat(20 + i * 5)}</a></h3>
        <p>${'Y'.repeat(50 + i * 10)}</p>
      </div>`).join('');
    const html = `<html><body><div class="results">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.ok(r.count >= 5);
    // All items should be unique
    const titles = r.items.map(i => i.title);
    const unique = new Set(titles);
    assert.equal(unique.size, titles.length, 'all titles should be unique');
  });
});

describe('adversarial: items with no children', () => {
  it('handles items that are leaf elements with only text', () => {
    // Items with zero children get childSignature like "P||"
    const items = Array.from({ length: 5 }, (_, i) =>
      `<p class="entry">Entry number ${i}: This has enough text to pass the richness check hopefully</p>`).join('');
    const html = `<html><body><div class="list">${items}</div></body></html>`;
    const r = extractFromHTML(html);
    // These are leaf elements with no children, richness will be low (children.length < 2)
    // and no links/images. Likely returns hint. Should NOT crash.
    assert.ok(r.items !== undefined);
  });
});

describe('adversarial: nested tables', () => {
  it('handles table within table without crashing', () => {
    const html = `<html><body>
      <table>
        <tr><td>
          <table class="inner">
            <tr><td><a href="/a">A</a></td><td class="price">$10</td><td>Cat A</td></tr>
            <tr><td><a href="/b">B</a></td><td class="price">$20</td><td>Cat B</td></tr>
            <tr><td><a href="/c">C</a></td><td class="price">$30</td><td>Cat C</td></tr>
            <tr><td><a href="/d">D</a></td><td class="price">$40</td><td>Cat D</td></tr>
          </table>
        </td></tr>
      </table>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.ok(r.count >= 3, `Expected >=3, got ${r.count}`);
  });
});

describe('adversarial: mixed tag types as siblings', () => {
  it('only groups siblings with same signature', () => {
    // Mix of <div> and <section> — the algorithm should group them separately
    const html = `<html><body>
      <div class="feed">
        <div class="card"><h3><a href="/d1">Div 1</a></h3><p>Description one</p></div>
        <section class="card"><h3><a href="/s1">Section 1</a></h3><p>Description one</p></section>
        <div class="card"><h3><a href="/d2">Div 2</a></h3><p>Description two</p></div>
        <section class="card"><h3><a href="/s2">Section 2</a></h3><p>Description two</p></section>
        <div class="card"><h3><a href="/d3">Div 3</a></h3><p>Description three</p></div>
        <section class="card"><h3><a href="/s3">Section 3</a></h3><p>Description three</p></section>
      </div>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.ok(r.count >= 3, `Expected >=3, got ${r.count}`);
    // All items should be of the same tag type
    const tag = r.pattern.split('.')[0];
    assert.ok(['DIV', 'SECTION'].includes(tag), `Expected DIV or SECTION, got: ${tag}`);
  });
});

describe('adversarial: special characters in class names', () => {
  it('handles class names with hyphens and numbers', () => {
    const items = Array.from({ length: 4 }, (_, i) => `
      <div class="item-2025-v3 result-card__wrapper">
        <h3><a href="/x/${i}">Result ${i}</a></h3>
        <p>Detailed content for result number ${i} to ensure sufficient richness scoring</p>
      </div>`).join('');
    const html = `<html><body><div>${items}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.ok(r.count >= 4);
    assert.ok(r.pattern.includes('result'), `pattern should reference result class: ${r.pattern}`);
  });
});
