#!/usr/bin/env python3
"""Local reference application with paginated invoices and a searchable draft form."""

import argparse
import html
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import socket
import threading
import time
from urllib.parse import parse_qs, urlsplit


LEDGER = {
    "2026-08": [("INV-801", 1250), ("INV-802", 725), ("INV-803", 3100), ("INV-804", 450), ("INV-805", 999)],
    "2026-09": [("INV-901", 4200), ("INV-902", 350), ("INV-903", 875)],
}
SCENARIOS = ("normal", "delayed", "overlap", "conflict", "repeat-page", "repeated-cursor",
             "no-progress", "changed-revision", "truncated", "empty", "wrong-account",
             "wrong-row-account", "invalid-amount", "expired-session", "disconnect",
             "lost-response", "delayed-commit", "late-commit", "missing-draft", "duplicate-draft", "wrong-fields",
             "lookup-unavailable", "duplicate-submit", "transport-retry")

INVOICES = """<h1>Invoices</h1><label>Page <input id="page" readonly value="0"></label>
<table id="invoices"><thead><tr><th>Invoice</th><th>Amount (EUR)</th></tr></thead><tbody></tbody></table>
<button id="next" disabled>Next page</button><p id="error" role="status"></p>
<script>
const period = new URL(location).searchParams.get('period');
const table = document.querySelector('#invoices'), page = document.querySelector('#page');
const next = document.querySelector('#next');
async function load(number) {
  page.value = '0'; next.disabled = true;
  try {
    const response = await fetch('/invoice-data?period='+encodeURIComponent(period)+'&page='+number);
    if (!response.ok) throw new Error('Invoice request failed');
    const data = await response.json();
    if (data.redirect) { location.href = data.redirect; return; }
    Object.assign(table.dataset, {period:data.period,revision:data.revision,total:String(data.total),next:data.next});
    table.querySelector('tbody').replaceChildren();
    for (const item of data.rows) {
      const row = document.createElement('tr');
      Object.assign(row.dataset, {id:item.id,account:item.account,period:item.period,amountCents:String(item.amount_cents),currency:item.currency});
      for (const value of [item.id, (Number(item.amount_cents)/100).toFixed(2)]) {
        const cell = document.createElement('td'); cell.textContent = value; row.appendChild(cell);
      }
      table.querySelector('tbody').appendChild(row);
    }
    page.value = String(data.page); next.disabled = data.next === '';
  } catch (_) { document.querySelector('#error').textContent = 'Invoice response unavailable'; }
}
next.addEventListener('click', () => load(table.dataset.next));
load(1);
</script>"""

DRAFT_FORM = """<h1>New draft</h1>
<label>Reference <input id="reference"></label><label>Title <input id="title"></label>
<label>Amount (EUR) <input id="amount"></label><button id="create">Create draft</button>
<p id="status" role="status"></p>
<script>
document.querySelector('#create').addEventListener('click', async () => {
  document.querySelector('#create').disabled = true;
  const value = id => document.querySelector(id).value;
  try {
    const response = await fetch('/draft-data', {method:'POST',headers:{'Content-Type':'application/json'},
      body:JSON.stringify({reference:value('#reference'),title:value('#title'),amount:value('#amount')})});
    if (!response.ok) throw new Error('Create failed');
    const draft = await response.json();
    document.querySelector('#status').textContent = 'Draft saved: '+draft.id;
  } catch (_) { document.querySelector('#status').textContent = 'Creation response unavailable'; }
});
</script>"""

SEARCH = """<h1>Search drafts</h1><label>Reference <input id="query" readonly></label>
<input id="search-state" readonly value="loading" aria-label="Search state">
<div id="results"></div><p id="error" role="status"></p>
<script>
const query = new URL(location).searchParams.get('reference');
document.querySelector('#query').value = query;
fetch('/draft-data?reference='+encodeURIComponent(query)).then(response => {
  if (!response.ok) throw new Error('Search failed'); return response.json();
}).then(data => {
  const results = document.querySelector('#results');
  results.dataset.total = String(data.total); results.dataset.reference = data.reference;
  for (const draft of data.items) {
    const card = document.createElement('article'); card.className = 'draft';
    for (const key of ['id','account','reference','title','currency','status']) card.dataset[key] = draft[key];
    card.dataset.amountCents = String(draft.amount_cents);
    card.textContent = draft.id+' — '+draft.title+' — '+(draft.amount_cents/100).toFixed(2)+' EUR';
    results.appendChild(card);
  }
  document.querySelector('#search-state').value = 'ready';
}).catch(() => { document.querySelector('#search-state').value = 'error'; });
</script>"""


class DemoServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, port, account, scenario, page_size):
        super().__init__(("127.0.0.1", port), Handler)
        self.account, self.scenario, self.page_size = account, scenario, page_size
        self.lock = threading.Lock()
        self.page_requests, self.creations, self.drafts = [], [], []


