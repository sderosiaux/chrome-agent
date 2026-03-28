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
// Fixture-based tests
// ---------------------------------------------------------------------------

describe('fixture: extract_cards.html (blog posts)', () => {
  const html = loadFixture('extract_cards.html');

  it('detects 4 blog post cards', () => {
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.equal(r.items.length, 4);
  });

  it('pattern references ARTICLE tag', () => {
    const r = extractFromHTML(html);
    assert.ok(r.pattern, 'should have a pattern');
    assert.match(r.pattern, /ARTICLE/i);
  });

  it('extracts titles from headings', () => {
    const r = extractFromHTML(html);
    const titles = r.items.map(i => i.title);
    assert.ok(titles.includes('Understanding Rust Async'));
    assert.ok(titles.includes('Building LLM Agents That Work'));
  });

  it('extracts URLs from links', () => {
    const r = extractFromHTML(html);
    const urls = r.items.map(i => i.url);
    assert.ok(urls.some(u => u && u.includes('/blog/rust-async')));
  });

  it('extracts dates from <time> elements', () => {
    const r = extractFromHTML(html);
    const dates = r.items.map(i => i.date).filter(Boolean);
    assert.ok(dates.length >= 3, `Expected >=3 dates, got ${dates.length}`);
    assert.ok(dates.includes('2025-03-15'));
  });

  it('extracts images', () => {
    const r = extractFromHTML(html);
    const images = r.items.map(i => i.image).filter(Boolean);
    assert.ok(images.length >= 3, `Expected >=3 images, got ${images.length}`);
  });
});

describe('fixture: extract_ecommerce.html (product cards)', () => {
  const html = loadFixture('extract_ecommerce.html');

  it('detects 4 product cards, not nav links', () => {
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    // nav links should NOT dominate
    const titles = r.items.map(i => i.title);
    assert.ok(!titles.includes('Home'));
    assert.ok(!titles.includes('Login'));
  });

  it('extracts product titles from headings', () => {
    const r = extractFromHTML(html);
    const titles = r.items.map(i => i.title);
    assert.ok(titles.includes('Ultra Boost Running Shoes'));
    assert.ok(titles.includes('Travel Backpack 40L'));
  });

  it('extracts prices', () => {
    const r = extractFromHTML(html);
    const prices = r.items.map(i => i.price).filter(Boolean);
    assert.ok(prices.length >= 3, `Expected >=3 prices, got ${prices.length}`);
    assert.ok(prices.includes('$180.00'));
  });

  it('extracts images', () => {
    const r = extractFromHTML(html);
    const images = r.items.map(i => i.image).filter(Boolean);
    assert.equal(images.length, 4);
  });

  it('pattern mentions product', () => {
    const r = extractFromHTML(html);
    assert.ok(r.pattern, 'should have a pattern');
    assert.match(r.pattern, /product/i);
  });
});

describe('fixture: extract_list.html (search results)', () => {
  const html = loadFixture('extract_list.html');

  it('detects 5 search result items', () => {
    const r = extractFromHTML(html);
    assert.equal(r.count, 5);
  });

  it('extracts titles and URLs', () => {
    const r = extractFromHTML(html);
    assert.ok(r.items.some(i => i.title && i.title.includes('Playwright')));
    assert.ok(r.items.some(i => i.url && i.url.includes('playwright.dev')));
  });

  it('pattern includes LI or result', () => {
    const r = extractFromHTML(html);
    assert.ok(r.pattern.match(/LI|result/i), `pattern was: ${r.pattern}`);
  });
});

describe('fixture: extract_table.html (product table)', () => {
  const html = loadFixture('extract_table.html');

  it('detects 5 table rows', () => {
    const r = extractFromHTML(html);
    assert.ok(r.count >= 5, `Expected >=5, got ${r.count}`);
  });

  it('extracts prices from table cells', () => {
    const r = extractFromHTML(html);
    const prices = r.items.map(i => i.price).filter(Boolean);
    assert.ok(prices.length >= 3, `Expected >=3 prices, got ${prices.length}`);
  });

  it('extracts links from table cells', () => {
    const r = extractFromHTML(html);
    assert.ok(r.items.some(i => i.url && i.url.includes('/p/laptop')));
  });
});

describe('fixture: extract_flat_table.html (leaderboard)', () => {
  const html = loadFixture('extract_flat_table.html');

  it('detects 7 leaderboard rows', () => {
    const r = extractFromHTML(html);
    assert.ok(r.count >= 7, `Expected >=7, got ${r.count}`);
  });

  it('extracts user links', () => {
    const r = extractFromHTML(html);
    assert.ok(r.items.some(i => i.url && i.url.includes('/u/alice')));
  });

  it('pattern references TR', () => {
    const r = extractFromHTML(html);
    assert.match(r.pattern, /TR/i);
  });
});

describe('fixture: extract_hn_like.html (HN-style news)', () => {
  const html = loadFixture('extract_hn_like.html');

  it('extracts item rows (not spacer rows)', () => {
    const r = extractFromHTML(html);
    // There are 4 item-rows and 3 spacer rows. We want item rows.
    assert.ok(r.count >= 3, `Expected >=3, got ${r.count}`);
  });

  it('vote arrows should NOT become the title', () => {
    const r = extractFromHTML(html);
    for (const item of r.items) {
      if (item.title) {
        assert.notEqual(item.title.trim(), '\u25B2', 'Vote arrow should not be the title');
      }
    }
  });
});

