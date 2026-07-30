<p align="center">
  <img src="docs/logo.png" alt="HiWave" width="120" />
</p>

<h1 align="center">HiWave</h1>

<p align="center">
  <strong>Focus. Flow. Freedom.</strong><br>
  A privacy-first browser built from scratch in Rust.
</p>

<p align="center">
  <a href="#features">Features</a> •
  <a href="#download">Download</a> •
  <a href="#philosophy">Philosophy</a> •
  <a href="#contributing">Contributing</a> •
  <a href="#support">Support</a>
</p>

<p align="center">
  <a href="https://github.com/hiwavebrowser/hiwave-windows/actions/workflows/metrics.yml"><img src="https://github.com/hiwavebrowser/hiwave-windows/actions/workflows/metrics.yml/badge.svg?branch=master" alt="Windows CI" /></a>
  <img src="https://img.shields.io/badge/engine-RustKit_(original)-orange" alt="Engine: RustKit" />
  <img src="https://img.shields.io/badge/status-alpha-blueviolet" alt="Status: Alpha" />
  <img src="https://img.shields.io/badge/license-MPL--2.0-blue" alt="License: MPL-2.0" />
  <img src="https://img.shields.io/badge/platforms-Win%20%7C%20Mac%20%7C%20Linux-lightgrey" alt="Platforms" />
</p>

---

## The Problem

Modern browsers are designed to keep you browsing. More tabs, more tracking, more data vultures, more history, more extensions, more complexity. The result? Dozens of open tabs you'll "get to eventually," fractured attention, and digital clutter that drains your focus and steals your privacy.

## The Solution

**HiWave** flips the script. We built a browser that actively helps you browse *less* — in a good way.

- **The Shelf** — Tabs you're not using decay and fade away, so you don't have to manually manage them
- **Workspaces** — Separate contexts (work, personal, research) that don't bleed into each other
- **Built-in Privacy** — Ad and tracker blocking with no extensions needed
- **Three Modes** — Choose your level of automation: do it yourself, get suggestions, or let Flow handle it

## The Engine

Unlike browsers that wrap Chromium or WebKit, **HiWave runs on RustKit** — our own browser engine written from scratch in Rust. No Blink, no WebKit, no Gecko. Just under **83,000 lines** of original Rust code across **38 crates**, handling everything from HTML parsing to GPU compositing.

Why build our own engine?
- **Full control** — We can innovate on features other browsers can't touch
- **Memory safety** — Rust prevents entire classes of security vulnerabilities
- **Minimal footprint** — No legacy code, no compatibility cruft
- **True independence** — We don't inherit another engine's priorities or limitations

---

## Engine Status — Windows

