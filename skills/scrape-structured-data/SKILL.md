---
name: scrape-structured-data
description: Get the repeating records off a web page (product grids, search results, job listings, news feeds, tables) as JSON, without writing CSS selectors and without spending a model call to read the HTML. Works on sites with no API, including ones behind a login or bot protection. Runs locally, one binary, no API key. Use when the user says scrape, extract the list of, get all the products/results/posts from, turn this page into JSON, or asks for data from a site that has no API.
metadata:
  author: sderosiaux
  version: "0.8.0"
  tags: ["scraping", "extract", "data", "json", "web"]
---

# Scrape structured data from a page

`chrome-agent extract` finds the repeating record pattern on a page and returns it as
JSON. You don't write selectors and you don't read the HTML into the model to find them.

## Install check

```bash
which chrome-agent || npm install -g chrome-agent
```

## The whole thing

```bash
chrome-agent goto https://news.ycombinator.com
chrome-agent --json extract --limit 50
```

```json
{"count":30,"pattern":"TR","items":[
  {"title":"PGSimCity - How PostgreSQL Works","url":"https://nikolays.github.io/PGSimCity/","fields":["..."]},
  ...
]}
```

Check `count` against what the page actually shows. If it's wildly off, the page has
more than one repeating pattern and you should scope it. See below.

## Why not just read the page

On the Hacker News front page, measured with `./scripts/measure.sh`:

| approach | tokens | what you get |
|---|---|---|
| `extract --limit 30` | ~1,570 | all 30 stories as records with URLs |
| `inspect` (accessibility tree) | ~5,650 | the tree, stories mixed into the page chrome |
| raw HTML | ~8,730 | everything, including markup you'll never read |

All three contain the same 30 stories. Only the first hands them over as records; the
others hand over the page and leave the model to find the stories in it, which costs
twice — once in input tokens, again in the reasoning to parse them.

That's 5.6x against raw HTML and 3.6x against the accessibility tree on this page. It is
not always that wide: on a blog archive that is nothing *but* a list, `extract` returns
~12,500 tokens against ~16,100 for the tree, because there's little page chrome to strip.
The win comes from pages where the records sit inside a lot of other markup.

## When extract returns nothing

It looks for a repeating pattern, so a page without one gives you an empty list and a
hint. That's the correct answer for an article or a landing page. Fall back in this order:

```bash
chrome-agent read                          # articles, blog posts, via Mozilla Readability
chrome-agent text --selector "main"        # scoped visible text
chrome-agent eval "JSON.stringify([...document.querySelectorAll('.x')].map(e=>e.textContent))"
chrome-agent network --filter "api"        # the page called an API: take the payload instead
```

`network` is worth trying early on any site that loads its data over XHR. The JSON the
page fetched is cleaner than anything scraped out of the rendered DOM.

## Scoping when a page has several patterns

```bash
chrome-agent extract --selector ".product-grid"     # restrict to one container
chrome-agent extract --limit 100                    # default is 10
```

## Lazy loading and infinite scroll

```bash
chrome-agent extract --scroll                # scrolls, waits for new nodes, then extracts
chrome-agent extract --a11y --scroll --limit 50   # React/SPA feeds where the DOM is noise
```

`--a11y` reads the accessibility tree instead of the DOM. Use it when a site renders into
nested generated `<div>`s (X.com, most React apps) and plain `extract` returns junk.

## Sites that need a login

```bash
chrome-agent --copy-cookies goto https://app.example.com/reports
chrome-agent --json extract
```

`--copy-cookies` reads cookies from your real Chrome profile, so anything you're already
logged into in your browser works here. On macOS the OS will prompt for Keychain access, and
that prompt is the consent step. It can't be skipped silently.

## Sites that block automation

```bash
chrome-agent --stealth goto https://example.com     # Cloudflare, Turnstile
chrome-agent --connect http://127.0.0.1:9222 goto https://example.com   # DataDome, Kasada
```

For `--connect`, the user launches their own Chrome first:
`google-chrome --remote-debugging-port=9222`. That's a real browser with a real
fingerprint, which is what the hardest protections check.

## Output

`--json` gives `{"ok":true,"count":N,"pattern":"...","items":[...]}`. Each item has
`title`, `url` when there is one, and `fields` with the rest of the row's text. Errors
exit 1 and still print JSON on stdout with an `error` and a `hint`.

## Limits worth knowing

- `extract` finds *one* pattern, the highest-scoring one. Pages with two equally strong
  lists need `--selector` to pick.
- It reads what's rendered. Content that only appears on hover or after a click isn't there
  until you click it.
- Record fields are positional text, not a typed schema. If you need specific attributes,
  `eval` with a selector is the honest tool.
- Clicking a download link doesn't work; get the href with `inspect --urls` and pass the
  URL to `chrome-agent download`.