class Handler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass

    def send(self, body, status=200, content_type="application/json"):
        if not isinstance(body, str):
            body = json.dumps(body)
        body = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def disconnect(self):
        self.close_connection = True
        try:
            self.connection.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        self.connection.close()

    def lose_response_body(self):
        # A partial response prevents Chrome's own retry of a request that received no bytes.
        # The transport-retry scenario below deliberately preserves that separate failure.
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", "999")
        self.end_headers()
        self.wfile.write(b'{"id":')
        self.wfile.flush()
        self.disconnect()

    def page(self, body):
        account = "other" if self.server.scenario == "wrong-account" else self.server.account
        self.send('<!doctype html><html lang="en"><meta charset="utf-8"><title>Workflow demo</title>'
                  '<style>body{font:16px system-ui;max-width:54rem;margin:3rem auto}label{display:block;margin:1rem 0}'
                  'input,button{font:inherit;padding:.4rem}td,th{text-align:left;padding:.5rem}</style>'
                  '<label>Account <input id="account" readonly value="' + html.escape(account, quote=True) + '"></label>'
                  + body + '</html>', content_type="text/html; charset=utf-8")

    def do_GET(self):
        path = urlsplit(self.path)
        query = parse_qs(path.query)
        if path.path == "/invoices":
            self.page(INVOICES)
        elif path.path == "/drafts/new":
            extra = '<button id="create">Another create</button>' if self.server.scenario == "duplicate-submit" else ''
            self.page(DRAFT_FORM + extra)
        elif path.path == "/drafts/search":
            self.page(SEARCH)
        elif path.path == "/login":
            self.send("<h1>Sign in</h1>", content_type="text/html")
        elif path.path == "/invoice-data":
            self.invoices(query)
        elif path.path == "/draft-data":
            reference = query.get("reference", [""])[0]
            with self.server.lock:
                unavailable = self.server.scenario == "lookup-unavailable" and bool(self.server.creations)
                items = [dict(row) for row in self.server.drafts if row["reference"] == reference]
            if unavailable:
                self.disconnect()
            else:
                self.send(dict(reference=reference, total=len(items), items=items))
        elif path.path == "/state":
            # Independent test oracle, never called by either task script.
            with self.server.lock:
                self.send(dict(pages=self.server.page_requests, creations=self.server.creations, drafts=self.server.drafts))
        else:
            self.send("Not found", 404, "text/plain")

    def invoices(self, query):
        period = query.get("period", [""])[0]
        try:
            requested = int(query.get("page", ["1"])[0])
            if not 1 <= requested <= 1000 or period not in LEDGER:
                raise ValueError()
        except ValueError:
            self.send("Invalid page or period", 400)
            return
        scenario = self.server.scenario
        with self.server.lock:
            self.server.page_requests.append(dict(page=requested, period=period))
        if scenario == "delayed":
            time.sleep(0.6)
        if requested > 1 and scenario == "disconnect":
            self.disconnect()
            return
        if requested > 1 and scenario == "expired-session":
            self.send(dict(redirect="/login"))
            return
        page = 1 if requested > 1 and scenario == "repeat-page" else requested
        ledger = [] if scenario == "empty" else LEDGER[period]
        offset, size = (page - 1) * self.server.page_size, self.server.page_size
        rows = [dict(id=key, account=self.server.account, period=period, amount_cents=amount, currency="EUR")
                for key, amount in ledger]
        current = rows[offset:offset + size]
        next_page = str(page + 1) if offset + size < len(rows) else ""
        if page > 1 and scenario in ("overlap", "conflict"):
            overlap = dict(rows[offset - 1])
            if scenario == "conflict":
                overlap["amount_cents"] += 1
            current = [overlap] + current
        if page > 1 and scenario == "no-progress":
            current = rows[:size]
        if scenario == "repeated-cursor":
            next_page = "1"
        if page == 2 and scenario == "truncated":
            next_page = ""
        if page > 1 and current and scenario == "wrong-row-account":
            current[0] = dict(current[0], account="other")
        if page > 1 and current and scenario == "invalid-amount":
            current[0] = dict(current[0], amount_cents="NaN")
        revision = "r2" if page > 1 and scenario == "changed-revision" else "r1"
        self.send(dict(page=page, period=period, total=len(rows), revision=revision, next=next_page, rows=current))

    def do_POST(self):
        if self.path != "/draft-data":
            self.send("Not found", 404)
            return
        try:
            size = int(self.headers.get("Content-Length", "0"))
            if not 0 < size <= 4096:
                raise ValueError()
            values = json.loads(self.rfile.read(size))
            amount = values["amount"]
            whole, fraction = amount.split(".")
            amount_cents = int(whole) * 100 + int(fraction)
            if len(fraction) != 2 or amount_cents < 0:
                raise ValueError()
            reference, title = values["reference"], values["title"]
            if not isinstance(reference, str) or not isinstance(title, str):
                raise ValueError()
        except (ValueError, KeyError, TypeError):
            self.send("Invalid draft", 400)
            return
        with self.server.lock:
            self.server.creations.append(dict(reference=reference, title=title, amount_cents=amount_cents))
            sequence = len(self.server.creations)
        scenario = self.server.scenario
        draft = dict(id=f"DRAFT-{sequence}", account=self.server.account, reference=reference,
                     title=title + " changed" if scenario == "wrong-fields" else title,
                     amount_cents=amount_cents, currency="EUR", status="draft")

        def commit():
            with self.server.lock:
                self.server.drafts.append(draft)
                if scenario == "duplicate-draft":
                    self.server.drafts.append(dict(draft, id=f"DRAFT-{sequence}-duplicate"))

        if scenario in ("delayed-commit", "late-commit"):
            def later():
                time.sleep(4 if scenario == "late-commit" else 1.5)
                commit()
            threading.Thread(target=later, daemon=True).start()
        elif scenario != "missing-draft":
            commit()
        if scenario in ("lost-response", "delayed-commit", "late-commit", "missing-draft"):
            self.lose_response_body()
        elif scenario == "transport-retry":
            self.disconnect()
        else:
            self.send(draft)


if __name__ == "__main__":
    cli = argparse.ArgumentParser(description=__doc__)
    cli.add_argument("--port", type=int, default=0)
    cli.add_argument("--account", default="acme")
    cli.add_argument("--scenario", choices=SCENARIOS, default="normal")
    cli.add_argument("--page-size", type=int, choices=range(1, 6), default=2)
    args = cli.parse_args()
    server = DemoServer(args.port, args.account, args.scenario, args.page_size)
    print(json.dumps({"url": f"http://127.0.0.1:{server.server_port}"}), flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