*Measured on `master`, 2026-07-30. Numbers come from CI, not from hand-editing
this file — see [How these numbers are produced](#how-these-numbers-are-produced).*

| | |
|---|---|
| Build | **passing** (`cargo build --workspace`, 0 errors) |
| Tests | **869 passing**, 0 failing, 5 ignored |
| Rust source | ~82,900 lines across 38 crates |
| Visual parity vs Chrome | **not measured on Windows** — see below |

### What landed recently

The Windows engine gained a substantial amount of layout and CSS capability in
late July 2026, ported from the macOS tree as contract ports rather than
line-diff copies:

- **Unicode text algorithms** — bidi (UAX #9), line breaking (UAX #14), grapheme
  and word segmentation. Mandatory line breaks (LF, CRLF, VT, FF, U+0085,
  U+2028, U+2029) are now honoured, so `white-space: pre` and `pre-line` preserve hard breaks.
- **CSS type coverage** — transforms (`TransformList`/`TransformOp`, 2D affine
  matrices), box-shadow and backdrop-filter, animation timing functions,
  background layer properties, and high-precision `ColorF32` with
  premultiplied and gamma-correct interpolation.
- **Length units** — viewport units (`vw`/`vh`/`vmin`/`vmax`) and the CSS math
  functions `min()`, `max()`, `clamp()`.
- **Layout** — CSS 2.1 §8.3.1 margin collapsing, an epoch-based intrinsic-size
  cache, and grid track sizing that credits spanned gutters.
- **A `rem` parsing bug** that silently dropped *every* `rem` length is fixed.

### Known gaps — stated, not hidden

- **No visual-parity number for Windows.** The parity harness needs a real GPU
  adapter; run headless on a machine without one it reports a confident-looking
  100% diff that measures the runner rather than the renderer. Rather than
  publish that, this row stays empty until a GPU-capable runner exists.
- **Gradients** use a Windows-local representation that differs from the macOS
  tree; unifying them is deferred until the renderer work so representation and
  consumer move together.
- **Last-child margin collapsing** over-includes in one case, documented in
  [`docs/LEDGER_MARGIN_COLLAPSE_DIVERGENCE.md`](docs/LEDGER_MARGIN_COLLAPSE_DIVERGENCE.md)
  with the condition required to fix it safely.
- Ported modules are registered and tested but **not all are wired into the
  render path yet** — a passing test count is not a claim about what you see
  on screen.

### How these numbers are produced

Every push runs [`.github/workflows/metrics.yml`](.github/workflows/metrics.yml),
which builds the workspace, runs the full test suite, attributes results
per crate, and flags any crate compiling green while executing **zero** tests.
Each run on `master` appends a row to `metrics/history.csv` on the
[`metrics-history`](https://github.com/hiwavebrowser/hiwave-windows/tree/metrics-history)
branch, so the figures above are auditable rather than asserted.

Deliberately **not** collected: the visual-parity diff, for the reason above. The
metrics JSON records that omission explicitly instead of defaulting it to a
number.

---

## Features

### 🗂️ The Shelf
Park tabs for later without leaving them open. Shelved items show their age, naturally fading so forgotten pages don't haunt you forever.

### ⏰ Tab Decay
Unused tabs gradually fade, giving you visual cues about what's actually important. In Flow mode, old tabs automatically shelve themselves.

### 🛡️ Flow Shield
Native ad and tracker blocking powered by Brave's engine. No extension required. Just fast, private browsing out of the box.

### 🔐 Flow Vault
Built-in password manager with AES-256 encryption. Your credentials stay local and secure.

### 🗃️ Workspaces
Separate your browsing contexts completely. Work tabs stay in Work, personal stays in Personal. Switch instantly with keyboard shortcuts.

### ⌨️ Keyboard First
Power users rejoice. Everything is accessible via keyboard:
- `Ctrl+K` — Command palette (search anything)
- `Ctrl+Shift+S` — Shelve current tab
- `Ctrl+B` — Toggle sidebar
- `Ctrl+1-9` — Jump to specific tab

### 🎛️ Three Modes
| Mode | For | What It Does |
|------|-----|--------------|
| **Essentials** | Control freaks | Manual everything |
| **Balanced** | Most people | Smart suggestions |
| **Flow** | Trust the system | Full automation |

---

## Download

### Latest Release

| Platform | Download |
|----------|----------|
| Windows (x64) | [hiwave-windows-x64.zip](#) |
| macOS (Intel) | [hiwave-macos-intel.dmg](#) |
| macOS (Apple Silicon) | [hiwave-macos-arm64.dmg](#) |
| Linux (x64) | [hiwave-linux-x64.AppImage](#) |

> **Note:** HiWave is currently in alpha. Expect some rough edges!

### Build from Source

```bash
# Prerequisites: Rust 1.75+, Visual Studio Build Tools (Windows)
# See INSTALL.md for detailed platform-specific instructions

git clone https://github.com/hiwavebrowser/hiwave-windows.git
cd hiwave-windows

# Build with RustKit engine (default)
cargo build --release -p hiwave-app

# Run
cargo run --release -p hiwave-app
```

> **Note:** RustKit is our custom Rust-native browser engine rendering all web content.

### Run Modes

HiWave supports multiple rendering modes on Windows:

| Mode | Command | Description |
|------|---------|-------------|
| **RustKit Hybrid** (default) | `cargo run --release` or `.\scripts\run-rustkit.ps1` | RustKit for content, WebView2 for chrome UI |
| **WebView2 Fallback** | `.\scripts\run-webview2.ps1` | System WebView2 for all rendering |
| **Native Win32** (experimental) | `.\scripts\run-native-win32.ps1` | 100% RustKit (work in progress) |

#### RustKit Hybrid Mode (Default) ⭐
```powershell
# Using convenience script
.\scripts\run-rustkit.ps1

# Or directly with cargo
cargo run --release -p hiwave-app
cargo run -p hiwave-app --no-default-features --features rustkit
```

Hybrid mode uses **RustKit for all web content**, WebView2 only for browser chrome:
- ✅ **All websites rendered by RustKit** - Wikipedia, Twitter, YouTube, etc.
- ✅ Engine-level ad/tracker blocking via Brave's adblock-rust
- ✅ Memory-safe Rust rendering pipeline
- 🎨 Browser chrome (tabs, address bar) uses WebView2 for stability
- ⚡ Best balance of performance and compatibility

**This is what the demo video shows!**

#### WebView2 Fallback Mode
```powershell
.\scripts\run-webview2.ps1

# Or directly with cargo
cargo run -p hiwave-app --no-default-features --features webview-fallback
```

WebView2 fallback uses Microsoft Edge WebView2 for all rendering:
- ✅ Maximum web compatibility
- 🔍 Useful for debugging RustKit-specific issues
- 🌐 Full Chromium rendering support

#### Native Win32 Mode (Experimental)
```powershell
.\scripts\run-native-win32.ps1

# Or directly with cargo
cargo run -p hiwave-app --no-default-features --features native-win32
```

**Status:** Work in progress. This mode aims to use RustKit for everything (chrome UI + content):
- 🚧 Currently being developed
- 🚀 Will use 100% Rust rendering when complete
- 🔧 No wry/tao/WebView2 dependencies
- ⚡ Fastest startup and lowest memory when done

**Want to help?** See [NATIVE-WIN32-IMPLEMENTATION.md](NATIVE-WIN32-IMPLEMENTATION.md) for details.

---

## Philosophy

### Attention over Tabs
We don't measure success by how many tabs you open. We measure it by how focused you stay.

### Simplicity over Extensibility  
No extension ecosystem. Features are built-in, tested, and integrated. One browser, one experience.

### Privacy by Default
Tracking protection isn't an add-on, it's foundational. We don't collect your data. Period.

### Modern Web Only
We target post-2020 web standards. No legacy cruft, no compatibility hacks for sites that should've been updated years ago.

### Opinionated but Respectful
We have strong opinions about how browsing should work, but we offer three modes so you can choose your level of buy-in.

---

## Screenshots

<p align="center">
  <em>Coming soon — the UI is still evolving!</em>
</p>

---

## Roadmap

### ✅ Completed - RustKit Engine (Phases 0-30)
- ✅ Core browsing (tabs, navigation, address bar)
- ✅ The Shelf with decay visualization
- ✅ Workspaces
- ✅ Flow Shield (ad blocking)
- ✅ Flow Vault (password manager)
- ✅ Command palette
- ✅ **RustKit browser engine** (All 30 Phases Complete! 🎉)
  - ✅ HTML parsing & DOM (rustkit-dom, rustkit-html)
  - ✅ CSS parsing & styling (rustkit-css, rustkit-cssparser)
  - ✅ Block/inline/flex/grid layout (rustkit-layout)
  - ✅ Text rendering with DirectWrite (rustkit-text)
  - ✅ JavaScript (Boa engine - rustkit-js)
  - ✅ Networking & downloads (rustkit-http, rustkit-net)
  - ✅ Event handling (mouse, keyboard, touch, pointer)
  - ✅ Forms & input (text, buttons, validation)
  - ✅ Images & media (PNG/JPEG/GIF/WebP via rustkit-codecs)
  - ✅ Scrolling & overflow
  - ✅ Navigation & History API
  - ✅ Security (CSP, CORS, same-origin)
  - ✅ CSS Animations & Transitions
  - ✅ SVG support (rustkit-svg)
  - ✅ Canvas 2D API (rustkit-canvas)
  - ✅ WebGL 1.0 (rustkit-webgl)
  - ✅ Audio/Video elements (rustkit-media)
  - ✅ Service Workers (rustkit-sw)
  - ✅ IndexedDB (rustkit-idb)
  - ✅ Web Workers (rustkit-worker)
  - ✅ Accessibility (rustkit-a11y)
- ✅ **Independence Project - Bravo Phases**
  - ✅ Bravo 1: `rustkit-cssparser` (replaced `cssparser`)
  - ✅ Bravo 2: `rustkit-text` (replaced `dwrote`)
  - ✅ Bravo 3: `rustkit-codecs` (replaced `image` crate)
  - ✅ Bravo 4: `rustkit-http` (replaced `reqwest`)
  - ✅ Bravo 6: `rustkit-html` (replaced `html5ever`)

### Now (Beta)
- 🔄 Find in Page (Ctrl+F)
- 🔄 Bookmarks & history
- 🔄 Context menus
- 🔄 Import from Chrome/Firefox

### Future
- [ ] Bravo 5: `rustkit-gpu` (replace wgpu)
- [ ] HiWave Sync (cross-device)
- [ ] Reader Mode
- [ ] Mobile companion
- [ ] WebRTC
- [ ] WebAssembly

See [docs/RUSTKIT-ROADMAP.md](docs/RUSTKIT-ROADMAP.md) for the complete engine roadmap.

---

## Contributing

We welcome contributions! See [CONTRIBUTING.md](CONTRIBUTING.md) for:
- Development setup
- Code style guidelines
- Pull request process
- Areas where we need help

**Quick Start:**
```bash
cargo test --workspace        # Run tests
cargo fmt                     # Format code
cargo clippy                  # Lint
```

---

## Support HiWave's Development

HiWave is **free and open source**. No ads, no tracking, no data selling.

If HiWave helps you focus better, consider supporting its development:

<p align="center">
  <a href="https://github.com/sponsors/YOUR_USERNAME">
    <img src="https://img.shields.io/badge/sponsor-GitHub%20Sponsors-ea4aaa" alt="GitHub Sponsors" />
  </a>
  <a href="https://ko-fi.com/hiwavebrowser">
    <img src="https://img.shields.io/badge/support-Ko--fi-ff5e5b" alt="Ko-fi" />
  </a>
  <a href="https://opencollective.com/hiwavebrowser">
    <img src="https://img.shields.io/badge/support-Open%20Collective-3385ff" alt="Open Collective" />
  </a>
</p>

Your support helps cover:
- Development time
- Infrastructure costs
- Future features like HiWave Sync

---

## Architecture

HiWave is built on **RustKit**, our ground-up browser engine. This isn't a WebKit/Chromium wrapper — it's original code:

```
┌─────────────────────────────────────────┐
│  Chrome Layer (Browser UI)              │
│  Tabs • Address Bar • Sidebar           │
├─────────────────────────────────────────┤
│                                         │
│  RustKit Engine (Original)              │
│  HTML → DOM → CSS → Layout → Paint      │
│                                         │
└─────────────────────────────────────────┘
```

### What We Built (Original Rust Code)

| Component | What It Does |
|-----------|--------------|
| `rustkit-html` | HTML5 tokenizer & tree builder (40+ states, 23 insertion modes) |
| `rustkit-css` | CSS parser, cascade, selector matching |
| `rustkit-layout` | Block, inline, flexbox, and grid layout |
| `rustkit-dom` | DOM tree, events, manipulation |
| `rustkit-compositor` | GPU rendering pipeline |
| `rustkit-http` | HTTP/1.1 client with TLS |
| `rustkit-codecs` | PNG/JPEG/GIF/WebP image decoding |
| `rustkit-text` | Text shaping and rendering |

### External Dependencies (Minimal)

We use a handful of well-maintained crates for things that don't make sense to rewrite:
- **Boa** — JavaScript engine (excellent Rust-native implementation)
- **wgpu** — GPU abstraction (may replace with `rustkit-gpu`)
- **wry/tao** — Window management for browser chrome
- **native-tls** — TLS via OS crypto libraries
- **adblock** — Brave's filter list engine
- **rusqlite** — SQLite for storage

See [docs/RUSTKIT-ROADMAP.md](docs/RUSTKIT-ROADMAP.md) for the complete engine roadmap.

---

## License

HiWave is **dual-licensed**:

| License | Best For |
|---------|----------|
| [MPL-2.0](LICENSE) (free) | Most users, open source projects |
| [Commercial](COMMERCIAL-LICENSE.md) (paid) | Keeping modifications private, dedicated support |

**Under MPL-2.0:**
- ✅ Free to use, modify, and distribute
- ✅ Build commercial products
- ✅ Keep your own code proprietary
- ⚠️ Changes to HiWave's files must be shared under MPL-2.0

**Need more flexibility?** See [COMMERCIAL-LICENSE.md](COMMERCIAL-LICENSE.md) for commercial licensing options.

**Want to contribute?** By submitting code, you agree to our [Contributor License Agreement](CLA.md).

---

## FAQ

**Q: Why not just use Firefox/Brave/Arc?**
A: They're great browsers! But they all share engines (Gecko, Blink) with decades of legacy code. HiWave's RustKit engine is written from scratch — no inherited complexity, no compatibility hacks. Plus, none of them have The Shelf, tab decay, or our philosophy around reducing cognitive load.

**Q: You built your own browser engine? Really?**
A: Yes. RustKit is ~50,000 lines of original Rust code. We wrote our own HTML parser, CSS engine, layout system, and more. It's not a WebKit fork or Chromium wrapper.

**Q: Is this production-ready?**  
A: Not yet. We're in alpha. Use it as a secondary browser while we iron out the kinks.

**Q: Will there be a mobile version?**  
A: Eventually! Desktop is the priority for now.

**Q: How do you make money?**  
A: We don't yet. Future plans include optional HiWave Sync (paid) and possibly search partnerships. We will never sell your data or show ads.

---

<p align="center">
  <strong>Built with 💜 for people who want to focus.</strong>
</p>

<p align="center">
  <a href="https://www.hiwavebrowser.com">Website</a> •
  <a href="https://github.com/hiwavebrowser/hiwave-windows">GitHub</a> •
  <a href="https://twitter.com/hiwavebrowser">Twitter</a>
</p>
