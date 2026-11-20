#!/usr/bin/env python3
"""QB-ARCHIVE-INPUT: keep what each real-site board load received, and replay it offline.

The board (scripts/realsite_board.py) records both sides of every site: RustKit's load with
`parity-capture --dump-har` and the Chrome oracle's capture `a` with Playwright's recordHar
(tools/parity_oracle/realsite.mjs). `archive_inputs` turns each raw recording into:

  <side>.har.gz          the HAR, gzipped, with image/video/audio bodies left out (the entry stays,
                         `content._skipped: "media"`; SVG is kept; REALSITE_HAR_KEEP_IMAGES=1 keeps
                         images) and the side's bodies capped (default 50 MB,
                         REALSITE_HAR_CAP_MB): past the cap the largest bodies go first
                         (`_skipped: "cap"`), never the main document's
  <side>-document.html   the main document that side received (after redirects)

Secrets: whatever wrote the HAR (RustKit already leaves them out; Playwright's recordHar does not),
`slim_har` keeps only allow-listed request headers (REQUEST_HEADERS: accept, content-type, range,
referer, user-agent, origin, sec-fetch-*, ...), drops the Cookie(2), Set-Cookie(2), Authorization
and Proxy-Authorization response headers, empties the request and response cookie arrays, and drops request POST bodies (`postData` keeps its mimeType).
Query strings and response bodies are kept whole: replay matches exact URLs and serves the bodies.
Both browser contexts are fresh (no account), but the archive still holds per-visit identifiers the
sites put in URLs and pages, so HARs belong in a non-git folder with short retention, not in git.
The raw, unfiltered recording is removed whatever happens.

and returns what it kept for the site record (`rec["inputs"][side]`). REALSITE_ARCHIVE_INPUTS=0
turns the recording off.

Replay (bisecting a lost point offline, no live load):

  python3 scripts/realsite_har.py replay --har RUN/SITE/rustkit.har.gz \\
      --capture-bin BIN --out DIR [--url URL] [--server tools/replay/server.py]

starts the loopback HAR replay server on the archived HAR (the handtest one,
~/Repos/.worktrees/handtest-runs/replay-tools-727/tools/replay/server.py, or REALSITE_REPLAY_SERVER)
and runs BIN with the board's arguments and `--replay-proxy`, writing DIR/rustkit.ppm,
DIR/rustkit-display-list.json, DIR/misses.txt and DIR/replay.json. Run it once per candidate
binary and compare the frames (or `node tools/parity_oracle/realsite.mjs diff` against the
archived chrome.png). Requests the HAR does not hold are answered 404 and listed in misses.txt;
media bodies were not archived, so images replay as misses. The replay starts at the main
document the recorded redirects led to (`--url` overrides it): the capture follows a redirect
itself, straight to its target rather than through the replay proxy.
"""

import argparse
import base64
import gzip
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from pathlib import Path

DEFAULT_CAP_MB = 50
MEDIA_PREFIXES = ("image/", "video/", "audio/")
# Small text that drives layout (viewBox-sized icons): kept like fonts, not dropped as media.
KEPT_MEDIA = ("image/svg+xml",)
# Response headers never archived (replay serves the rest as recorded).
SECRET_HEADERS = frozenset({"cookie", "cookie2", "set-cookie", "set-cookie2", "authorization",
                            "proxy-authorization"})
# Request headers archived: only what replay or a diagnosis needs. Every other request header
# (x-csrf-token, x-api-key, x-amz-security-token, ...) goes, whatever it is called. Replay matches
# on method + URL and does not need request headers at all. Mirrored in rustkit-net har.rs.
REQUEST_HEADERS = frozenset({":method", ":path", ":authority", ":scheme", "host", "accept",
                             "accept-language", "accept-encoding", "content-type", "range",
                             "referer", "user-agent", "origin"})
REQUEST_HEADER_PREFIXES = ("sec-fetch-",)


def request_header_kept(name):
    name = str(name).lower()
    return name in REQUEST_HEADERS or name.startswith(REQUEST_HEADER_PREFIXES)
DEFAULT_SERVER = (
    Path.home() / "Repos/.worktrees/handtest-runs/replay-tools-727/tools/replay/server.py"
)


def archive_enabled():
    return os.environ.get("REALSITE_ARCHIVE_INPUTS", "1") != "0"


def keep_images():
    """REALSITE_HAR_KEEP_IMAGES=1: keep image bodies (within the cap); video and audio never."""
    return os.environ.get("REALSITE_HAR_KEEP_IMAGES", "0") == "1"


