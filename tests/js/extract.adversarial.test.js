const { describe, it } = require('node:test');
const assert = require('node:assert/strict');

const { extractFromHTML } = require('./helpers.js');

describe('adversarial: malformed HTML recovery', () => {
  it('detects repeated entries with omitted closing tags', () => {
    const html = `<html><body>
      <div class="feed">
        <article class="entry"><h3><a href="/broken/1">One</a><p>Description one with enough text for scoring</article>
        <article class="entry"><h3><a href="/broken/2">Two</a><p>Description two with enough text for scoring</article>
        <article class="entry"><h3><a href="/broken/3">Three</a><p>Description three with enough text for scoring</article>
      </div>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 3);
  });

  it('keeps distinct URLs after parser recovery on broken lists', () => {
    const html = `<html><body>
      <ul class="results">
        <li class="result"><a href="/li/1">One</a><span>Desc one with enough text<li class="result"><a href="/li/2">Two</a><span>Desc two with enough text<li class="result"><a href="/li/3">Three</a><span>Desc three with enough text
      </ul>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.deepEqual(
      r.items.map(item => item.url),
      ['/li/1', '/li/2', '/li/3'],
    );
  });
});

describe('adversarial: nested tables and layout tables', () => {
  it('extracts repeated data rows from a nested table', () => {
    const html = `<html><body>
      <table class="outer">
        <tr><td>
          <table class="records">
            <tr class="record"><td><a href="/sku/1">One</a></td><td class="price">$10</td><td>Alpha</td></tr>
            <tr class="record"><td><a href="/sku/2">Two</a></td><td class="price">$20</td><td>Beta</td></tr>
            <tr class="record"><td><a href="/sku/3">Three</a></td><td class="price">$30</td><td>Gamma</td></tr>
            <tr class="record"><td><a href="/sku/4">Four</a></td><td class="price">$40</td><td>Delta</td></tr>
          </table>
        </td></tr>
      </table>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.match(r.pattern, /TR/i);
  });

  it('ignores one-off layout rows wrapped around nested tables', () => {
    const html = `<html><body>
      <table class="shell">
        <tr><td>
          <table class="summary"><tr><td>Total</td><td>4 items</td></tr></table>
        </td></tr>
        <tr class="record"><td><a href="/orders/1">Order 1</a></td><td class="price">$15</td><td>Ready</td></tr>
        <tr class="record"><td><a href="/orders/2">Order 2</a></td><td class="price">$25</td><td>Packed</td></tr>
        <tr class="record"><td><a href="/orders/3">Order 3</a></td><td class="price">$35</td><td>Shipped</td></tr>
        <tr class="record"><td><a href="/orders/4">Order 4</a></td><td class="price">$45</td><td>Delivered</td></tr>
      </table>
    </body></html>`;
    const r = extractFromHTML(html);
    assert.ok(r.items.some(item => item.title === 'Order 1'));
    assert.ok(!r.items.some(item => item.title === 'Total'));
  });
});

describe('adversarial: shadow-DOM-like wrapper nesting', () => {
  it('finds repeated cards under custom-element wrappers', () => {
    const cards = Array.from({ length: 4 }, (_, i) => `
      <x-product-card>
        <div class="shadow-root">
          <article class="product-card">
            <h3><a href="/shadow/${i}">Shadow Product ${i}</a></h3>
            <p>Description for shadow product ${i} with enough text for scoring.</p>
          </article>
        </div>
      </x-product-card>
    `).join('');
    const html = `<html><body><section class="catalog">${cards}</section></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.ok(r.items.some(item => item.title === 'Shadow Product 0'));
  });
});

