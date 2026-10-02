# Visual testing on Windows

Two PowerShell runners open the RustKit engine in a window so a person can
look at what it renders. They are the Windows twins of
`scripts/visual_test_runner.sh` and `scripts/visual_live_runner.sh` on
hiwave-macos and take the same options, spelled the PowerShell way.

Both drive `hiwave-smoke`, the small windowed harness in
`crates/hiwave-smoke` (a chrome bar and shelf in WebView2, the content area in
RustKit). They build it first, so the first run after a checkout takes a few
minutes; later runs start in a second.

## Prerequisites

- The normal Windows build toolchain (see the repository README).
- For `-Compare` in the live runner: Google Chrome. The runner looks at
  `$env:PARITY_CHROME_PATH`, then a pinned Chrome for Testing under
  `.browsers\chrome\win64-*\chrome-win64\chrome.exe`, then the installed
  Chrome. The installed Chrome is not the pinned parity-oracle build, so a
  side-by-side view is a visual aid, not a board number.

## Canned fixtures: `scripts\visual_test_runner.ps1`

```powershell
.\scripts\visual_test_runner.ps1                          # every case, 3 s each
.\scripts\visual_test_runner.ps1 -ListCases               # the case table
.\scripts\visual_test_runner.ps1 -Case card-grid -DurationMs 8000
.\scripts\visual_test_runner.ps1 -Resolution fhd -Fullscreen
.\scripts\visual_test_runner.ps1 -AllResolutions          # fhd, macbook, laptop, ipad
.\scripts\visual_test_runner.ps1 -Stress                  # keep the sidebar/shelf churn
```

The case table is the same 13 pages the macOS runner shows: the shell's own
`new_tab`, `about`, `settings`, `chrome` and `shelf` pages, and the eight
`websuite/cases`. Resolution presets: `fhd` 1920x1080, `macbook` 1440x900,
`qhd` 2560x1440, `laptop` 1366x768, `ipad` 1024x768, `mobile` 414x896.

By default the page is shown static at the requested size. `-Stress` keeps
hiwave-smoke's scripted layout churn (the sidebars and shelf animate in and
the content area ends up smaller), which is what the harness was built for
and what the macOS fixture runner shows.

## Live sites: `scripts\visual_live_runner.ps1`

```powershell
.\scripts\visual_live_runner.ps1                          # all 20 board sites, 10 s each
.\scripts\visual_live_runner.ps1 -List                    # the site ids
.\scripts\visual_live_runner.ps1 -Site wikipedia -Compare # Chrome opens beside it
.\scripts\visual_live_runner.ps1 -Url https://news.ycombinator.com -DurationMs 20000
```

The site list is `websuite\realsite-top20.json`, the same pinned list as the
macOS real-site board (1280x800 first viewport). A live URL goes through the
engine's real navigation path with the product user agent, so the page is
fetched the way HiWave fetches it. A slow site may still be loading when a
short `-DurationMs` expires; the default 10 s is usually enough for the first
viewport.

## What the runners return

Each run prints one block per page and ends with `Shown: N, errors: M`. The
exit code is `M`, so `0` means every window opened and closed cleanly. Lines
from hiwave-smoke that contain `ERROR`, `Failed` or `panicked` are echoed
under the page they belong to.

## hiwave-smoke flags used

`--html-file <path>` or `--url <url>`, `--width`, `--height`,
`--duration-ms`, `--static` (no layout churn; implied by `--url`) and
`--fullscreen`. `--dump-frame <file.ppm>` still captures the content frame for
the golden/parity scripts. The window is sized to `--width` x `--height` plus
the 72 px chrome bar; in fullscreen the content takes whatever the screen
gives.

## Not covered here

Pixel parity numbers against the Chrome oracle come from the parity scripts
(`scripts\parity_gate.ps1`, `scripts\parity_swarm.py`, `tools\parity_oracle`),
not from these runners. These runners are for eyes.