def cap_bytes():
    try:
        return int(float(os.environ.get("REALSITE_HAR_CAP_MB", DEFAULT_CAP_MB)) * 2**20)
    except ValueError:
        return DEFAULT_CAP_MB * 2**20


def _mime(entry):
    return ((entry.get("response") or {}).get("content") or {}).get("mimeType") or ""


def _body_len(content):
    text = content.get("text")
    if text is None:
        return 0
    return len(text.encode("utf-8")) if content.get("encoding") != "base64" else len(text) * 3 // 4


def is_media(mime, keep_images=False):
    """Whether a body of this type is left out: image, video and audio, except SVG (and images when
    `keep_images`)."""
    mime = mime.lower().split(";")[0].strip()
    if not mime.startswith(MEDIA_PREFIXES) or mime in KEPT_MEDIA:
        return False
    return not (keep_images and mime.startswith("image/"))


def strip_secrets(entry):
    """Keep only allow-listed request headers, drop cookie and authorization response headers,
    empty the cookie arrays and drop the request's POST body."""
    for part in ("request", "response"):
        msg = entry.get(part)
        if not isinstance(msg, dict):
            continue
        if part == "request":
            keep = lambda h: request_header_kept(h.get("name", ""))  # noqa: E731
        else:
            keep = lambda h: str(h.get("name", "")).lower() not in SECRET_HEADERS  # noqa: E731
        msg["headers"] = [h for h in msg.get("headers") or [] if keep(h)]
        msg["cookies"] = []
    req = entry.get("request")
    if isinstance(req, dict) and "postData" in req:
        post = req.pop("postData") or {}
        req["postData"] = {"mimeType": post.get("mimeType", "")}


def _main_document_index(entries):
    for i, e in enumerate(entries):
        resp = e.get("response") or {}
        status = resp.get("status") or 0
        if 200 <= status < 300 and "html" in _mime(e).lower() and \
                (resp.get("content") or {}).get("text") is not None:
            return i
    return None


def slim_har(har, cap_bytes, keep_images=False):
    """Strip secrets, leave media bodies out and cap the rest; returns (har, stats). Every entry
    stays. The main document's body is never capped."""
    entries = (har.get("log") or {}).get("entries") or []
    stats = {"entries": len(entries), "skipped_media": 0, "skipped_cap": 0, "capped": False}
    kept = []
    for e in entries:
        strip_secrets(e)
        content = (e.setdefault("response", {})).setdefault("content", {})
        if "text" in content and is_media(_mime(e), keep_images):
            content.pop("text", None)
            content.pop("encoding", None)
            content["_skipped"] = "media"
            stats["skipped_media"] += 1
        elif "text" in content:
            kept.append(content)
    doc = _main_document_index(entries)
    doc_content = entries[doc]["response"]["content"] if doc is not None else None
    total = sum(_body_len(c) for c in kept)
    for content in sorted(kept, key=_body_len, reverse=True):
        if total <= cap_bytes:
            break
        if content is doc_content:
            continue
        total -= _body_len(content)
        content.pop("text", None)
        content.pop("encoding", None)
        content["_skipped"] = "cap"
        stats["skipped_cap"] += 1
        stats["capped"] = True
    stats["body_bytes"] = total
    return har, stats


def main_document(har):
    """(url, text) of the first 2xx HTML answer (redirect hops are passed over), or None."""
    entries = (har.get("log") or {}).get("entries") or []
    i = _main_document_index(entries)
    if i is None:
        return None
    content = entries[i]["response"]["content"]
    text = content["text"]
    if content.get("encoding") == "base64":
        text = base64.b64decode(text).decode("utf-8", errors="replace")
    return (entries[i].get("request") or {}).get("url"), text


def archive_inputs(site_dir, side, raw_har, cap_bytes, keep_images=None):
    """Slim and gzip `raw_har` into SITE/<side>.har.gz, write SITE/<side>-document.html, remove the
    raw file whatever happens; returns what was kept (or `{"error": ...}`)."""
    site_dir, raw_har = Path(site_dir), Path(raw_har)
    if not raw_har.exists():
        return {"error": "no recording"}
    out = site_dir / ("%s.har.gz" % side)
    try:
        try:
            har = json.loads(raw_har.read_text(encoding="utf-8", errors="replace"))
        except (OSError, json.JSONDecodeError) as e:
            return {"error": "unreadable recording: %s" % str(e)[:120]}
        if keep_images is None:
            keep_images = globals()["keep_images"]()
        har, stats = slim_har(har, cap_bytes, keep_images)
        try:
            with gzip.open(out, "wt", encoding="utf-8", compresslevel=6) as f:
                json.dump(har, f, separators=(",", ":"))
            stats["har"] = out.name
            stats["har_gz_bytes"] = out.stat().st_size
            doc = main_document(har)
            if doc:
                (site_dir / ("%s-document.html" % side)).write_text(doc[1], encoding="utf-8")
                stats["document"] = doc[0]
        except OSError as e:
            out.unlink(missing_ok=True)
            return {"error": "archive not written: %s" % str(e)[:120]}
        return stats
    finally:
        raw_har.unlink(missing_ok=True)


