#!/usr/bin/env python3
"""Local reference app for export_report.py; no credentials or external service required."""

import argparse
import csv
import html
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import io
import json
import socket
import threading
import time
from urllib.parse import parse_qs, urlsplit


# This is the reference app's ledger, not read by the export script. Tests inspect /state
# independently and compare the published CSV against their own expected invoice identities.
LEDGER = {
    "2026-08": [("INV-801", "12.50"), ("INV-802", "7.25")],
    "2026-09": [("INV-901", "42.00"), ("INV-902", "3.50"), ("INV-903", "8.75")],
}
SCENARIOS = ("normal", "delayed", "wrong-account", "wrong-selection", "wrong-file-account",
             "wrong-file-period", "empty-file", "truncated-report", "missing-download", "disconnect",
             "expired-session", "duplicate-export")

PAGE = """<!doctype html><html lang="en"><meta charset="utf-8"><title>Report export demo</title>
<style>body {font:16px system-ui;max-width:52rem;margin:3rem auto;line-height:1.6}
label {display:block;margin:1rem 0}input,select,button {font:inherit;padding:.4rem}</style>
<h1>Invoice report</h1>
<label>Account <input id="account" readonly value="ACCOUNT"></label>
<label>Period <select id="period"><option>2026-08</option><option>2026-09</option></select></label>
<p id="report-summary" data-ready="false">Loading report…</p>
<button id="export" disabled>Export CSV</button>DUPLICATE
<p id="status" role="status"></p>
<script>
const period = document.querySelector('#period');
const summary = document.querySelector('#report-summary');
const button = document.querySelector('#export');
let generation = 0;
async function preview() {
  const ownGeneration = ++generation;
  button.disabled = true;
  summary.dataset.ready = 'false';
  if (SCENARIO === 'wrong-selection') period.value = '2026-08';
  const response = await fetch('/preview?period=' + encodeURIComponent(period.value));
  const result = await response.json();
  if (ownGeneration !== generation) return;
  Object.assign(summary.dataset, {account: result.account, period: result.period,
    rowCount: String(result.row_count), totalCents: String(result.total_cents),
    currency: result.currency, ready: 'true'});
  summary.textContent = result.row_count + ' invoices for ' + result.account + ', ' + result.period;
  button.disabled = false;
}
period.addEventListener('change', preview);
button.addEventListener('click', async () => {
  button.disabled = true;
  document.querySelector('#status').textContent = 'Export requested';
  try {
    const response = await fetch('/export', {method:'POST',
      headers:{'Content-Type':'application/json'}, body:JSON.stringify({period:period.value})});
    if (response.status === 202) return;
    if (!response.ok) throw new Error('Export failed');
    const blob = await response.blob();
    const anchor = document.createElement('a');
    anchor.href = URL.createObjectURL(blob);
    anchor.download = 'invoices.csv';
    anchor.click();
    document.querySelector('#status').textContent = 'Export ready';
  } catch (_) { document.querySelector('#status').textContent = 'Export response unavailable'; }
});
preview();
</script></html>"""


class ReportServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, port, account, scenario):
        super().__init__(("127.0.0.1", port), Handler)
        self.account = account
        self.scenario = scenario
        self.exports = []
        self.lock = threading.Lock()


class Handler(BaseHTTPRequestHandler):
    def log_message(self, format, *args):
        pass

    def send(self, body, content_type="application/json", status=200, headers=None):
        if isinstance(body, str):
            body = body.encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        for key, value in (headers or {}).items():
            self.send_header(key, value)
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        path = urlsplit(self.path)
        account = "other-account" if self.server.scenario == "wrong-account" else self.server.account
        if path.path == "/":
            if self.server.scenario == "expired-session":
                self.send("", status=302, headers={"Location": "/login"})
                return
            page = PAGE.replace("ACCOUNT", html.escape(account, quote=True))
            page = page.replace("SCENARIO", json.dumps(self.server.scenario))
            page = page.replace("DUPLICATE", '<button id="export">Another export</button>'
                                if self.server.scenario == "duplicate-export" else "")
            self.send(page, "text/html; charset=utf-8")
        elif path.path == "/login":
            self.send("<h1>Sign in</h1>", "text/html")
        elif path.path == "/preview":
            period = parse_qs(path.query).get("period", [""])[0]
            if self.server.scenario == "delayed":
                time.sleep(0.8)
            rows = LEDGER.get(period, [])
            total = sum(int(amount.replace(".", "")) for _, amount in rows)
            self.send(json.dumps({"account": account, "period": period, "row_count": len(rows),
                                  "total_cents": total, "currency": "EUR"}))
        elif path.path == "/state":
            with self.server.lock:
                self.send(json.dumps({"exports": self.server.exports}))
        else:
            self.send("Not found", "text/plain", 404)

    def do_POST(self):
        if self.path != "/export":
            self.send("Not found", "text/plain", 404)
            return
        size = int(self.headers.get("Content-Length", "0"))
        if not 0 < size <= 1024:
            self.send("Invalid request", "text/plain", 400)
            return
        try:
            period = json.loads(self.rfile.read(size))["period"]
            rows = LEDGER[period]
        except (ValueError, KeyError, TypeError):
            self.send("Unknown period", "text/plain", 400)
            return
        with self.server.lock:
            self.server.exports.append({"account": self.server.account, "period": period,
                                        "invoice_ids": [invoice for invoice, _ in rows]})
        scenario = self.server.scenario
        if scenario == "disconnect":
            # The server accepted the export, but its response is lost. A retry would create
            # a second event in /state, which the integration test checks independently.
            self.close_connection = True
            self.connection.shutdown(socket.SHUT_RDWR)
            self.connection.close()
            return
        if scenario == "missing-download":
            self.send("{}", status=202)
            return
        if scenario == "delayed":
            time.sleep(0.8)
        output = io.StringIO(newline="")
        writer = csv.writer(output)
        writer.writerow(["account", "period", "invoice_id", "amount", "currency"])
        if scenario == "truncated-report":
            rows = rows[:1]
        for invoice, amount in rows:
            writer.writerow(["other-account" if scenario == "wrong-file-account" else self.server.account,
                             "2026-07" if scenario == "wrong-file-period" else period, invoice, amount, "EUR"])
        body = "" if scenario == "empty-file" else output.getvalue()
        self.send(body, "text/csv; charset=utf-8", headers={"Content-Disposition": 'attachment; filename="invoices.csv"'})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=0, help="0 chooses a free port")
    parser.add_argument("--account", default="acme")
    parser.add_argument("--scenario", choices=SCENARIOS, default="normal")
    args = parser.parse_args()
    with ReportServer(args.port, args.account, args.scenario) as server:
        print(json.dumps({"url": f"http://127.0.0.1:{server.server_port}/"}), flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