describe('adversarial: decorative elements with no textContent', () => {
  it('does not crash on repeated decorative cards with empty text', () => {
    const cards = Array.from({ length: 4 }, () => `
      <div class="card">
        <span aria-hidden="true"></span>
        <span></span>
        <i></i>
      </div>
    `).join('');
    const html = `<html><body><div class="icons">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    // The cards do group, but every one is dropped for having under 3 chars of text.
    assert.deepEqual(r, { items: [], count: 0, pattern: 'DIV.card' });
  });
});

describe('adversarial: huge class lists', () => {
  it('handles very large but identical class lists', () => {
    const commonClasses = Array.from({ length: 80 }, (_, i) => `cls-${i}`).join(' ');
    const cards = Array.from({ length: 4 }, (_, i) => `
      <section class="${commonClasses} result-card">
        <h3><a href="/heavy/${i}">Heavy ${i}</a></h3>
        <p>Heavy class list item ${i} with enough content to be considered rich.</p>
      </section>
    `).join('');
    const html = `<html><body><div class="grid">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
  });

  it('still groups records when each item has an extra unique utility class', () => {
    const commonClasses = Array.from({ length: 60 }, (_, i) => `u-${i}`).join(' ');
    const cards = Array.from({ length: 4 }, (_, i) => `
      <section class="${commonClasses} result-card row-${i}">
        <h3><a href="/variant/${i}">Variant ${i}</a></h3>
        <p>Variant ${i} has rich content and identical structure besides one unique class.</p>
      </section>
    `).join('');
    const html = `<html><body><div class="grid">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    // The digit-class filter in childSignature is what lets these group.
    assert.equal(r.count, 4);
  });
});

describe('adversarial: data and role attributes', () => {
  it('extracts visible prices from data-price elements', () => {
    const cards = Array.from({ length: 4 }, (_, i) => `
      <div class="product">
        <h3><a href="/product/${i}">Product ${i}</a></h3>
        <span data-price="$${i + 10}">$${i + 10}</span>
        <p>Description for product ${i} with enough body text.</p>
      </div>
    `).join('');
    const html = `<html><body><div class="grid">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.deepEqual(
      r.items.map(item => item.price),
      ['$10', '$11', '$12', '$13'],
    );
  });

  it('uses the data-price attribute when the node has no visible price text', () => {
    const cards = Array.from({ length: 4 }, (_, i) => `
      <div class="product">
        <h3><a href="/attr/${i}">Attr Product ${i}</a></h3>
        <span data-price="$${i + 20}"></span>
        <p>Description for attr product ${i} with enough body text.</p>
      </div>
    `).join('');
    const html = `<html><body><div class="grid">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.deepEqual(
      r.items.map(item => item.price),
      ['$20', '$21', '$22', '$23'],
    );
  });

  it('uses [role=heading] as the item title', () => {
    const cards = Array.from({ length: 4 }, (_, i) => `
      <div class="record">
        <div role="heading">Role Heading ${i}</div>
        <div>Description for record ${i} with enough content for extraction.</div>
      </div>
    `).join('');
    const html = `<html><body><div class="records">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.items[0].title, 'Role Heading 0');
  });
});

describe('adversarial: deep recursion and mixed encoding', () => {
  it('handles deeply recursive wrappers around repeated records', () => {
    function nest(level, inner) {
      return level === 0 ? inner : `<div class="layer-${level}">${nest(level - 1, inner)}</div>`;
    }

    const records = Array.from({ length: 4 }, (_, i) =>
      nest(
        30,
        `<article class="record"><h3><a href="/deep/${i}">Deep ${i}</a></h3><p>Description ${i} with enough content to survive recursive traversal.</p></article>`,
      )).join('');
    const html = `<html><body><div class="root">${records}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
  });

  it('decodes HTML entities and mixed unicode content', () => {
    const cards = [
      ['AT&amp;T&nbsp;Launch', 'Price&nbsp;&euro;10'],
      ['Fran&ccedil;ais &#9731;', 'R&eacute;sum&eacute; &amp; details'],
      ['Emoji &#x1F680; mission', 'Mixed&nbsp;text &amp; entities'],
      ['M&uuml;nchen data', 'Encoded &lt;strong&gt;text&lt;/strong&gt; sample'],
    ].map(([title, body], i) => `
      <article class="record">
        <h3><a href="/encoded/${i}">${title}</a></h3>
        <p>${body} long enough content here for richness scoring.</p>
      </article>
    `).join('');
    const html = `<html><body><div class="feed">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    const titles = r.items.map(item => item.title);
    assert.ok(titles.includes('AT&T Launch'));
    assert.ok(titles.includes('Français ☃'));
    assert.ok(titles.includes('Emoji 🚀 mission'));
    assert.ok(titles.includes('München data'));
  });
});

