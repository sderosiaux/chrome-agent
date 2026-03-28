const { describe, test } = require('node:test');
const assert = require('node:assert/strict');

const { extractFromHTML } = require('./helpers.js');

function titlesOf(result) {
  return result.items.map(item => item.title);
}

function urlsOf(result) {
  return result.items.map(item => item.url);
}

function itemFields(item) {
  return Array.isArray(item.fields) ? item.fields : [];
}

describe('codex: complex real-world HTML patterns', () => {
  test('extracts amazon-like catalog cards instead of utility links and pagination', () => {
    const products = [
      ['Noise-Cancelling Headphones', '/dp/100', '$299.99'],
      ['Mechanical Keyboard, 75%', '/dp/101', '$149.00'],
      ['Portable USB-C Monitor', '/dp/102', '$219.95'],
      ['Ergonomic Desk Lamp', '/dp/103', '$79.50'],
    ].map(([title, url, price], i) => `
      <article class="product-card s-result-item">
        <div class="badge-row"><span>Prime</span><span>Climate Pledge Friendly</span></div>
        <h2 class="a-size-base-plus"><a href="${url}">${title}</a></h2>
        <div class="image-wrap"><img src="/img/${i}.jpg" alt="${title}"></div>
        <div class="meta">
          <span class="rating">4.${i + 3} stars</span>
          <span class="price">${price}</span>
          <a href="/offers/${i}">More buying choices</a>
        </div>
      </article>
    `).join('');

    const html = `<html><body>
      <header>
        <nav class="top-links">
          <a href="/deals">Today's Deals</a>
          <a href="/registry">Registry</a>
          <a href="/sell">Sell</a>
          <a href="/help">Help</a>
        </nav>
      </header>
      <main>
        <div class="grid">${products}</div>
        <nav class="pagination">
          <a href="/page/1">1</a>
          <a href="/page/2">2</a>
          <a href="/page/3">3</a>
          <a href="/page/next">Next</a>
        </nav>
      </main>
    </body></html>`;

    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(titlesOf(r), [
      'Noise-Cancelling Headphones',
      'Mechanical Keyboard, 75%',
      'Portable USB-C Monitor',
      'Ergonomic Desk Lamp',
    ]);
    assert.deepEqual(r.items.map(item => item.price), ['$299.99', '$149.00', '$219.95', '$79.50']);
    assert.match(r.pattern, /product|result|ARTICLE/i);
  });

  test('extracts reddit-like submission cards and ignores nested replies', () => {
    const posts = [
      ['Show HN: Visual DOM diff for browser agents', '/r/programming/comments/abc123/show_hn_visual_dom_diff'],
      ['Ask HN: Best way to scrape paginated forums?', '/r/programming/comments/abc124/ask_hn_scrape_paginated_forums'],
      ['TIL about CSS content-visibility', '/r/programming/comments/abc125/til_css_content_visibility'],
      ['Launch: Deterministic test fixtures for JS DOM extraction', '/r/programming/comments/abc126/launch_dom_extraction_fixtures'],
    ].map(([title, url], i) => `
      <article class="post thing">
        <div class="score">${120 + i}</div>
        <h3><a href="${url}">${title}</a></h3>
        <ul class="meta">
          <li>u/example_${i}</li>
          <li>r/programming</li>
          <li>${10 + i} comments</li>
        </ul>
        <div class="comments">
          <article class="reply"><a href="${url}#r1">Reply one with lots of words ${i}</a></article>
          <article class="reply"><a href="${url}#r2">Reply two with lots of words ${i}</a></article>
        </div>
      </article>
    `).join('');

    const html = `<html><body><main class="feed">${posts}</main></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(urlsOf(r), [
      '/r/programming/comments/abc123/show_hn_visual_dom_diff',
      '/r/programming/comments/abc124/ask_hn_scrape_paginated_forums',
      '/r/programming/comments/abc125/til_css_content_visibility',
      '/r/programming/comments/abc126/launch_dom_extraction_fixtures',
    ]);
    assert.ok(!titlesOf(r).some(title => title && title.includes('Reply one')));
  });

  test('extracts GitHub-like issues instead of repo sidebar counts', () => {
    const issues = [
      ['Intermittent parser panic on malformed list HTML', '/org/repo/issues/101'],
      ['Track hidden tab panels during extraction scoring', '/org/repo/issues/102'],
      ['Prefer heading links over CTA links in cards', '/org/repo/issues/103'],
      ['Support dt/dd style result lists without flattening', '/org/repo/issues/104'],
    ].map(([title, url], i) => `
      <div class="js-issue-row issue-list-item">
        <div class="flex-row">
          <h3 class="issue-title"><a href="${url}">${title}</a></h3>
          <div class="labels">
            <a href="/labels/bug">bug</a>
            <a href="/labels/extractor">extractor</a>
            <a href="/labels/p${i + 1}">p${i + 1}</a>
          </div>
        </div>
        <div class="subtle">
          <span>#${101 + i}</span>
          <time datetime="2026-03-2${i + 1}">Mar ${21 + i}, 2026</time>
          <span>opened by qa-bot</span>
        </div>
      </div>
    `).join('');

    const html = `<html><body>
      <aside class="repo-nav">
        <a href="/org/repo/issues">Issues 128</a>
        <a href="/org/repo/pulls">Pull requests 9</a>
        <a href="/org/repo/actions">Actions</a>
        <a href="/org/repo/security">Security</a>
      </aside>
      <section class="issues">${issues}</section>
    </body></html>`;

    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(urlsOf(r), [
      '/org/repo/issues/101',
      '/org/repo/issues/102',
      '/org/repo/issues/103',
      '/org/repo/issues/104',
    ]);
    assert.ok(!titlesOf(r).includes('Pull requests 9'));
  });

  test('extracts Airbnb-like listings instead of map markers', () => {
    const listings = [
      ['Loft in SoHo', '/rooms/501', '$320 night'],
      ['Cabin near Asheville', '/rooms/502', '$210 night'],
      ['Design studio in Lisbon', '/rooms/503', '$185 night'],
      ['Beach house in Santa Cruz', '/rooms/504', '$440 night'],
    ].map(([title, url, price], i) => `
      <article class="listing-card stay-card">
        <img src="/listing/${i}.jpg" alt="${title}">
        <div class="copy">
          <h2><a href="${url}">${title}</a></h2>
          <div class="meta">
            <span>4.${8 - i} stars</span>
            <span class="price">${price}</span>
            <span>Free cancellation</span>
          </div>
        </div>
      </article>
    `).join('');

    const html = `<html><body>
      <main class="results">${listings}</main>
      <aside class="map">
        <ol class="markers">
          <li><button>1</button></li>
          <li><button>2</button></li>
          <li><button>3</button></li>
          <li><button>4</button></li>
        </ol>
      </aside>
    </body></html>`;

    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(titlesOf(r), [
      'Loft in SoHo',
      'Cabin near Asheville',
      'Design studio in Lisbon',
      'Beach house in Santa Cruz',
    ]);
    assert.ok(!titlesOf(r).includes('1'));
    assert.ok(r.items.every(item => item.price && item.price.includes('$')));
  });

  test('extracts Yelp-like review cards and ignores side recommendations', () => {
    const reviews = [
      ['Worth the wait for ramen', '/biz/tatsu-ramen?hrid=1'],
      ['Excellent patio and cocktails', '/biz/tatsu-ramen?hrid=2'],
      ['Surprisingly good vegan options', '/biz/tatsu-ramen?hrid=3'],
      ['Service recovered after a rough start', '/biz/tatsu-ramen?hrid=4'],
    ].map(([title, url], i) => `
      <article class="review review-card">
        <div class="stars">5 stars</div>
        <h3><a href="${url}">${title}</a></h3>
        <p>The full review body for card ${i} has enough content to beat short recommendation links in sidebars.</p>
        <time datetime="2026-02-1${i}">Feb ${10 + i}, 2026</time>
      </article>
    `).join('');

    const html = `<html><body>
      <main>${reviews}</main>
      <aside>
        <ul class="also-viewed">
          <li><a href="/biz/other-1">People also viewed Sushi Place</a></li>
          <li><a href="/biz/other-2">People also viewed Burger Place</a></li>
          <li><a href="/biz/other-3">People also viewed Pizza Place</a></li>
          <li><a href="/biz/other-4">People also viewed Bakery Place</a></li>
        </ul>
      </aside>
    </body></html>`;

    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.ok(titlesOf(r).includes('Worth the wait for ramen'));
    assert.ok(!titlesOf(r).some(title => title && title.includes('People also viewed')));
  });

  test('extracts Wikipedia-like table rows from tbody instead of headers', () => {
    const rows = [
      ['Norway', '/wiki/Norway', '5.5 million'],
      ['Japan', '/wiki/Japan', '123.9 million'],
      ['Chile', '/wiki/Chile', '19.6 million'],
      ['Kenya', '/wiki/Kenya', '55.1 million'],
    ].map(([country, url, population]) => `
      <tr class="data-row">
        <th scope="row"><a href="${url}">${country}</a></th>
        <td>${population}</td>
        <td><a href="${url}#economy">Economy</a></td>
      </tr>
    `).join('');

    const html = `<html><body>
      <table class="wikitable sortable">
        <thead>
          <tr><th>Country</th><th>Population</th><th>Notes</th></tr>
        </thead>
        <tbody>${rows}</tbody>
      </table>
    </body></html>`;

    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.match(r.pattern, /TR/i);
    assert.deepEqual(titlesOf(r), ['Norway', 'Japan', 'Chile', 'Kenya']);
    assert.ok(!titlesOf(r).includes('Country'));
  });

  test('extracts Spotify-like playlist rows with artist fields', () => {
    const tracks = [
      ['Ocean Avenue', '/track/201', 'Yellowcard', 'Ocean Avenue'],
      ['Intro', '/track/202', 'The xx', 'xx'],
      ['Dreams', '/track/203', 'Fleetwood Mac', 'Rumours'],
      ['Midnight City', '/track/204', 'M83', 'Hurry Up, We Are Dreaming'],
    ].map(([title, url, artist, album], i) => `
      <tr class="tracklist-row">
        <td class="index">${i + 1}</td>
        <td class="drag-handle" aria-hidden="true">::</td>
        <td class="title-cell"><a href="${url}">${title}</a></td>
        <td class="artist-cell">${artist}</td>
        <td class="album-cell">${album}</td>
        <td class="time-cell">${3 + i}:1${i}</td>
      </tr>
    `).join('');

    const html = `<html><body>
      <table class="playlist-table">
        <tbody>${tracks}</tbody>
      </table>
    </body></html>`;

    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(titlesOf(r), ['Ocean Avenue', 'Intro', 'Dreams', 'Midnight City']);
    assert.ok(r.items.some(item => itemFields(item).some(field => field.includes('Yellowcard'))));
  });

  test('extracts loaded job cards and ignores lazy skeleton placeholders', () => {
    const skeletons = Array.from({ length: 3 }, () => `
      <article class="job-card skeleton" aria-hidden="true">
        <div>Loading title...</div>
        <div>Loading company...</div>
      </article>
    `).join('');

    const jobs = [
      ['Senior Browser Automation Engineer', '/jobs/901'],
      ['Staff Data Extraction Engineer', '/jobs/902'],
      ['Platform QA Lead', '/jobs/903'],
      ['Frontend Systems Engineer', '/jobs/904'],
    ].map(([title, url], i) => `
      <article class="job-card">
        <h2><a href="${url}">${title}</a></h2>
        <div class="company">Company ${i + 1}</div>
        <div class="details">
          <span>Remote</span>
          <time datetime="2026-03-1${i}">Mar ${10 + i}</time>
        </div>
      </article>
    `).join('');

    const html = `<html><body>
      <section class="jobs">${skeletons}${jobs}</section>
      <aside><a href="/newsletter">Get job alerts</a></aside>
    </body></html>`;

    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.ok(!titlesOf(r).some(title => title && title.includes('Loading')));
    assert.deepEqual(urlsOf(r), ['/jobs/901', '/jobs/902', '/jobs/903', '/jobs/904']);
  });

  test('extracts recipe cards instead of accordion FAQ entries', () => {
    const recipes = [
      ['Crispy Chili Oil Noodles', '/recipes/chili-oil-noodles'],
      ['Sheet-Pan Gnocchi with Tomatoes', '/recipes/sheet-pan-gnocchi'],
      ['Brown Butter Lemon Cookies', '/recipes/brown-butter-cookies'],
      ['Miso Mushroom Rice Bowls', '/recipes/miso-mushroom-rice-bowls'],
    ].map(([title, url], i) => `
      <article class="recipe-card">
        <img src="/recipes/${i}.jpg" alt="${title}">
        <h2><a href="${url}">${title}</a></h2>
        <div class="summary">
          <span>30 mins</span>
          <span>${4 + i} ingredients</span>
        </div>
      </article>
    `).join('');

    const faq = [
      'How spicy is it?',
      'Can I freeze it?',
      'What oil should I use?',
      'Can I substitute tofu?',
    ].map(question => `
      <details class="faq-item">
        <summary>${question}</summary>
        <div>Detailed answer with enough words to look tempting to a heuristic.</div>
      </details>
    `).join('');

    const html = `<html><body>
      <main class="recipes">${recipes}</main>
      <aside class="faq">${faq}</aside>
    </body></html>`;

    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.ok(titlesOf(r).includes('Crispy Chili Oil Noodles'));
    assert.ok(!titlesOf(r).includes('How spicy is it?'));
  });

  test('prefers headline links over source links in news-aggregator cards', () => {
    const stories = [
      ['Database vendor open-sources query planner', '/story/701', 'dbvendor.example.com'],
      ['Benchmark shows static extraction gains', '/story/702', 'bench.example.com'],
      ['Research note on repeated-record detection', '/story/703', 'papers.example.org'],
      ['Open dataset for adversarial DOM pages', '/story/704', 'datasets.example.net'],
    ].map(([title, url, source], i) => `
      <article class="story-card">
        <h2><a href="${url}">${title}</a></h2>
        <div class="meta">
          <a href="https://${source}/very/long/source/link/${i}">${source} / research / web / extraction / feed</a>
          <span>${100 + i} points</span>
          <span>${20 + i} comments</span>
        </div>
      </article>
    `).join('');

    const html = `<html><body><section class="news-feed">${stories}</section></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(urlsOf(r), ['/story/701', '/story/702', '/story/703', '/story/704']);
  });
});