# ---------------------------------------------------------------- replay


def replay_site(har_gz, url, capture_bin, out_dir, server=None, width=1280, height=800,
                timeout_ms=30000, extra_args=()):
    """Load `url` with `capture_bin` through the loopback replay server serving `har_gz`."""
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    server = Path(server or os.environ.get("REALSITE_REPLAY_SERVER") or DEFAULT_SERVER)
    if not server.exists():
        return {"status": "error", "error": "no replay server at %s" % server}
    with tempfile.TemporaryDirectory() as td:
        har_path = Path(td) / "site.har"
        opener = gzip.open if str(har_gz).endswith(".gz") else open
        with opener(har_gz, "rt", encoding="utf-8") as src:
            text = src.read()
        har_path.write_text(text, encoding="utf-8")
        # The capture's own redirect hops go straight to their target, not through the replay
        # proxy, so a replay starts at the document the redirects led to.
        if not url:
            doc = main_document(json.loads(text))
            if not doc:
                return {"status": "error", "error": "no HTML document in the HAR to start from"}
            url = doc[0]
        misses = out / "misses.txt"
        server_log = open(out / "server.log", "w", encoding="utf-8")
        srv = subprocess.Popen(
            [sys.executable, str(server), str(har_path), "--port", "0", "--time-scale", "0",
             "--miss-log", str(misses), "--no-tls"],
            stdout=subprocess.PIPE, stderr=server_log, text=True,
            cwd=str(server.parent), encoding="utf-8"
        )
        try:
            line = srv.stdout.readline().strip()
            m = re.search(r"(http://127\.0\.0\.1:\d+)", line)
            if not m:
                return {"status": "error", "error": "replay server did not start: %r" % line}
            proxy = m.group(1)
            frame, dl = out / "rustkit.ppm", out / "rustkit-display-list.json"
            cmd = [str(capture_bin), "--url", url, "--replay-proxy", proxy, "--width", str(width),
                   "--height", str(height), "--timeout-ms", str(timeout_ms), "--dump-frame", str(frame),
                   "--dump-display-list", str(dl), *extra_args]
            started = time.time()
            try:
                p = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout_ms / 1000 + 15, encoding="utf-8")
                lines = [l for l in p.stdout.splitlines() if l.strip().startswith("{")]
                capture = json.loads(lines[-1]) if lines else {"status": "crash", "stderr": p.stderr[-500:]}
            except subprocess.TimeoutExpired:
                capture = {"status": "timeout"}
        finally:
            srv.terminate()
            try:
                srv.wait(5)
            except subprocess.TimeoutExpired:
                srv.kill()
                srv.wait(5)
            srv.stdout.close()
            server_log.close()
    missed = [l for l in misses.read_text(encoding="utf-8").splitlines() if l.strip()] if misses.exists() else []
    res = {"status": capture.get("status", "error"), "proxy": proxy, "misses": len(missed),
           "wall_s": round(time.time() - started, 1), "capture": capture}
    (out / "replay.json").write_text(json.dumps(res, indent=1), encoding="utf-8")
    return res


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("replay", help="load a site from its archived HAR through the replay server")
    r.add_argument("--har", required=True, help="RUN/SITE/rustkit.har.gz (or chrome.har.gz)")
    r.add_argument("--url", help="default: the main document the HAR's redirects led to "
                                    "(redirect hops do not go through the replay proxy)")
    r.add_argument("--capture-bin", required=True)
    r.add_argument("--out", required=True)
    r.add_argument("--server", help="tools/replay/server.py (default: REALSITE_REPLAY_SERVER or handtest's)")
    r.add_argument("--width", type=int, default=1280)
    r.add_argument("--height", type=int, default=800)
    r.add_argument("--timeout-ms", type=int, default=30000)
    a = ap.parse_args()
    res = replay_site(a.har, a.url, a.capture_bin, a.out, a.server, a.width, a.height, a.timeout_ms)
    print(json.dumps({k: v for k, v in res.items() if k != "capture"}))
    return 0 if res["status"] == "ok" else 1


if __name__ == "__main__":
    sys.exit(main())