describe('adversarial: same-tag siblings and noisy children', () => {
  it('preserves unique titles for same-tag siblings with different content lengths', () => {
    const articles = [
      ['Alpha', 'Short body with enough content to matter for extraction.'],
      ['Beta release notes', 'Much longer body content that still shares the same DOM structure for grouping.'],
      ['Gamma roadmap', 'Another body block that differs in length and wording but not structure.'],
      ['Delta incident review', 'Final body block with additional words for variation and scoring depth.'],
    ].map(([title, body], i) => `
      <article class="entry">
        <h3><a href="/siblings/${i}">${title}</a></h3>
        <p>${body}</p>
      </article>
    `).join('');
    const html = `<html><body><div class="feed">${articles}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(new Set(r.items.map(item => item.title)).size, 4);
  });

  it('prefers the primary heading link over a longer secondary CTA link', () => {
    const articles = [
      'Short title',
      'Another concise title',
      'Third concise title',
      'Fourth concise title',
    ].map((title, i) => `
      <article class="entry">
        <h3><a href="/primary/${i}">${title}</a></h3>
        <a href="/cta/${i}">Read the full annotated transcript for item ${i}</a>
        <p>Description for article ${i} with enough detail to pass richness scoring.</p>
      </article>
    `).join('');
    const html = `<html><body><div class="feed">${articles}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.deepEqual(
      r.items.map(item => item.url),
      ['/primary/0', '/primary/1', '/primary/2', '/primary/3'],
    );
  });

  it('ignores script and style tag noise inside rich cards', () => {
    const stories = Array.from({ length: 4 }, (_, i) => `
      <article class="story">
        <script>window.__noise${i} = ${i};</script>
        <style>.story-${i} { color: red; }</style>
        <h2><a href="/story/${i}">Story ${i}</a></h2>
        <p>Story ${i} body with enough meaningful text to dominate any embedded noise.</p>
      </article>
    `).join('');
    const html = `<html><body><div class="feed">${stories}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.deepEqual(
      r.items.map(item => item.title),
      ['Story 0', 'Story 1', 'Story 2', 'Story 3'],
    );
  });
});

describe('adversarial: svg-heavy cards and forms as records', () => {
  it('extracts cards that include inline SVG elements', () => {
    const cards = Array.from({ length: 4 }, (_, i) => `
      <div class="result">
        <svg viewBox="0 0 10 10" aria-hidden="true">
          <text x="1" y="5">${i}</text>
        </svg>
        <h3><a href="/svg/${i}">SVG Card ${i}</a></h3>
        <p>Description for SVG card ${i} with enough text to be rich.</p>
      </div>
    `).join('');
    const html = `<html><body><div class="results">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.ok(r.items.some(item => item.title === 'SVG Card 0'));
  });

  const formRows = `<html><body><form><div class="rows">${Array.from({ length: 4 }, (_, i) => `
      <div class="record">
        <label>Name</label><input value="User ${i}">
        <label>Email</label><input value="user${i}@example.com">
        <button type="button">Save ${i}</button>
      </div>
    `).join('')}</div></form></body></html>`;

  it('treats repeated form rows as repeated records', () => {
    const r = extractFromHTML(formRows);
    assert.equal(r.count, 4);
  });

  // By design: extract reads textContent, not input.value — use eval for form data. Was an
  // `it.skip` asserting the opposite, which could neither pass nor fail.
  it('does not read input values into fields', () => {
    const r = extractFromHTML(formRows);
    for (const item of r.items) {
      const blob = JSON.stringify(item);
      assert.doesNotMatch(blob, /User \d/);
      assert.doesNotMatch(blob, /@example\.com/);
    }
  });
});

