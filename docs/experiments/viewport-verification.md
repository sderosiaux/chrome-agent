# Exact viewport — reproduction and verification

Repository: `/Users/sderosiaux/code/personal/chrome-agent`.
Baseline commit: `143c50b97c3cbc5291f60b28bbe52f186065e621` (0.16.0).
Installed comparison binary: `~/.cargo/bin/chrome-agent` (0.12.0).
Environment: macOS, Google Chrome 155.0.8059.40; headed screen 2560×1440 CSS pixels, DPR 2.

## Root cause, recorded before implementation

The reported **756×469** was reproduced in headless mode in both binaries, including the PNG.
Headed mode measured **1200×1309**, DPR 2, with a **2400×2618** PNG.

Baseline source locations:

- `src/browser.rs:248–274`: `managed_launch_args` provides neither a window size nor a viewport.
- `src/run_helpers.rs:518–527`: `Target.createTarget` omits `width` and `height`.
- `src/emulation.rs:303–327`: the existing reapply path only sets metrics when a page has an
  explicit `device_emulation`. Unconfigured pages inherit Chrome/OS dimensions.
- `src/commands/screenshot.rs:94–109`: an ordinary screenshot has no clip, captures Chrome's
  surface at its current viewport and DPR, and does not set dimensions.

The hypothesis that no `Emulation.setDeviceMetricsOverride` call exists is false for the current
repository: `src/emulation.rs:150–160` already has one behind explicit `emulate device`. The missing
contract is an explicit default, a viewport-only interface, and a measured success criterion.
The 87 px height difference between inner and outer dimensions was observed; this report does
not infer how Chrome divided that space between individual UI elements.

## Mechanism and behavior

Implementation entry points in the final source: `src/viewport.rs:84` (CDP application),
`src/viewport.rs:106` (top-document measurement), `src/viewport.rs:126` (verdict),
`src/viewport.rs:157` (commit and read-back), `src/session.rs:68` (per-page persistence),
`src/cli.rs:106` (global flag), `src/landing.rs:31` and `src/run_helpers.rs:569` (reporting).

`Emulation.setDeviceMetricsOverride` applies width, height, DPR, `mobile:false`, screen dimensions,
and the visible surface size on the selected target. The same mechanism handles headless and
headed Chrome. `Page.captureScreenshot` explicitly requests the surface. OS window bounds are
reported separately: a smaller visible window does not prevent a larger render surface.

`--window-size` and `Browser.setWindowBounds` size an OS window, shared by tabs, and cannot alone
provide an independent exact CSS viewport per named page. `Target.createTarget` width/height are
restricted to a new window or headless shell and do not update existing targets. These are not
used to approximate the requested viewport.