describe('fixture: extract_nested_nav.html (feature page with nav)', () => {
  const html = loadFixture('extract_nested_nav.html');

  it('detects feature divs, not nav links', () => {
    const r = extractFromHTML(html);
    assert.ok(r.count >= 3, `Expected >=3 features, got ${r.count}`);
    const titles = r.items.map(i => i.title);
    // Should include feature titles, not nav
    assert.ok(titles.includes('Fast') || titles.includes('Smart') || titles.includes('Stable'),
      `Feature titles missing. Got: ${titles.join(', ')}`);
  });

  it('nav links are NOT the main pattern', () => {
    const r = extractFromHTML(html);
    const titles = r.items.map(i => i.title);
    assert.ok(!titles.includes('Home'));
    assert.ok(!titles.includes('Login'));
    assert.ok(!titles.includes('Sign Up'));
  });
});

describe('fixture: extract_no_pattern.html (about page)', () => {
  const html = loadFixture('extract_no_pattern.html');

  it('returns hint when no repeating pattern found', () => {
    const r = extractFromHTML(html);
    assert.ok(r.hint, 'should return a hint');
    assert.deepEqual(r.items, []);
  });

  it('hint contains actionable suggestion', () => {
    const r = extractFromHTML(html);
    assert.match(r.hint, /selector/i, 'hint should mention selector');
  });
});

describe('fixture: extract_mixed.html (activity feed with sidebar)', () => {
  const html = loadFixture('extract_mixed.html');

  it('detects activity items, not sidebar stats', () => {
    const r = extractFromHTML(html);
    assert.ok(r.count >= 3, `Expected >=3 activity items, got ${r.count}`);
  });

  it('extracts dates from activity items', () => {
    const r = extractFromHTML(html);
    const dates = r.items.map(i => i.date).filter(Boolean);
    assert.ok(dates.length >= 2, `Expected >=2 dates, got ${dates.length}`);
  });

  it('extracts images (avatars)', () => {
    const r = extractFromHTML(html);
    const images = r.items.map(i => i.image).filter(Boolean);
    assert.ok(images.length >= 3, `Expected >=3 images, got ${images.length}`);
  });
});

describe('fixture: extract_link_heavy_nav.html (jobs with heavy nav)', () => {
  const html = loadFixture('extract_link_heavy_nav.html');

  it('detects job listings, not nav links', () => {
    const r = extractFromHTML(html);
    assert.ok(r.count >= 3, `Expected >=3, got ${r.count}`);
    const titles = r.items.map(i => i.title);
    assert.ok(!titles.includes('Page One'), 'nav link should not be a title');
  });

  it('extracts job titles', () => {
    const r = extractFromHTML(html);
    const titles = r.items.map(i => i.title);
    assert.ok(titles.includes('Senior Rust Engineer'));
    assert.ok(titles.includes('DevOps Engineer'));
  });

  it('extracts dates', () => {
    const r = extractFromHTML(html);
    const dates = r.items.map(i => i.date).filter(Boolean);
    assert.ok(dates.length >= 2);
  });

  it('pattern references listing', () => {
    const r = extractFromHTML(html);
    assert.match(r.pattern, /listing/i);
  });
});

describe('fixture: extract_definition_list.html (FAQ)', () => {
  const html = loadFixture('extract_definition_list.html');

  it('detects FAQ items', () => {
    const r = extractFromHTML(html);
    assert.ok(r.count >= 4, `Expected >=4, got ${r.count}`);
  });

  it('extracts question titles', () => {
    const r = extractFromHTML(html);
    const titles = r.items.map(i => i.title);
    assert.ok(titles.some(t => t && t.includes('aibrowsr')));
  });
});

describe('fixture: extract_semantic_classes.html (repo list)', () => {
  const html = loadFixture('extract_semantic_classes.html');

  it('detects 4 repo cards', () => {
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
  });

  it('extracts repo names as titles', () => {
    const r = extractFromHTML(html);
    const titles = r.items.map(i => i.title);
    assert.ok(titles.includes('aibrowsr'));
    assert.ok(titles.includes('dev-browser'));
  });

  it('extracts repo URLs', () => {
    const r = extractFromHTML(html);
    assert.ok(r.items.some(i => i.url && i.url.includes('/repo/aibrowsr')));
  });

  it('extracts dates', () => {
    const r = extractFromHTML(html);
    const dates = r.items.map(i => i.date).filter(Boolean);
    assert.ok(dates.length >= 3, `Expected >=3 dates, got ${dates.length}`);
  });

  it('pattern mentions repo', () => {
    const r = extractFromHTML(html);
    assert.match(r.pattern, /repo/i);
  });
});

describe('fixture: extract_ads_interleaved.html (news with ads)', () => {
  const html = loadFixture('extract_ads_interleaved.html');

  it('detects news stories, not ad banners', () => {
    const r = extractFromHTML(html);
    assert.ok(r.count >= 3, `Expected >=3 stories, got ${r.count}`);
  });

  it('ad banners are not extracted as main items', () => {
    const r = extractFromHTML(html);
    const titles = r.items.map(i => i.title);
    assert.ok(!titles.some(t => t && t.includes('Sponsored')),
      'Ad/sponsored titles should not be in main items');
  });

  it('extracts story titles', () => {
    const r = extractFromHTML(html);
    const titles = r.items.map(i => i.title);
    assert.ok(titles.some(t => t && t.includes('Protein Folding')));
    assert.ok(titles.some(t => t && t.includes('WebAssembly GC')));
  });

  it('pattern references story or article', () => {
    const r = extractFromHTML(html);
    assert.match(r.pattern, /ARTICLE|story/i);
  });
});