// The semantic fast-pass must not be a way around the anti-navigation rules.
describe('adversarial: navigation wearing a data class name', () => {
  it('does not return nav links as the record set when real content is present', () => {
    // "nav-item" matches DATA_CLASS_RE, so these enter the phase-1 fast pass; without the
    // shared nav and link-density penalties they outscore the real product list.
    const nav = Array.from({ length: 8 }, (_, i) =>
      `<li class="nav-item"><a href="/p${i}">Navigation Menu Label ${i}</a></li>`).join('');
    const cards = Array.from({ length: 3 }, (_, i) =>
      `<div class="feature-box"><h3>Product ${i}</h3>` +
      `<p>A real description of product ${i} here.</p><span>$${i}9.99</span></div>`).join('');
    const html = `<html><body><nav class="main-nav"><ul>${nav}</ul></nav><main>${cards}</main></body></html>`;

    const r = extractFromHTML(html);
    assert.ok(
      !/nav-item/.test(r.pattern),
      `expected the content list, got the navigation: pattern=${r.pattern}`
    );
    assert.equal(r.count, 3, `expected the 3 product cards, got ${r.count}`);
    assert.ok(/Product 0/.test(JSON.stringify(r.items[0])), JSON.stringify(r.items[0]));
  });

  it('still returns a semantic list that is genuinely content', () => {
    const items = Array.from({ length: 5 }, (_, i) =>
      `<div class="product-item"><h3>Item ${i}</h3><p>Description number ${i} with real text.</p>` +
      `<span class="price">$${i}.00</span></div>`).join('');
    const html = `<html><body><main><div class="list">${items}</div></main></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 5, `the fast-pass must still fire on real content: ${JSON.stringify(r)}`);
  });
});

describe('adversarial: identical siblings with different content', () => {
  it('detects items that share structure but have different text', () => {
    const cards = Array.from({ length: 5 }, (_, i) => `
      <div class="result">
        <h3><a href="/r/${i}">${'X'.repeat(20 + i * 5)}</a></h3>
        <p>${'Y'.repeat(50 + i * 10)}</p>
      </div>`).join('');
    const html = `<html><body><div class="results">${cards}</div></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 5);
    const titles = r.items.map(i => i.title);
    assert.equal(new Set(titles).size, titles.length, 'all titles should be unique');
  });
});

describe('adversarial: items with no children', () => {
  it('handles items that are leaf elements with only text', () => {
    const items = Array.from({ length: 5 }, (_, i) =>
      `<p class="entry">Entry number ${i}: This has enough text to pass the richness check hopefully</p>`).join('');
    const html = `<html><body><div class="list">${items}</div></body></html>`;
    const r = extractFromHTML(html);
    // No children and no links puts richness under 2, so nothing groups.
    assert.deepEqual(r, { items: [], hint: 'No repeating pattern found. Try: extract --selector or eval --selector' });
  });
});

describe('adversarial: mixed tag types as siblings', () => {
  it('only groups siblings with same signature', () => {
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
    // Exactly 3, not `>= 3`: the six siblings are two record types of three each, and merging
    // them into one group of six is the defect this fixture exists for.
    assert.equal(r.count, 3, `the two tag groups must stay apart, got ${r.count}`);
    const tag = r.pattern.split('.')[0];
    assert.ok(['DIV', 'SECTION'].includes(tag), `Expected DIV or SECTION, got: ${tag}`);
    const titles = r.items.map(i => JSON.stringify(i)).join(' ');
    const mixed = /Div \d/.test(titles) && /Section \d/.test(titles);
    assert.ok(!mixed, `records of both types were merged into one list: ${titles}`);
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
    assert.equal(r.count, 4);
    // Every digit-carrying class is dropped from the signature; only the digit-free one is left.
    assert.equal(r.pattern, 'DIV.result-card__wrapper');
  });
});
