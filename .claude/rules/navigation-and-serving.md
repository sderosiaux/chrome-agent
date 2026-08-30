---
paths:
  - "src/landing.rs"
  - "src/serving.rs"
  - "src/commands/goto.rs"
  - "tests/goto_landed_tests.rs"
  - "tests/goto_settle_tests.rs"
---

# Where a navigation landed, and what answered there

## `landed`

`src/landing.rs`. The `goto` response carries `landed{requested, final, redirected, http_status?, serving, challenge_from?}`.

`requested` is the URL *after* the tool's own `https://` prefixing. Comparing the raw argument
would call every `goto example.com` a redirect.

### The redirect rule

Ignored (not a redirect):

- the fragment — never sent, so an anchor jump redirected nothing
- one trailing slash on the path — `/orders` and `/orders/` are the same resource
- the default port, and host case
- an empty query

Reported:

- a scheme change — an `http`→`https` upgrade is the server overriding the caller, and it changes which cookies travel
- a host change
- any other path change
- any query change — a gained `?next=…` is the usual shape of the bounce this exists to expose

Query parameters are compared in order. Reordering them would be a claim about server semantics.

### `http_status`

From Navigation Timing (`performance.getEntriesByType('navigation')[0].responseStatus`), the same
stealth-safe path as retroactive `network` capture: no `Network.enable`, no extra round trip (it
rides on the eval that already reads `location.href`).

It reports the LAST hop, so a followed 302 says 200. Absent — never `0`, never guessed — when the
page exposes none (no navigation entry, an older Chrome, or `responseStatus` of 0). It is what
the browser answered, not proof an HTTP response happened: on Chrome 151 a `file://` document
reports 200, so the tests assert "plausible or absent" rather than a value.

### The auth-wall hint

`goto` stays out of the verdict machinery and out of `mutates_page`: `landed` is self-describing,
and a `navigated` verdict on a command whose purpose is to navigate says nothing.

The one judgement is a `hint`, fired only when `redirected` is true AND a path *segment stem* of
the final URL is one of `login` / `log-in` / `signin` / `sign-in` / `sign_in` / `auth` / `sso`.
Stem, so `/login.php` matches and `/authors/tolkien` does not; segment, so `?next=/login` does
not. It is worded as a guess about a URL, because that is all it is.

Also attached to `navigate_and_read`, which takes a URL and hands back prose. Not attached to
`forward`/`back`: the caller asked for a history entry, not a URL.

Known gap: when `read` refuses the destination (Readability's 200-char minimum),
`navigate_and_read` fails with `ok:false` and the `landed` that would have explained why is lost
with it — the error path carries no landing.

## `serving`: what answered

`src/serving.rs`. `landed` used to settle only *where* a navigation ended up, so three shapes
measured on real sites were indistinguishable from a load:

- `cnrs.fr` — `http_status: 200`, `ok:true`, an F5 ASM document reading "The requested URL was rejected."
- `nowsecure.nl` — 200 with a Cloudflare widget as the whole document.
- `leboncoin.fr` — `http_status: 403`, `ok:true`, no judgement of any kind.

7 of 92 domains on a sweep behaved that way. `ok:true` is right in all three (the navigation
happened, and failing it would conflate a tool failure with a page fact), and it was the only
thing an agent had to branch on.

`serving` is one token from a closed set of five, plus `challenge_from` when a vendor was named.
Each word is a conjunction, and every threshold in it was moved by a real page.

| Token | Rule |
|---|---|
| `challenge` | a frame or script from a known vendor host, no form control of the site's own, under `TEXT_FLOOR` |
| `error` | the status is 4xx/5xx |
| `nothing_actionable` | no control, no link, no script, under `TEXT_FLOOR` (512 chars, ~80 words; the F5 notice holds 152) |
| `unreadable` | the probe did not run |
| `page` | none of the above |

**Ranked in that order.** `challenge` above `error` because `leboncoin.fr` is both: "403" reads
as an authorization problem and sends an agent after credentials, while the vendor names the
mechanism and the recovery (`--connect`, not `--stealth`). `http_status: 403` stays on the
response, so the ranking costs nothing. `error` above `nothing_actionable` because a status is
the server's own statement and a shape is our inference; a 404 with a nav bar and a 404 with
nothing on it are both `error`.

### Why these signals

- **A hostname is the evidence, not a keyword.** Vendor-chosen, identical across every site that deploys it, language-independent, and not written by whoever is describing their own block page — which is what makes "Request Rejected" / "Access Denied" fragile.
- **Scripts as well as frames.** `leboncoin.fr`'s DataDome frame carries `geo.captcha-delivery.com`, but `nowsecure.nl`'s Turnstile interstitial reports `src: ""` (it injects into an `about:blank` frame) and the only vendor URL in the document is `challenges.cloudflare.com/turnstile/v0/api.js`. Frames alone answered `nothing_actionable` there.
- **Controls and links are counted separately.** `npmjs.com`'s Cloudflare block page carries two links of its own and nothing else, so one "actionable" total was never zero and it fell through to `error`. A page that carries a captcha *and* is usable carries form controls, because that is what a captcha protects.
- **`scripts == 0` is what an appliance refusal has and a half-rendered page does not.** The edge generates its notice without reaching the origin's asset pipeline (`cnrs.fr`: 0 scripts, 0 stylesheets, 0 images), while `amazon.fr` ships 21 `<script src>` from the first byte. Price: a refusal that ships a script reads `page`.

### What the words do not claim

- **`page` is the absence of evidence, not a certificate.** A paywall, a cookie wall, a soft 404 and an unknown vendor all read `page`. The rule leans that way on purpose: declaring a usable page blocked makes an agent abandon work it could have done.
- **`nothing_actionable` measures the document and does not say "you were blocked."** An edge refusal and a page whose content had not arrived look identical from here, and the hint names both. Residual false positive, measured: `www.amazon.fr` over three runs read `nothing_actionable`, `page`, `page` — its first paint is genuinely empty. One domain in a 30-domain sweep. Waiting and re-measuring was rejected: it puts a second observation window on the path of every genuine refusal, and the recovery costs one `inspect`.
- `challenge_from` is reported whenever a vendor host is present, INCLUDING under `serving: "page"`. A Turnstile on a working login form is worth knowing when the submit later fails. The word is the branch, the field is the evidence.

### Cost and wiring

The shape rides on the eval that already reads `location.href`: no extra round trip, no
`Network.enable`, `--stealth` intact. Every count over-counts rather than under-counts (hidden
text and off-screen buttons are counted), because over-counting produces `page`, which is
silence. Text is walked with a `TreeWalker` capped at 4096 rather than read from `innerText`,
which forces layout.

`assess` is pure and unit-tested against the six measured shapes with no Chrome; `Landing` wires
it, so CLI, pipe and batch cannot drift.

`serving` is `landed`'s vocabulary, not `verdict`'s, and there is deliberately no `next` token —
the hint carries the recovery. Where both judgements fire, `serving` outranks the auth-wall guess
in the single `hint` field: one is a status the server sent and a document that was measured, the
other is a reading of a string in a URL.

## `forward` and `--header`

- `forward` is symmetric to `back`: `Page.getNavigationHistory` + `Page.navigateToHistoryEntry`.
- `goto --header` is repeatable `"Name: Value"` (split on the first colon), applied via `Network.setExtraHTTPHeaders` before navigating.
- `--post` is intentionally not implemented: fragile over `Page.navigate`.
