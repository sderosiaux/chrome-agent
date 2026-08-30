---
name: scrape-structured-data
description: Get the repeating records off a web page (product grids, search results, job listings, news feeds, tables) as JSON, without writing CSS selectors and without spending a model call to read the HTML. Works on sites with no API, including ones behind a login or bot protection. Runs locally, one binary, no API key. Use when the user says scrape, extract the list of, get all the products/results/posts from, turn this page into JSON, or asks for data from a site that has no API.
metadata:
  author: sderosiaux
  version: "0.16.0"
  tags: ["scraping", "extract", "data", "json", "web"]
---

# Scrape structured data from a page

`chrome-agent extract` finds the repeating record pattern on a page and returns it as JSON. No
selectors to write, no HTML read into the model to find them.

```bash
which chrome-agent || npm install -g chrome-agent
chrome-agent goto https://news.ycombinator.com
chrome-agent --json extract --limit 50
```

```json
{"count":30,"pattern":"TR","items":[
  {"title":"PGSimCity - How PostgreSQL Works","url":"https://nikolays.github.io/PGSimCity/","fields":["..."]},
  ...
]}
```

Check `count` against what the page shows. Wildly off means the page has more than one repeating
pattern — scope it with `--selector`.

## Why extract rather than read the page

Measured on the Hacker News front page. All three contain the same 30 stories; only the first
hands them over as records.

| approach | tokens | what you get |
|---|---|---|
| `extract --limit 30` | ~1,570 | all 30 stories as records with URLs |
| `inspect` (accessibility tree) | ~5,650 | the tree, stories mixed into the page chrome |
| raw HTML | ~8,730 | everything, including markup you'll never read |

3.6x against the tree, 5.6x against raw HTML here. The margin narrows on pages that are nothing
but a list: on a blog archive, ~12,500 tokens against ~16,100 for the tree. The win comes from
pages where the records sit inside a lot of other markup.

## Scoping, lazy loading, infinite scroll

```bash
chrome-agent extract --selector ".product-grid"     # restrict to one container
chrome-agent extract --limit 100                    # default is 10
chrome-agent extract --scroll                       # scrolls, waits for new nodes, then extracts
chrome-agent extract --a11y --scroll --limit 50     # React/SPA feeds where the DOM is noise
```

`--a11y` reads the accessibility tree instead of the DOM. Use it when a site renders into nested
generated `<div>`s (X.com, most React apps) and plain `extract` returns junk.

## When extract returns nothing

A page with no repeating pattern gives an empty list and a hint — the correct answer for an
article or a landing page. Fall back in this order:

```bash
chrome-agent network --filter "api"        # the page called an API: take the payload instead
chrome-agent read                          # articles, blog posts, via Mozilla Readability
chrome-agent text --selector "main"        # scoped visible text
chrome-agent eval "JSON.stringify([...document.querySelectorAll('.x')].map(e=>e.textContent))"
```

Try `network` early on any site that loads its data over XHR: the JSON the page fetched is cleaner
than anything scraped out of the rendered DOM.

## Logins and bot protection

```bash
chrome-agent --copy-cookies goto https://app.example.com/reports
chrome-agent --stealth goto https://example.com     # Cloudflare JS challenge
chrome-agent --connect http://127.0.0.1:9222 goto https://example.com   # DataDome, Kasada
```

`--copy-cookies` reads cookies from your real Chrome profile, so anything you are already logged
into works. On macOS the OS prompts for Keychain access; that prompt is the consent step and
cannot be skipped silently.

For `--connect`, the user launches their own Chrome first with
`google-chrome --remote-debugging-port=9222`. A real browser with a real fingerprint is what the
hardest protections check.

## Output and limits

`--json` gives `{"ok":true,"count":N,"pattern":"...","items":[...]}`. Each item has `title`, `url`
when there is one, and `fields` with the rest of the row's text. Errors exit 1 and still print JSON
on stdout with an `error` and a `hint`.

- `extract` finds *one* pattern, the highest-scoring one. Two equally strong lists need
  `--selector` to pick.
- It reads what is rendered. Content that appears only on hover or after a click is not there
  until you click it.
- Record fields are positional text, not a typed schema. For specific attributes, `eval` with a
  selector is the honest tool.
- Clicking a download link does not work here: get the href with `inspect --urls` and pass the URL
  to `chrome-agent download`.
