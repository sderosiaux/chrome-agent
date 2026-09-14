import json
from html.parser import HTMLParser
from urllib.parse import quote, urljoin

from bridge import browser

MAX_PAGES = 200
MAX_FALLBACK_CHARS = 400000


class Result(dict):
    """Mapping that also exposes .complete / .articles / .notes attributes."""

    def __init__(self, complete, articles, notes):
        super().__init__(complete=complete, articles=articles, notes=notes)
        self.complete = complete
        self.articles = articles
        self.notes = notes


class _RowParser(HTMLParser):
    """Collect table rows as lists of cell strings (no regex escaping needed)."""

    def __init__(self):
        HTMLParser.__init__(self, convert_charrefs=True)
        self.rows = []
        self._row = None
        self._cell = None

    def handle_starttag(self, tag, attrs):
        if tag == "tr":
            self._row = []
            self._cell = None
        elif tag in ("td", "th") and self._row is not None:
            self._cell = []

    def handle_endtag(self, tag):
        if tag in ("td", "th") and self._cell is not None and self._row is not None:
            self._row.append(" ".join("".join(self._cell).split()))
            self._cell = None
        elif tag == "tr" and self._row is not None:
            self.rows.append(self._row)
            self._row = None
            self._cell = None

    def handle_data(self, data):
        if self._cell is not None:
            self._cell.append(data)


def _text(selector=None):
    cmd = {"cmd": "text"}
    if selector:
        cmd["selector"] = selector
    resp = browser(cmd) or {}
    return resp.get("text") or ""


def _json_body(blob):
    start = blob.find("{")
    end = blob.rfind("}")
    if start < 0 or end <= start:
        return None
    try:
        return json.loads(blob[start:end + 1])
    except ValueError:
        return None


def _quoted_after(text, token):
    """Return the first quoted literal that follows token, or None."""
    i = text.find(token)
    if i < 0:
        return None
    j = i + len(token)
    while j < len(text) and text[j] in " \t\r\n=(":
        j += 1
    if j >= len(text) or text[j] not in "\"'":
        return None
    quote_char = text[j]
    k = text.find(quote_char, j + 1)
    if k < 0:
        return None
    value = text[j + 1:k]
    return value or None


def _is_iso_date(value):
    if len(value) != 10 or value[4] != "-" or value[7] != "-":
        return False
    digits = value[0:4] + value[5:7] + value[8:10]
    return digits.isdigit()


def _row_state(row, section, since):
    """Return (matches_section, is_older_than_since)."""
    sec = str(row.get("section", "") or "").strip()
    date = str(row.get("date", "") or "").strip()
    in_section = (not section) or sec.lower() == section.strip().lower()
    older = bool(since and date and date < since)
    return in_section, older


def _make_article(base, article_prefix, rid, row):
    return {
        "id": rid,
        "title": str(row.get("title", "") or "").strip(),
        "date": str(row.get("date", "") or "").strip(),
        "section": str(row.get("section", "") or "").strip(),
        "url": urljoin(base, article_prefix + quote(rid, safe="")),
    }


def _scrape_rendered_table(base, section, since, article_prefix):
    """Last resort: read the rows already rendered (cannot paginate)."""
    resp = browser({"cmd": "read", "html": True}) or {}
    html = resp.get("content") or ""
    if len(html) > MAX_FALLBACK_CHARS:
        html = html[:MAX_FALLBACK_CHARS]
    parser = _RowParser()
    try:
        parser.feed(html)
        parser.close()
    except Exception:
        return []
    out = []
    seen = set()
    for cells in parser.rows:
        if len(cells) < 4:
            continue
        rid, title, date, sec = cells[0], cells[1], cells[2], cells[3]
        if not _is_iso_date(date) or not rid or rid in seen:
            continue
        row = {"id": rid, "title": title, "date": date, "section": sec}
        in_section, older = _row_state(row, section, since)
        if in_section and not older:
            seen.add(rid)
            out.append(_make_article(base, article_prefix, rid, row))
    return out


def run(inputs):
    inputs = inputs or {}
    base = inputs.get("url") or ""
    section = (inputs.get("section") or "").strip()
    since = (inputs.get("since") or "").strip()
    notes = []

    browser({"cmd": "goto", "url": base, "inspect": True})
    try:
        browser({"cmd": "assert", "what": "exists", "selector": "table tbody tr",
                 "min": 1, "within": 5})
    except Exception:
        notes.append("archive rows did not render within 5s")

    heading = _text("h1").strip()
    if section and heading and heading.lower() != section.lower():
        notes.append("entry page heading %r does not match requested section %r; "
                     "rows are filtered by their own section field" % (heading, section))

    # The listing is client-rendered: an inline script carries the seed cursor,
    # the feed endpoint and the article URL prefix. Discover all three.
    script = _text("script")
    cursor = _quoted_after(script, "cursor=")
    feed_prefix = _quoted_after(script, "fetch")
    article_prefix = "/article/" if "/article/" in script else None
    if article_prefix is None:
        article_prefix = _quoted_after(script, "href") or "/article/"

    if not cursor or not feed_prefix:
        rows = _scrape_rendered_table(base, section, since, article_prefix)
        notes.append("could not locate the feed cursor/endpoint in page scripts; "
                     "only the first rendered page could be read")
        return Result(False, rows, notes)

    articles = []
    seen = set()
    seen_cursors = set()
    complete = False
    pages = 0

    while cursor and pages < MAX_PAGES:
        if str(cursor) in seen_cursors:
            notes.append("cursor repeated; aborted to avoid a pagination loop")
            break
        seen_cursors.add(str(cursor))
        pages += 1

        url = urljoin(base, feed_prefix + quote(str(cursor), safe=""))
        browser({"cmd": "goto", "url": url, "inspect": False})
        data = _json_body(_text())
        if not isinstance(data, dict) or not isinstance(data.get("rows"), list):
            notes.append("unparseable feed payload on feed request %d" % pages)
            break

        rows = data["rows"]
        older_count = 0
        for row in rows:
            if not isinstance(row, dict):
                continue
            rid = str(row.get("id", "") or "").strip()
            in_section, older = _row_state(row, section, since)
            if older:
                older_count += 1
            if not in_section or older or not rid or rid in seen:
                continue
            seen.add(rid)
            articles.append(_make_article(base, article_prefix, rid, row))

        nxt = data.get("cursor")
        if not nxt:
            complete = True
            break
        if since and rows and older_count == len(rows):
            # Feed is ordered newest-first: a page entirely below the bound ends it.
            complete = True
            break
        cursor = nxt

    if not complete and pages >= MAX_PAGES:
        notes.append("stopped after MAX_PAGES=%d feed requests; more pages may remain" % MAX_PAGES)

    if not articles:
        notes.append("no rows matched section=%r since=%r" % (section, since))
        complete = False

    articles.sort(key=lambda a: (a["date"], a["id"]), reverse=True)
    return Result(complete, articles, notes)
