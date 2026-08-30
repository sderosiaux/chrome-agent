---
paths:
  - "src/hints/**"
  - "tests/refusal_report_tests.rs"
---

# The contract every error message holds to

Every error carries a `hint`, and each one holds to three rules:

1. **One FACT about what is known**, never a question or a hedge. "Is Chrome running?" asked the reader something the tool knows better (it launches its own Chrome); "Element may be hidden" hedged about a box model it had just measured.
2. **Exactly one imperative command with the real values substituted.** `chrome-agent scroll <uid>` is a template, not a command — an agent copying it runs a uid literally named `<uid>`. The uid comes out of the error message, and the invocation carries `--browser <this session>`, since a hint that drops it points at another agent's browser. Where two routes exist, the criterion that chooses between them is stated: two options with no criterion is the same shrug as no hint.
3. **When a retry would be dangerous, forbid it in words.** The advice on a lost transport was "Try running the command again", on a click that may already have been delivered, and the page cannot tell a retry from a second deliberate action.

Hints are owned `String`s for rule 2. The rules are enforced by tests over every message the
module recognises: no unresolved placeholder, no retry wording, no command aimed at the wrong
browser.

A node with no `backendDOMNodeId` (an `e{n}` uid, no DOM element behind it) and a CDP reply we
could not parse used to share "Page structure issue" — two causes, neither named. They are split.

## Navigation failures

`Navigation failed` answered **"Check the URL is valid and the page is reachable"** — no fact, no
command, one sentence for five causes measured over 22 real failures:

| Code | Count | Stage |
|---|---|---|
| `ERR_NAME_NOT_RESOLVED` | 16 | DNS |
| `ERR_HTTP_RESPONSE_CODE_FAILURE` | 2 | HTTP |
| `ERR_CONNECTION_REFUSED` | 2 | TCP |
| `ERR_CERT_COMMON_NAME_INVALID` | 1 | TLS |
| `ERR_CONNECTION_RESET` | 1 | TCP |

Each stage rules out a different set of causes, which is exactly the fact rule 1 asks for.
`hints::navigation_failure` gives each its own, plus `ERR_UNSAFE_PORT` and `ERR_ABORTED`, a branch
that reports an unrecognised `net::` code as itself, and one that says a message carries no code
rather than picking a cause.

`commands::goto` writes the URL into the message (`Navigation failed for {url}: …`) because rule 2
needs values and the error was the only channel to `error_hint`.

Two routes the tool genuinely owns:

- DNS names the `www.` variant as one imperative command, with the criterion that chooses it (an apex with no address record is usually a missing subdomain).
- A refused connection offers `http://`: `goto` is what prefixed `https://`, so a refused connection on a plain-HTTP server is a failure the tool's own default caused.

**Two carry no command, deliberately.** A certificate the site chose and a connection something in
the middle dropped have no recovery inside this tool, and rule 2 asks for one imperative command
*with real values*, not for one invented. Both say so ("no chrome-agent flag makes Chrome accept
it"), which is rule 3 in the form it takes when nothing can be retried usefully.
