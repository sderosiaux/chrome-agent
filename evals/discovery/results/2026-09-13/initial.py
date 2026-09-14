import json
import re
from urllib.parse import quote, urljoin

from bridge import browser

MAX_PAGES = 200


class Result(dict):
    """Mapping that also exposes .complete / .articles / .notes attributes."""

    def __init__(self, complete, articles, notes):
        super().__init__(complete=complete, articles=articles, notes=notes)
        self.complete = complete
        self.articles = articles
        self.notes = notes


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


def _row_state(row, section, since):
    """Return (matches_section, is_older_than_since)."""
    sec = str(row.get("section", "") or "").strip()
    date = str(row.get("date", "") or "").strip()
    in_section = (not section) or sec.lower() == section.strip().lower()
    older = bool(since and date and date < since)
    return in_section, older


def _scrape_rendered_table(base, section, since, article_prefix):
    """Last-resort: read whatever rows are already rendered (cannot paginate)."""
    resp = browser({"cmd": "read", "html": True}) or {}
    html = resp.get("content") or ""
    out = []
    for tr in re.findall(r"<tr[^>]*>(.*?)</tr>", html, re.S | re.I):
        cells = re.findall(r"<t[dh][^>]*>(.*?)</t[dh]>", tr, re.S | re.I)
        if len(cells) < 4:
            continue
        vals = [re.sub(r"\\s+", " ", re.sub(r"<[^>]+>", " ", c)).strip() for c in cells]
        rid, title, date, sec = vals[0], vals[1], vals[2], vals[3]
        if not re.match(r"^\\d{4}-\\d{2}-\\d{2}$", date):
            continue
        row = {"id": rid, "title": title, "date": date, "section": sec}
        in_section, older = _row_state(row, section, since)
        if in_section and not older and rid:
            row["url"] = urljoin(base, article_prefix + quote(rid, safe=""))
            out.append(row)
    return out


def run(inputs):
    base = (inputs or {}).get("url") or ""
    section = ((inputs or {}).get("section") or "").strip()
    since = ((inputs or {}).get("since") or "").strip()
    notes = []

    browser({"cmd": "goto", "url": base, "inspect": True})
    try:
        browser({"cmd": "assert", "what": "exists", "selector": "table tbody tr",
                 "min": 1, "within": 5})
    except Exception:
        notes.append("archive rows did not render within 5s")

    # The listing is client-rendered: an inline script holds the seed cursor and
    # the feed endpoint. Discover both instead of hardcoding them.
    script = _text("script")
    cur_m = re.search(r"cursor\\s*=\\s*[\"']([^\"']+)[\"']", script)
    feed_m = re.search(r"fetch\\(\\s*[\"']([^\"']+)[\"']", script)
    art_m = re.search(r"[\"']([^\"']*/article/)[\"']\\s*\\+", script)
    article_prefix = art_m.group(1) if art_m else "/article/"

    if not cur_m or not feed_m:
        rows = _scrape_rendered_table(base, section, since, article_prefix)
        notes.append("could not locate the feed cursor/endpoint in page scripts; "
                     "only the first rendered page could be read")
        return Result(False, rows, notes)

    feed_prefix = feed_m.group(1)
    cursor = cur_m.group(1)

    articles = []
    seen = set()
    complete = False
    pages = 0

    while cursor and pages < MAX_PAGES:
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
            articles.append({
                "id": rid,
                "title": str(row.get("title", "") or "").strip(),
                "date": str(row.get("date", "") or "").strip(),
                "section": str(row.get("section", "") or "").strip(),
                "url": urljoin(base, article_prefix + quote(rid, safe="")),
            })

        nxt = data.get("cursor")
        if not nxt:
            complete = True
            break
        if since and rows and older_count == len(rows):
            # Feed is ordered newest-first; an entire page below the bound ends it.
            complete = True
            break
        if str(nxt) == str(cursor):
            notes.append("cursor stopped advancing; aborted to avoid a loop")
            break
        cursor = nxt

    if not complete and pages >= MAX_PAGES:
        notes.append("stopped after MAX_PAGES=%d feed requests; more pages may remain" % MAX_PAGES)

    if not articles:
        notes.append("no rows matched section=%r since=%r" % (section, since))
        complete = False

    articles.sort(key=lambda a: (a["date"], a["id"]), reverse=True)
    return Result(complete, articles, notes)