describe('codex: tricky DOM structures that can confuse heuristics', () => {
  test('detects nested product cards inside repeated department shelves rather than the shelves themselves', () => {
    const departments = [
      ['Audio', 0],
      ['Lighting', 2],
      ['Travel', 4],
    ].map(([name, start]) => `
      <section class="department">
        <header><h2>${name}</h2><a href="/department/${name.toLowerCase()}">View all</a></header>
        <ul class="carousel">
          <li class="product-card"><h3><a href="/sku/${start + 1}">${name} Item ${start + 1}</a></h3><p>Short description for ${name} item ${start + 1}</p></li>
          <li class="product-card"><h3><a href="/sku/${start + 2}">${name} Item ${start + 2}</a></h3><p>Short description for ${name} item ${start + 2}</p></li>
        </ul>
      </section>
    `).join('');

    const html = `<html><body><main class="homepage">${departments}</main></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 6);
    assert.ok(titlesOf(r).includes('Audio Item 1'));
    assert.ok(!titlesOf(r).includes('Audio'));
  });

  test('groups mixed-size cards with one featured item and three regular items', () => {
    const html = `<html><body>
      <section class="discover">
        <article class="listing-card featured">
          <div class="gallery"><img src="/feat.jpg"><img src="/feat2.jpg"></div>
          <h2><a href="/listing/featured">Featured Hideaway</a></h2>
          <p>This featured card has a much longer description, more badges, and more child nodes than the regular cards.</p>
          <div class="badges"><span>Guest favorite</span><span>Rare find</span><span>Instant book</span></div>
        </article>
        <article class="listing-card"><h2><a href="/listing/1">Canal Studio</a></h2><p>Compact but complete description.</p></article>
        <article class="listing-card"><h2><a href="/listing/2">Forest Cabin</a></h2><p>Compact but complete description.</p></article>
        <article class="listing-card"><h2><a href="/listing/3">City Penthouse</a></h2><p>Compact but complete description.</p></article>
      </section>
    </body></html>`;

    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(titlesOf(r), [
      'Featured Hideaway',
      'Canal Studio',
      'Forest Cabin',
      'City Penthouse',
    ]);
  });

  test('prefers the visible tab panel over hidden sibling panels with the same repeated structure', () => {
    const active = Array.from({ length: 4 }, (_, i) => `
      <article class="release-card">
        <h3><a href="/releases/active-${i}">Active Release ${i}</a></h3>
        <p>Visible release notes for active tab ${i}.</p>
      </article>
    `).join('');

    const archived = Array.from({ length: 4 }, (_, i) => `
      <article class="release-card">
        <h3><a href="/releases/archived-${i}">Archived Release ${i}</a></h3>
        <p>Hidden archived release notes for tab ${i}.</p>
      </article>
    `).join('');

    const html = `<html><body>
      <div class="tabs">
        <button aria-selected="true">Current</button>
        <button aria-selected="false">Archived</button>
      </div>
      <section role="tabpanel" id="current-panel">${active}</section>
      <section role="tabpanel" id="archived-panel" hidden>${archived}</section>
    </body></html>`;

    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(titlesOf(r), [
      'Active Release 0',
      'Active Release 1',
      'Active Release 2',
      'Active Release 3',
    ]);
  });

  test('extracts top-level comments without collapsing nested replies into the main record set', () => {
    const comments = Array.from({ length: 4 }, (_, i) => `
      <article class="comment">
        <h3><a href="/discussion/thread-1#comment-${i + 1}">Comment ${i + 1}</a></h3>
        <p>Top-level comment body ${i + 1} with enough text to dominate the vote controls and metadata.</p>
        <div class="replies">
          <article class="comment reply"><a href="/discussion/thread-1#reply-${i + 1}-a">Nested reply A for ${i + 1}</a></article>
          <article class="comment reply"><a href="/discussion/thread-1#reply-${i + 1}-b">Nested reply B for ${i + 1}</a></article>
        </div>
      </article>
    `).join('');

    const html = `<html><body><section class="thread">${comments}</section></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(titlesOf(r), ['Comment 1', 'Comment 2', 'Comment 3', 'Comment 4']);
    assert.ok(!titlesOf(r).some(title => title && title.includes('Nested reply')));
  });

  test('supports records split across dt and dd siblings inside a definition list', () => {
    const entries = [
      ['Pasta alla Norma', '/recipes/pasta-norma', '$18'],
      ['Grilled Halloumi Salad', '/recipes/halloumi-salad', '$16'],
      ['Roasted Eggplant Dip', '/recipes/eggplant-dip', '$12'],
      ['Lemon Olive Oil Cake', '/recipes/olive-oil-cake', '$9'],
    ].map(([title, url, price]) => `
      <div class="menu-pair">
        <dt><a href="${url}">${title}</a></dt>
        <dd>
          <span class="price">${price}</span>
          <span>Seasonal special with enough descriptive text for extraction.</span>
        </dd>
      </div>
    `).join('');

    const html = `<html><body><dl class="menu-list">${entries}</dl></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(titlesOf(r), [
      'Pasta alla Norma',
      'Grilled Halloumi Salad',
      'Roasted Eggplant Dip',
      'Lemon Olive Oil Cake',
    ]);
    assert.deepEqual(r.items.map(item => item.price), ['$18', '$16', '$12', '$9']);
  });
});

describe('codex: adversarial visibility and title-selection cases', () => {
  test('ignores display:none metadata embedded inside otherwise valid event cards', () => {
    const cards = Array.from({ length: 4 }, (_, i) => `
      <article class="event-card">
        <h2><a href="/events/${i + 1}">Conference Session ${i + 1}</a></h2>
        <span style="display:none">Internal SKU INVISIBLE-${900 + i}</span>
        <p>Public summary for conference session ${i + 1} with enough visible content.</p>
      </article>
    `).join('');

    const html = `<html><body><section class="events">${cards}</section></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    for (const item of r.items) {
      const blob = [item.title, item.text, ...itemFields(item)].filter(Boolean).join(' ');
      assert.doesNotMatch(blob, /INVISIBLE-9\d\d/);
    }
  });

  test('ignores aria-hidden promotional links that are longer than the real heading link', () => {
    const cards = Array.from({ length: 4 }, (_, i) => `
      <article class="release-note">
        <h2><a href="/release/${i + 1}">v2.${i + 1}</a></h2>
        <div aria-hidden="true">
          <a href="/promo/${i + 1}">This hidden promotional link is intentionally much longer than the visible version number title</a>
        </div>
        <p>Visible release details for v2.${i + 1} with enough surrounding text.</p>
      </article>
    `).join('');

    const html = `<html><body><section class="releases">${cards}</section></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(urlsOf(r), ['/release/1', '/release/2', '/release/3', '/release/4']);
    assert.deepEqual(titlesOf(r), ['v2.1', 'v2.2', 'v2.3', 'v2.4']);
  });

  test('does not let metadata-heavy issue cards override short titles', () => {
    const issues = Array.from({ length: 4 }, (_, i) => `
      <article class="issue-card">
        <h3><a href="/short/${i + 1}">Bug ${i + 1}</a></h3>
        <div class="metadata">
          <a href="/labels/high-priority">high-priority</a>
          <a href="/labels/regression">regression</a>
          <a href="/labels/customer-impact">customer-impact</a>
          <a href="/milestone/q2">milestone q2</a>
          <a href="/team/runtime">runtime team</a>
          <span>opened yesterday by qa-${i + 1}</span>
        </div>
        <p>Minimal body text for issue ${i + 1}.</p>
      </article>
    `).join('');

    const html = `<html><body><main class="issue-grid">${issues}</main></body></html>`;
    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.deepEqual(titlesOf(r), ['Bug 1', 'Bug 2', 'Bug 3', 'Bug 4']);
    assert.deepEqual(urlsOf(r), ['/short/1', '/short/2', '/short/3', '/short/4']);
  });

  test('ignores repeated header rows in data tables and extracts only tbody records', () => {
    const bodyRows = Array.from({ length: 4 }, (_, i) => `
      <tr class="build-row">
        <td>${i + 1}</td>
        <td><a href="/builds/${i + 1}">Build ${i + 1}</a></td>
        <td>${i % 2 === 0 ? 'green' : 'red'}</td>
      </tr>
    `).join('');

    const html = `<html><body>
      <table class="build-table">
        <thead>
          <tr><th>Rank</th><th>Build</th><th>Status</th></tr>
          <tr><th colspan="3">Latest pipeline results</th></tr>
        </thead>
        <tbody>${bodyRows}</tbody>
      </table>
    </body></html>`;

    const r = extractFromHTML(html);
    assert.equal(r.count, 4);
    assert.match(r.pattern, /TR/i);
    assert.deepEqual(titlesOf(r), ['Build 1', 'Build 2', 'Build 3', 'Build 4']);
    assert.ok(!titlesOf(r).includes('Rank'));
  });

  test('preserves full count under limit on a complex page with sidebars and pagination noise', () => {
    const stories = Array.from({ length: 7 }, (_, i) => `
      <article class="story-card">
        <h2><a href="/deep-story/${i + 1}">Deep Story ${i + 1}</a></h2>
        <p>Story ${i + 1} body with enough detail to survive extraction in the presence of many noisy sibling lists.</p>
        <time datetime="2026-03-${10 + i}">Mar ${10 + i}</time>
      </article>
    `).join('');

    const html = `<html><body>
      <aside class="left-rail">
        <ul>
          <li><a href="/topic/testing">Testing</a></li>
          <li><a href="/topic/html">HTML</a></li>
          <li><a href="/topic/js">JavaScript</a></li>
          <li><a href="/topic/rss">RSS</a></li>
        </ul>
      </aside>
      <main class="river">${stories}</main>
      <footer class="pagination">
        <a href="/page/1">1</a>
        <a href="/page/2">2</a>
        <a href="/page/3">3</a>
        <a href="/page/next">Next</a>
      </footer>
    </body></html>`;

    const r = extractFromHTML(html, 3);
    assert.equal(r.count, 7);
    assert.equal(r.items.length, 3);
    assert.deepEqual(urlsOf(r), ['/deep-story/1', '/deep-story/2', '/deep-story/3']);
  });
});
