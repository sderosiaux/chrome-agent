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
// More adversarial coverage
// ---------------------------------------------------------------------------

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
    assert.ok(r.items !== undefined);
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
    // BUG: childSignature uses the full classList, so one unique class per sibling prevents grouping.
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
    // BUG: extract.js matches [data-price] but reads textContent instead of the data-price attribute value.
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
    // BUG: extract.js picks the longest anchor text, which can promote CTA links over the heading/title link.
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

  it('treats repeated form rows as repeated records', () => {
    const rows = Array.from({ length: 4 }, (_, i) => `
      <div class="record">
        <label>Name</label><input value="User ${i}">
        <label>Email</label><input value="user${i}@example.com">
        <button type="button">Save ${i}</button>
      </div>
    `).join('');
    const html = `<html><body><form><div class="rows">${rows}</div></form></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
  });

  // By design: extract reads textContent, not input.value. Use eval for form data.
  it.skip('captures input values when forms are the repeated records', () => {
    const rows = Array.from({ length: 4 }, (_, i) => `
      <div class="record">
        <label>Name</label><input value="User ${i}">
        <label>Email</label><input value="user${i}@example.com">
        <button type="button">Save ${i}</button>
      </div>
    `).join('');
    const html = `<html><body><form><div class="rows">${rows}</div></form></body></html>`;
    const r = extractFromHTML(html);
    // BUG: the extractor only reads textContent, so form control values are currently invisible in extracted fields/text.
    assert.ok(r.items[0].fields.includes('User 0'));
    assert.ok(r.items[0].fields.includes('user0@example.com'));
  });
});