Protocol reference: [official CDP definitions](https://github.com/ChromeDevTools/devtools-protocol/blob/master/json/browser_protocol.json)
(`Emulation.setDeviceMetricsOverride`, `Page.captureScreenshot`, `Target.createTarget`,
`Browser.setWindowBounds`).

New default: **1440×900@1**, including legacy page records with no viewport. This deliberately
changes previous implicit dimensions and the native DPR in headed mode. The requested viewport
is stored per browser/page, reapplied on connection, and survives navigation. Closing/relaunching
a browser discards its page state, consistent with existing emulation. `status` shows requested
sizes and the default; `goto.landed.viewport` contains measured CSS dimensions and DPR.

`viewport set/get` measure the top document, regardless of an iframe binding, and return
`requested`, `effective`, `delta`, and `verdict: exact|mismatch|unknown`. Width, height, and DPR are
compared exactly. A mismatch returns `ok:false`, CLI exit 2; an unreadable reading exits 1.
`--verdict off` cannot disable this measurement. Global `--viewport` refuses the following command
if verification fails. Fractional DPR normalization is reported, not silently accepted.

The existing `emulate device` temporarily supersedes desktop metrics. `emulate reset` restores the
saved desktop viewport. Explicit `viewport set` or `--viewport` exits device mode. No device
presets, mobile user-agent feature, or new touch feature was added.

## Baseline command output

The first screenshot attempt used unsupported `--output`; it failed before capture. The successful
commands below use the actual CLI syntax and `file` to inspect the resulting image.

```text
$ /Users/sderosiaux/.cargo/bin/chrome-agent --browser viewport-before-installed-headless --page probe --json goto file:///tmp/chrome-agent-viewport-audit/probe.html
{"landed":{"final":"file:///tmp/chrome-agent-viewport-audit/probe.html","http_status":200,"redirected":false,"requested":"file:///tmp/chrome-agent-viewport-audit/probe.html"},"ok":true,"title":"Viewport probe","url":"file:///tmp/chrome-agent-viewport-audit/probe.html"}

$ /Users/sderosiaux/.cargo/bin/chrome-agent --browser viewport-before-installed-headless --page probe --json eval measure()
{"ok":true,"result":{"devicePixelRatio":1,"innerHeight":469,"innerWidth":756,"min1024":false,"outerHeight":556,"outerWidth":756,"screen":{"height":600,"width":800}}}

$ /Users/sderosiaux/.cargo/bin/chrome-agent --browser viewport-before-installed-headless --page probe --json screenshot
{"ok":true,"path":"/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791736117262.png"}

/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791736117262.png: PNG image data, 756 x 469, 8-bit/color RGB, non-interlaced
$ /Users/sderosiaux/.cargo/bin/chrome-agent --browser viewport-before-installed-headed --page probe --json --headed goto file:///tmp/chrome-agent-viewport-audit/probe.html
{"landed":{"final":"file:///tmp/chrome-agent-viewport-audit/probe.html","http_status":200,"redirected":false,"requested":"file:///tmp/chrome-agent-viewport-audit/probe.html"},"ok":true,"title":"Viewport probe","url":"file:///tmp/chrome-agent-viewport-audit/probe.html"}

$ /Users/sderosiaux/.cargo/bin/chrome-agent --browser viewport-before-installed-headed --page probe --json eval measure()
{"ok":true,"result":{"devicePixelRatio":2,"innerHeight":1309,"innerWidth":1200,"min1024":true,"outerHeight":1396,"outerWidth":1200,"screen":{"height":1440,"width":2560}}}

$ /Users/sderosiaux/.cargo/bin/chrome-agent --browser viewport-before-installed-headed --page probe --json screenshot
{"ok":true,"path":"/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791736118699.png"}

/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791736118699.png: PNG image data, 2400 x 2618, 8-bit/color RGB, non-interlaced
$ target/debug/chrome-agent --browser viewport-before-repo-headless --page probe --json goto file:///tmp/chrome-agent-viewport-audit/probe.html
{"hint":"This document has no link, no form control, no script and 184 characters of text — nothing to act on, and nothing loaded from anywhere else. An edge appliance's refusal notice is self-contained like this; so is a page whose content had not arrived when the tool looked. It is a measurement, not a claim you were blocked. Run `chrome-agent --browser viewport-before-repo-headless inspect` to read it.","landed":{"final":"file:///tmp/chrome-agent-viewport-audit/probe.html","http_status":200,"redirected":false,"requested":"file:///tmp/chrome-agent-viewport-audit/probe.html","serving":"nothing_actionable"},"ok":true,"title":"Viewport probe","url":"file:///tmp/chrome-agent-viewport-audit/probe.html"}

$ target/debug/chrome-agent --browser viewport-before-repo-headless --page probe --json eval measure()
{"ok":true,"result":{"devicePixelRatio":1,"innerHeight":469,"innerWidth":756,"min1024":false,"outerHeight":556,"outerWidth":756,"screen":{"height":600,"width":800}}}

$ target/debug/chrome-agent --browser viewport-before-repo-headless --page probe --json screenshot
{"ok":true,"path":"/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791736120034.png"}

/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791736120034.png: PNG image data, 756 x 469, 8-bit/color RGB, non-interlaced
$ target/debug/chrome-agent --browser viewport-before-repo-headed --page probe --json --headed goto file:///tmp/chrome-agent-viewport-audit/probe.html
{"hint":"This document has no link, no form control, no script and 187 characters of text — nothing to act on, and nothing loaded from anywhere else. An edge appliance's refusal notice is self-contained like this; so is a page whose content had not arrived when the tool looked. It is a measurement, not a claim you were blocked. Run `chrome-agent --browser viewport-before-repo-headed inspect` to read it.","landed":{"final":"file:///tmp/chrome-agent-viewport-audit/probe.html","http_status":200,"redirected":false,"requested":"file:///tmp/chrome-agent-viewport-audit/probe.html","serving":"nothing_actionable"},"ok":true,"title":"Viewport probe","url":"file:///tmp/chrome-agent-viewport-audit/probe.html"}

$ target/debug/chrome-agent --browser viewport-before-repo-headed --page probe --json eval measure()
{"ok":true,"result":{"devicePixelRatio":2,"innerHeight":1309,"innerWidth":1200,"min1024":true,"outerHeight":1396,"outerWidth":1200,"screen":{"height":1440,"width":2560}}}

$ target/debug/chrome-agent --browser viewport-before-repo-headed --page probe --json screenshot
{"ok":true,"path":"/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791736121066.png"}

/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791736121066.png: PNG image data, 2400 x 2618, 8-bit/color RGB, non-interlaced
```

## Validation

Acceptance matrix, measured in both headless and headed Chrome:

| Request | innerWidth × innerHeight | DPR | min-width: 1024px | PNG (`file`) | Verdict |
|---|---|---|---|---|---|
| 1440x900 | 1440×900 | 1 | true | 1440×900 | exact |
| 1024x768 | 1024×768 | 1 | true | 1024×768 | exact |
| 390x844@2 | 390×844 | 2 | false | 780×1688 | exact |

The headed 1440 px layout is exact even though the OS window is 1200 px wide. The 390×844@2
headed PNG was also inspected visually; its displayed measurements agree with `eval` and `file`.
The fixture is `tests/fixtures/viewport_probe.html` (a top-level page, no iframe harness).

The new integration suite also checks two browser profiles concurrently, two named pages,
separate-process persistence, navigation persistence, the global flag after the subcommand,
pipe/nested batch, device reset interaction, and top-frame reads with an iframe selected.
Deliberately contradictory JS measurements prove `mismatch`, nonzero CLI exit, and refusal to
execute the command following a failed global viewport setting. A throwing measurement proves
`unknown` instead of a success claim.

Commands below use a private `CHROME_AGENT_SESSION_DIR`; they do not modify the installed binary.

```text
CHROME_AGENT_SESSION_DIR=/tmp/chrome-agent-viewport-audit/manual-sessions
$ target/debug/chrome-agent --browser report-headless --page probe --json goto file:///Users/sderosiaux/code/personal/chrome-agent/tests/fixtures/viewport_probe.html
{"hint":"This document has no link, no form control, no script and 185 characters of text — nothing to act on, and nothing loaded from anywhere else. An edge appliance's refusal notice is self-contained like this; so is a page whose content had not arrived when the tool looked. It is a measurement, not a claim you were blocked. Run `chrome-agent --browser report-headless inspect` to read it.","landed":{"final":"file:///Users/sderosiaux/code/personal/chrome-agent/tests/fixtures/viewport_probe.html","http_status":200,"redirected":false,"requested":"file:///Users/sderosiaux/code/personal/chrome-agent/tests/fixtures/viewport_probe.html","serving":"nothing_actionable","viewport":{"dpr":1.0,"height":900,"width":1440}},"ok":true,"title":"Viewport probe","url":"file:///Users/sderosiaux/code/personal/chrome-agent/tests/fixtures/viewport_probe.html"}
$ target/debug/chrome-agent --browser report-headless --page probe --json viewport set 1440x900
{"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":1,"innerHeight":900,"innerWidth":1440,"min1024":true,"outerHeight":556,"outerWidth":756,"screen":{"height":900,"width":1440}},"ok":true,"requested":{"dpr":1.0,"height":900,"width":1440},"verdict":"exact"}
$ target/debug/chrome-agent --browser report-headless --page probe --json eval measure()
{"ok":true,"result":{"devicePixelRatio":1,"innerHeight":900,"innerWidth":1440,"min1024":true,"outerHeight":556,"outerWidth":756,"screen":{"height":900,"width":1440}}}
$ target/debug/chrome-agent --browser report-headless --page probe --json eval innerWidth
{"ok":true,"result":1440}
$ target/debug/chrome-agent --browser report-headless --page probe --json screenshot --filename verified-viewport-headless-1440x900.png
{"ok":true,"path":"/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headless-1440x900.png"}
$ file /Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headless-1440x900.png
/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headless-1440x900.png: PNG image data, 1440 x 900, 8-bit/color RGB, non-interlaced
$ target/debug/chrome-agent --browser report-headless --page probe --json viewport set 1024x768
{"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":1,"innerHeight":768,"innerWidth":1024,"min1024":true,"outerHeight":556,"outerWidth":756,"screen":{"height":768,"width":1024}},"ok":true,"requested":{"dpr":1.0,"height":768,"width":1024},"verdict":"exact"}
$ target/debug/chrome-agent --browser report-headless --page probe --json eval measure()
{"ok":true,"result":{"devicePixelRatio":1,"innerHeight":768,"innerWidth":1024,"min1024":true,"outerHeight":556,"outerWidth":756,"screen":{"height":768,"width":1024}}}
$ target/debug/chrome-agent --browser report-headless --page probe --json eval innerWidth
{"ok":true,"result":1024}
$ target/debug/chrome-agent --browser report-headless --page probe --json screenshot --filename verified-viewport-headless-1024x768.png
{"ok":true,"path":"/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headless-1024x768.png"}
$ file /Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headless-1024x768.png
/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headless-1024x768.png: PNG image data, 1024 x 768, 8-bit/color RGB, non-interlaced
$ target/debug/chrome-agent --browser report-headless --page probe --json viewport set 390x844@2
{"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":2,"innerHeight":844,"innerWidth":390,"min1024":false,"outerHeight":556,"outerWidth":756,"screen":{"height":844,"width":390}},"ok":true,"requested":{"dpr":2.0,"height":844,"width":390},"verdict":"exact"}
$ target/debug/chrome-agent --browser report-headless --page probe --json eval measure()
{"ok":true,"result":{"devicePixelRatio":2,"innerHeight":844,"innerWidth":390,"min1024":false,"outerHeight":556,"outerWidth":756,"screen":{"height":844,"width":390}}}
$ target/debug/chrome-agent --browser report-headless --page probe --json eval innerWidth
{"ok":true,"result":390}
$ target/debug/chrome-agent --browser report-headless --page probe --json screenshot --filename verified-viewport-headless-390x844@2.png
{"ok":true,"path":"/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headless-390x844@2.png"}
$ file /Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headless-390x844@2.png
/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headless-390x844@2.png: PNG image data, 780 x 1688, 8-bit/color RGB, non-interlaced
$ target/debug/chrome-agent --browser report-headless --page probe --json close --purge
{"exited":true,"message":"Closed browser=report-headless (pid=81940) (profile purged)","ok":true,"signalled":true}
$ target/debug/chrome-agent --browser report-headed --page probe --json --headed goto file:///Users/sderosiaux/code/personal/chrome-agent/tests/fixtures/viewport_probe.html
{"hint":"This document has no link, no form control, no script and 185 characters of text — nothing to act on, and nothing loaded from anywhere else. An edge appliance's refusal notice is self-contained like this; so is a page whose content had not arrived when the tool looked. It is a measurement, not a claim you were blocked. Run `chrome-agent --browser report-headed inspect` to read it.","landed":{"final":"file:///Users/sderosiaux/code/personal/chrome-agent/tests/fixtures/viewport_probe.html","http_status":200,"redirected":false,"requested":"file:///Users/sderosiaux/code/personal/chrome-agent/tests/fixtures/viewport_probe.html","serving":"nothing_actionable","viewport":{"dpr":1.0,"height":900,"width":1440}},"ok":true,"title":"Viewport probe","url":"file:///Users/sderosiaux/code/personal/chrome-agent/tests/fixtures/viewport_probe.html"}
$ target/debug/chrome-agent --browser report-headed --page probe --json viewport set 1440x900
{"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":1,"innerHeight":900,"innerWidth":1440,"min1024":true,"outerHeight":1396,"outerWidth":1200,"screen":{"height":900,"width":1440}},"ok":true,"requested":{"dpr":1.0,"height":900,"width":1440},"verdict":"exact"}
$ target/debug/chrome-agent --browser report-headed --page probe --json eval measure()
{"ok":true,"result":{"devicePixelRatio":1,"innerHeight":900,"innerWidth":1440,"min1024":true,"outerHeight":1396,"outerWidth":1200,"screen":{"height":900,"width":1440}}}
$ target/debug/chrome-agent --browser report-headed --page probe --json eval innerWidth
{"ok":true,"result":1440}
$ target/debug/chrome-agent --browser report-headed --page probe --json screenshot --filename verified-viewport-headed-1440x900.png
{"ok":true,"path":"/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headed-1440x900.png"}
$ file /Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headed-1440x900.png
/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headed-1440x900.png: PNG image data, 1440 x 900, 8-bit/color RGB, non-interlaced
$ target/debug/chrome-agent --browser report-headed --page probe --json viewport set 1024x768
{"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":1,"innerHeight":768,"innerWidth":1024,"min1024":true,"outerHeight":1396,"outerWidth":1200,"screen":{"height":768,"width":1024}},"ok":true,"requested":{"dpr":1.0,"height":768,"width":1024},"verdict":"exact"}
$ target/debug/chrome-agent --browser report-headed --page probe --json eval measure()
{"ok":true,"result":{"devicePixelRatio":1,"innerHeight":768,"innerWidth":1024,"min1024":true,"outerHeight":1396,"outerWidth":1200,"screen":{"height":768,"width":1024}}}
$ target/debug/chrome-agent --browser report-headed --page probe --json eval innerWidth
{"ok":true,"result":1024}
$ target/debug/chrome-agent --browser report-headed --page probe --json screenshot --filename verified-viewport-headed-1024x768.png
{"ok":true,"path":"/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headed-1024x768.png"}
$ file /Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headed-1024x768.png
/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headed-1024x768.png: PNG image data, 1024 x 768, 8-bit/color RGB, non-interlaced
$ target/debug/chrome-agent --browser report-headed --page probe --json viewport set 390x844@2
{"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":2,"innerHeight":844,"innerWidth":390,"min1024":false,"outerHeight":1396,"outerWidth":1200,"screen":{"height":844,"width":390}},"ok":true,"requested":{"dpr":2.0,"height":844,"width":390},"verdict":"exact"}
$ target/debug/chrome-agent --browser report-headed --page probe --json eval measure()
{"ok":true,"result":{"devicePixelRatio":2,"innerHeight":844,"innerWidth":390,"min1024":false,"outerHeight":1396,"outerWidth":1200,"screen":{"height":844,"width":390}}}
$ target/debug/chrome-agent --browser report-headed --page probe --json eval innerWidth
{"ok":true,"result":390}
$ target/debug/chrome-agent --browser report-headed --page probe --json screenshot --filename verified-viewport-headed-390x844@2.png
{"ok":true,"path":"/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headed-390x844@2.png"}
$ file /Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headed-390x844@2.png
/Users/sderosiaux/.chrome-agent/tmp/verified-viewport-headed-390x844@2.png: PNG image data, 780 x 1688, 8-bit/color RGB, non-interlaced
$ target/debug/chrome-agent --browser report-headed --page probe --json close --purge
{"exited":true,"message":"Closed browser=report-headed (pid=82094) (profile purged)","ok":true,"signalled":true}
```

Final automated verification (exit codes observed as 0):

```text
$ CHROME_AGENT_REQUIRE_CHROME=1 CHROME_AGENT_SESSION_DIR=/tmp/chrome-agent-viewport-audit/suite-final cargo test --locked --no-fail-fast -- --test-threads=2
[71 test binaries completed: 1014 passed, 0 failed, 1 ignored; aggregate of the test-result lines]
```

The ignored test requires a graphical desktop and was explicitly executed in the separate
viewport suite below (7 passed, including headed):

```text
$ CHROME_AGENT_REQUIRE_CHROME=1 cargo test --test viewport_tests -- --include-ignored --test-threads=1 --nocapture
   Compiling chrome-agent v0.16.0 (/Users/sderosiaux/code/personal/chrome-agent)
    Finished `test` profile [unoptimized + debuginfo] target(s) in 0.34s
     Running tests/viewport_tests.rs (target/debug/deps/viewport_tests-cde40d99b08a04b3)

running 7 tests
test device_reset_restores_desktop_and_viewport_set_leaves_device_mode ... ok
test global_flag_pages_and_parallel_browsers_are_isolated ... ok
test headed_sizes_media_queries_png_and_persistence ... viewport set 1440x900 (headed=true): {"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":1,"innerHeight":900,"innerWidth":1440,"min1024":true,"outerHeight":1396,"outerWidth":1200,"screen":{"height":900,"width":1440}},"ok":true,"requested":{"dpr":1.0,"height":900,"width":1440},"verdict":"exact"}
/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791737333120.png: PNG image data, 1440 x 900, 8-bit/color RGB, non-interlaced
viewport set 1024x768 (headed=true): {"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":1,"innerHeight":768,"innerWidth":1024,"min1024":true,"outerHeight":1396,"outerWidth":1200,"screen":{"height":768,"width":1024}},"ok":true,"requested":{"dpr":1.0,"height":768,"width":1024},"verdict":"exact"}
/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791737333438.png: PNG image data, 1024 x 768, 8-bit/color RGB, non-interlaced
viewport set 390x844@2 (headed=true): {"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":2,"innerHeight":844,"innerWidth":390,"min1024":false,"outerHeight":1396,"outerWidth":1200,"screen":{"height":844,"width":390}},"ok":true,"requested":{"dpr":2.0,"height":844,"width":390},"verdict":"exact"}
/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791737333753.png: PNG image data, 780 x 1688, 8-bit/color RGB, non-interlaced
ok
test headless_sizes_media_queries_png_and_persistence ... viewport set 1440x900 (headed=false): {"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":1,"innerHeight":900,"innerWidth":1440,"min1024":true,"outerHeight":556,"outerWidth":756,"screen":{"height":900,"width":1440}},"ok":true,"requested":{"dpr":1.0,"height":900,"width":1440},"verdict":"exact"}
/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791737335015.png: PNG image data, 1440 x 900, 8-bit/color RGB, non-interlaced
viewport set 1024x768 (headed=false): {"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":1,"innerHeight":768,"innerWidth":1024,"min1024":true,"outerHeight":556,"outerWidth":756,"screen":{"height":768,"width":1024}},"ok":true,"requested":{"dpr":1.0,"height":768,"width":1024},"verdict":"exact"}
/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791737335329.png: PNG image data, 1024 x 768, 8-bit/color RGB, non-interlaced
viewport set 390x844@2 (headed=false): {"delta":{"dpr":0.0,"height":0.0,"width":0.0},"effective":{"devicePixelRatio":2,"innerHeight":844,"innerWidth":390,"min1024":false,"outerHeight":556,"outerWidth":756,"screen":{"height":844,"width":390}},"ok":true,"requested":{"dpr":2.0,"height":844,"width":390},"verdict":"exact"}
/Users/sderosiaux/.chrome-agent/tmp/screenshot-1791737335685.png: PNG image data, 780 x 1688, 8-bit/color RGB, non-interlaced
ok
test invalid_cli_sizes_fail_before_launch_and_help_explains_default ... ok
test pipe_and_nested_batch_share_the_viewport_and_persist_it ... ok
test viewport_reads_the_top_document_and_reports_a_mismatch_without_success ... ok

test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 7.85s


$ cargo clippy --locked --all-targets -- -D warnings
    Checking chrome-agent v0.16.0 (/Users/sderosiaux/code/personal/chrome-agent)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.75s

$ cargo fmt --all --check
[exit 0]
$ git diff --check
[exit 0]
$ cargo build --release --locked
    Finished `release` profile [optimized] target(s) in 55.04s
```

The complete Cargo log is at `/tmp/chrome-agent-viewport-audit/cargo-test-final.log`; manual
command output is at `/tmp/chrome-agent-viewport-audit/verified-commands.txt`.

## Concurrent-version interference found during validation

Other active agents on this machine were invoking the installed 0.12.0 binary while these tests
ran. Its session serializer discards fields it does not know when rewriting the shared session
store. Direct observation, with no command on the test page between readings:

```text
viewport set 390x844@2 => verdict=exact
session viewport at 0.0 s: {'width': 390, 'height': 844, 'dpr': 2.0}
session viewport at 1.2 s: {'width': 390, 'height': 844, 'dpr': 2.0}
session viewport at 1.3 s: None
```

This explains the early persistence failures returning the new default: the request itself had
been erased on disk. It is distinct from CDP resize behavior and from isolation between two
browser profiles driven by the updated binary.


`CHROME_AGENT_SESSION_DIR` now provides an absolute override for the session store, managed
browser profiles, and daemon files. Both profiles in the parallel isolation test share the same
private store, so the test still exercises concurrent session merges. Existing tests with their
own private home fixture opt out of this override. Screenshots, history and macros retain their
usual locations; no other agent's browser was stopped.

## Not verified / limits

- Linux and Windows headed window managers, remote Chrome connections, and other Chrome versions
  were not exercised. All live viewport checks here used Chrome 155.0.8059.40 on macOS.
- No agent-browser 0.39 benchmark was run. The requested exact dimensions were checked directly.
- Fractional DPR parsing is tested; the live acceptance matrix uses DPR 1 and 2. Fractional raster
  dimensions are subject to Chrome rounding; any observed DPR difference is reported.
- A real OS-imposed clamp was not observed: all headed sizes were exact. The mismatch/unknown
  reporting paths were exercised with controlled page-side measurement failures.
- Concurrent old/new binaries sharing the default session directory remain incompatible because
  the old binary drops unknown fields. Use separate session directories during development.
- Functional verification above used `target/debug/chrome-agent`. The optimized macOS binary
  was also built successfully. Cross-platform release packages were not verified locally.
