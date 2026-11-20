#!/usr/bin/env python3
"""QB-ARCHIVE-INPUT: the real-site board keeps each site's inputs, so a lost point can be bisected
offline.

For each site the board now writes, beside the frames:
  rustkit.har.gz / chrome.har.gz             the RustKit load (parity-capture --dump-har) and the
                                             Chrome oracle's load (Playwright recordHar), gzipped
  rustkit-document.html / chrome-document.html  the main document each side received
Bodies of images, video and audio are left out (the entry stays, with `_skipped: media`), and a
side's bodies are capped (default 50 MB): past the cap the largest bodies go first
(`_skipped: cap`). `realsite_har.py replay` loads a site from its archived HAR through the loopback
replay server (parity-capture --replay-proxy).

No browser, no network beyond loopback: captures and the replay server are stubbed.
"""

import base64
import gzip
import json
import os
import socket
import subprocess
import sys
import tempfile
import textwrap
import unittest
from pathlib import Path
from unittest import mock

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO / "scripts"))

import realsite_board  # noqa: E402
import realsite_har  # noqa: E402


def entry(url, status=200, mime="text/html", text=None, b64=None, location=None):
    content = {"size": len(text or "") if b64 is None else len(base64.b64decode(b64)), "mimeType": mime}
    if text is not None:
        content["text"] = text
    if b64 is not None:
        content["text"] = b64
        content["encoding"] = "base64"
    headers = [{"name": "content-type", "value": mime}]
    if location:
        headers.append({"name": "location", "value": location})
    return {
        "startedDateTime": "2026-11-20T09:00:00.000Z",
        "time": 1,
        "request": {"method": "GET", "url": url, "headers": []},
        "response": {"status": status, "headers": headers, "content": content},
    }


def har(*entries):
    return {"log": {"version": "1.2", "creator": {"name": "t"}, "entries": list(entries)}}


PNG = base64.b64encode(b"\x89PNG" + b"\0" * 100).decode()


class SlimTest(unittest.TestCase):
    def test_media_bodies_are_left_out_and_text_kept(self):
        h = har(
            entry("https://a.test/", text="<p>hi</p>"),
            entry("https://a.test/i.png", mime="image/png", b64=PNG),
            entry("https://a.test/v.mp4", mime="video/mp4", b64=PNG),
            entry("https://a.test/s.mp3", mime="audio/mpeg", b64=PNG),
            entry("https://a.test/f.woff2", mime="font/woff2", b64=PNG),
            entry("https://a.test/a.js", mime="application/javascript", text="x=1"),
        )
        slim, stats = realsite_har.slim_har(h, cap_bytes=10_000_000)
        bodies = {e["request"]["url"]: e["response"]["content"] for e in slim["log"]["entries"]}
        self.assertEqual(len(bodies), 6, "every entry stays")
        for u in ("i.png", "v.mp4", "s.mp3"):
            c = bodies["https://a.test/" + u]
            self.assertNotIn("text", c)
            self.assertEqual(c["_skipped"], "media")
        self.assertEqual(bodies["https://a.test/"]["text"], "<p>hi</p>")
        self.assertIn("text", bodies["https://a.test/f.woff2"], "fonts are kept")
        self.assertEqual(stats["skipped_media"], 3)
        self.assertEqual(stats["entries"], 6)

    def test_the_cap_drops_the_largest_bodies_first(self):
        h = har(
            entry("https://a.test/", text="d" * 100),
            entry("https://a.test/big.js", mime="application/javascript", text="b" * 5000),
            entry("https://a.test/mid.js", mime="application/javascript", text="m" * 2000),
            entry("https://a.test/small.css", mime="text/css", text="s" * 300),
        )
        slim, stats = realsite_har.slim_har(h, cap_bytes=2500)
        kept = {e["request"]["url"]: "text" in e["response"]["content"] for e in slim["log"]["entries"]}
        self.assertEqual(
            kept,
            {
                "https://a.test/": True,
                "https://a.test/big.js": False,
                "https://a.test/mid.js": True,
                "https://a.test/small.css": True,
            },
        )
        self.assertEqual(stats["skipped_cap"], 1)
        self.assertLessEqual(stats["body_bytes"], 2500)
        self.assertTrue(stats["capped"])


class DocumentTest(unittest.TestCase):
    def test_the_main_document_follows_redirects_to_the_first_html_answer(self):
        h = har(
            entry("https://a.test/", status=301, mime="", location="https://www.a.test/"),
            entry("https://www.a.test/", text="<html>main</html>"),
            entry("https://www.a.test/frame.html", text="<html>frame</html>"),
        )
        self.assertEqual(realsite_har.main_document(h), ("https://www.a.test/", "<html>main</html>"))

    def test_a_base64_document_is_decoded(self):
        body = base64.b64encode("<html>é</html>".encode()).decode()
        h = har(entry("https://a.test/", mime="text/html; charset=utf-8", b64=body))
        self.assertEqual(realsite_har.main_document(h), ("https://a.test/", "<html>é</html>"))

    def test_no_html_answer_is_none(self):
        self.assertIsNone(realsite_har.main_document(har(entry("https://a.test/x", status=404))))


class ArchiveTest(unittest.TestCase):
    def test_a_side_is_archived_gzipped_with_its_document_and_the_raw_file_goes(self):
        with tempfile.TemporaryDirectory() as td:
            d = Path(td)
            raw = d / "rustkit.raw.har"
            raw.write_text(json.dumps(har(entry("https://a.test/", text="<html>x</html>"),
                                          entry("https://a.test/i.png", mime="image/png", b64=PNG))), encoding="utf-8")
            stats = realsite_har.archive_inputs(d, "rustkit", raw, cap_bytes=50 * 2**20)
            self.assertFalse(raw.exists())
            with gzip.open(d / "rustkit.har.gz", "rt", encoding="utf-8") as f:
                back = json.load(f)
            self.assertEqual(len(back["log"]["entries"]), 2)
            self.assertEqual((d / "rustkit-document.html").read_text(encoding="utf-8"), "<html>x</html>")
            self.assertEqual(stats["document"], "https://a.test/")
            self.assertEqual(stats["skipped_media"], 1)
            self.assertEqual(stats["har"], "rustkit.har.gz")

    def test_images_can_be_kept_but_video_and_audio_never(self):
        with tempfile.TemporaryDirectory() as td, mock.patch.dict(os.environ, {"REALSITE_HAR_KEEP_IMAGES": "1"}):
            d = Path(td)
            raw = d / "rustkit.raw.har"
            raw.write_text(json.dumps(har(entry("https://a.test/i.png", mime="image/png", b64=PNG),
                                          entry("https://a.test/v.mp4", mime="video/mp4", b64=PNG))), encoding="utf-8")
            stats = realsite_har.archive_inputs(d, "rustkit", raw, cap_bytes=50 * 2**20)
            self.assertEqual(stats["skipped_media"], 1)
            with gzip.open(d / "rustkit.har.gz", "rt", encoding="utf-8") as f:
                kept = ["text" in e["response"]["content"] for e in json.load(f)["log"]["entries"]]
            self.assertEqual(kept, [True, False])

    def test_a_missing_or_broken_recording_is_reported_not_raised(self):
        with tempfile.TemporaryDirectory() as td:
            d = Path(td)
            self.assertEqual(realsite_har.archive_inputs(d, "chrome", d / "none.har", 1000)["error"], "no recording")
            bad = d / "bad.har"
            bad.write_text("{not json", encoding="utf-8")
            self.assertIn("error", realsite_har.archive_inputs(d, "chrome", bad, 1000))


SECRET = "s3cr3t-value"


def playwright_entry(url, method="GET", mime="text/html", text="<html>x</html>", post=None):
    """An entry as Playwright's recordHar (mode full) writes it: cookies, auth and POST bodies."""
    e = entry(url, mime=mime, text=text)
    e["request"].update({
        "method": method,
        "headers": [
            {"name": "Cookie", "value": "sid=" + SECRET},
            {"name": "Authorization", "value": "Bearer " + SECRET},
            {"name": "proxy-authorization", "value": "Basic " + SECRET},
            {"name": "Accept", "value": "text/html"},
        ],
        "cookies": [{"name": "sid", "value": SECRET}],
    })
    if post is not None:
        e["request"]["postData"] = {"mimeType": "application/json", "text": post,
                                    "params": [{"name": "k", "value": SECRET}]}
    e["response"]["headers"] += [
        {"name": "Set-Cookie", "value": "sid=%s; Path=/" % SECRET},
        {"name": "SET-COOKIE", "value": "other=%s" % SECRET},
    ]
    e["response"]["cookies"] = [{"name": "sid", "value": SECRET, "httpOnly": True}]
    return e


class SecretsTest(unittest.TestCase):
    """Neither side's archived HAR keeps cookies, authorization or POST bodies, whatever wrote it."""

    def test_no_cookie_authorization_or_post_body_survives_on_either_side(self):
        for side in ("chrome", "rustkit"):
            with self.subTest(side=side), tempfile.TemporaryDirectory() as td:
                d = Path(td)
                raw = d / ("%s.raw.har" % side)
                raw.write_text(json.dumps(har(
                    playwright_entry("https://a.test/"),
                    playwright_entry("https://a.test/beacon", method="POST", mime="application/json",
                                     text="{}", post='{"user":"%s"}' % SECRET),
                )), encoding="utf-8")
                realsite_har.archive_inputs(d, side, raw, cap_bytes=50 * 2**20)
                with gzip.open(d / ("%s.har.gz" % side), "rt", encoding="utf-8") as f:
                    text = f.read()
                self.assertNotIn(SECRET, text)
                back = json.loads(text)["log"]["entries"]
                for e in back:
                    for part in ("request", "response"):
                        names = {h["name"].lower() for h in e[part].get("headers", [])}
                        self.assertFalse(names & {"cookie", "set-cookie", "authorization",
                                                  "proxy-authorization"}, names)
                        self.assertEqual(e[part].get("cookies"), [])
                    self.assertIn("accept", {h["name"].lower() for h in e["request"]["headers"]})
                post = back[1]["request"]
                self.assertEqual(post["method"], "POST")
                self.assertEqual(post["postData"], {"mimeType": "application/json"},
                                 "the body goes, its type stays")
                self.assertNotIn("postData", back[0]["request"])


class RequestHeaderAllowListTest(unittest.TestCase):
    """Request headers are kept only when replay or diagnosis needs them; every other name goes,
    whatever credential it carries. Response headers (served by replay) lose only the deny-list."""

    KEPT = [":method", ":path", ":authority", ":scheme", "Host", "Accept", "Accept-Language",
            "Accept-Encoding", "Content-Type", "Range", "Referer", "User-Agent", "Origin",
            "Sec-Fetch-Dest", "sec-fetch-mode", "Sec-Fetch-Site", "Sec-Fetch-User"]
    DROPPED = ["X-CSRF-Token", "x-xsrf-token", "X-Api-Key", "x-auth-token", "X-Amz-Security-Token",
               "Cookie2", "Cookie", "Authorization", "Proxy-Authorization", "X-Anything-Else"]

    def test_only_allow_listed_request_headers_survive_on_either_side(self):
        for side in ("chrome", "rustkit"):
            with self.subTest(side=side), tempfile.TemporaryDirectory() as td:
                d = Path(td)
                e = entry("https://a.test/")
                e["request"]["headers"] = [{"name": n, "value": "v-" + SECRET} for n in self.KEPT + self.DROPPED]
                e["response"]["headers"] += [
                    {"name": "Set-Cookie2", "value": "a=" + SECRET},
                    {"name": "set-cookie", "value": "b=" + SECRET},
                    {"name": "X-Served-By", "value": "cache-1"},
                    {"name": "Location", "value": "https://a.test/next"},
                ]
                raw = d / ("%s.raw.har" % side)
                raw.write_text(json.dumps(har(e)), encoding="utf-8")
                realsite_har.archive_inputs(d, side, raw, cap_bytes=50 * 2**20)
                with gzip.open(d / ("%s.har.gz" % side), "rt", encoding="utf-8") as f:
                    back = json.load(f)["log"]["entries"][0]
                self.assertEqual([h["name"] for h in back["request"]["headers"]], self.KEPT)
                self.assertEqual([h["name"] for h in back["response"]["headers"]],
                                 ["content-type", "X-Served-By", "Location"])


class CapOrderTest(unittest.TestCase):
    def test_the_main_document_body_is_never_capped(self):
        h = har(
            entry("https://a.test/", status=302, mime="", location="https://a.test/home"),
            entry("https://a.test/home", text="d" * 9000),
            entry("https://a.test/a.js", mime="application/javascript", text="j" * 2000),
            entry("https://a.test/b.css", mime="text/css", text="c" * 500),
        )
        slim, stats = realsite_har.slim_har(h, cap_bytes=2500)
        kept = {e["request"]["url"]: "text" in e["response"]["content"] for e in slim["log"]["entries"]}
        self.assertTrue(kept["https://a.test/home"], "the main document keeps its body")
        self.assertEqual(realsite_har.main_document(slim), ("https://a.test/home", "d" * 9000))
        # The rest of the budget is what the document left: both others go past it.
        self.assertFalse(kept["https://a.test/a.js"])
        self.assertFalse(kept["https://a.test/b.css"])
        self.assertEqual(stats["skipped_cap"], 2)


class SvgTest(unittest.TestCase):
    def test_svg_bodies_are_kept_with_the_fonts(self):
        svg = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 8 8"/>'
        h = har(entry("https://a.test/i.svg", mime="image/svg+xml", text=svg),
                entry("https://a.test/j.svg", mime="image/svg+xml; charset=utf-8",
                      b64=base64.b64encode(svg.encode()).decode()),
                entry("https://a.test/i.png", mime="image/png", b64=PNG))
        slim, stats = realsite_har.slim_har(h, cap_bytes=10_000_000)
        kept = ["text" in e["response"]["content"] for e in slim["log"]["entries"]]
        self.assertEqual(kept, [True, True, False])
        self.assertEqual(stats["skipped_media"], 1)


class RawCleanupTest(unittest.TestCase):
    def test_the_raw_recording_goes_when_writing_the_archive_fails(self):
        with tempfile.TemporaryDirectory() as td:
            d = Path(td)
            raw = d / "chrome.raw.har"
            raw.write_text(json.dumps(har(playwright_entry("https://a.test/"))), encoding="utf-8")
            with mock.patch.object(realsite_har.gzip, "open", side_effect=OSError("disk full")):
                stats = realsite_har.archive_inputs(d, "chrome", raw, cap_bytes=50 * 2**20)
            self.assertIn("disk full", stats["error"], "reported, not raised: the board goes on")
            self.assertFalse(raw.exists(), "the unfiltered recording is not left behind")
            self.assertFalse((d / "chrome.har.gz").exists())


def write_ppm(path):
    w = h = 4
    px = bytearray()
    for y in range(h):
        for x in range(w):
            px += bytes((0, 0, 0)) if x == 0 else bytes((255, 255, 255))
    Path(path).write_bytes(b"P6\n%d %d\n255\n" % (w, h) + bytes(px))


class BoardTest(unittest.TestCase):
    def test_score_site_records_both_sides_and_archives_them(self):
        site = {"id": "s", "url": "https://a.test/"}
        calls = {}

        def fake_chrome(url, png, text_json, width, height, env, har_path=None):
            calls.setdefault("chrome_har", []).append(har_path)
            Path(png).write_bytes(b"png")
            Path(text_json).write_text(json.dumps({"url": url, "text": "hello"}), encoding="utf-8")
            if har_path:
                Path(har_path).write_text(json.dumps(har(entry(url, text="<html>chrome</html>"))), encoding="utf-8")
            return {"status": "ok", "http_status": 200, "final_url": url}

        def fake_run_json(cmd, timeout, env=None):
            calls["capture"] = cmd
            out = cmd[cmd.index("--dump-frame") + 1]
            write_ppm(out)
            Path(cmd[cmd.index("--dump-display-list") + 1]).write_text("[]", encoding="utf-8")
            Path(cmd[cmd.index("--dump-har") + 1]).write_text(
                json.dumps(har(entry("https://a.test/", text="<html>rustkit</html>"))), encoding="utf-8")
            return {"status": "ok"}, None, 0

        with tempfile.TemporaryDirectory() as td, \
                mock.patch.object(realsite_board, "chrome_capture", side_effect=fake_chrome), \
                mock.patch.object(realsite_board, "run_json", side_effect=fake_run_json), \
                mock.patch.object(realsite_board, "pixel_diff", return_value={"diffPercent": 1.0}), \
                mock.patch.object(realsite_board, "non_background_fraction", return_value=0.5), \
                mock.patch.object(realsite_board, "probe_access",
                                  return_value={"status": 200, "vendor": None, "blocked": False}), \
                mock.patch.object(realsite_board, "rustkit_viewport_text", return_value="hello"):
            out = Path(td)
            rec = realsite_board.score_site(site, "parity-capture", out, 4, 4, {})
            d = out / "s"
            # Only the oracle's capture (a) records; b is the self-noise capture.
            self.assertEqual([p is not None for p in calls["chrome_har"]], [True, False])
            self.assertIn("--dump-har", calls["capture"])
            for name in ("rustkit.har.gz", "chrome.har.gz", "rustkit-document.html", "chrome-document.html"):
                self.assertTrue((d / name).exists(), name)
            self.assertEqual((d / "chrome-document.html").read_text(encoding="utf-8"), "<html>chrome</html>")
            self.assertFalse(list(d.glob("*.raw.har")), "raw recordings are removed")
            self.assertEqual(rec["inputs"]["rustkit"]["har"], "rustkit.har.gz")
            self.assertEqual(rec["inputs"]["chrome"]["document"], "https://a.test/")
            self.assertEqual(rec["points"], 3)

    def test_archiving_can_be_turned_off(self):
        with mock.patch.dict(os.environ, {"REALSITE_ARCHIVE_INPUTS": "0"}):
            self.assertFalse(realsite_har.archive_enabled())
        with mock.patch.dict(os.environ, {}, clear=False):
            os.environ.pop("REALSITE_ARCHIVE_INPUTS", None)
            self.assertTrue(realsite_har.archive_enabled())


STUB_SERVER = textwrap.dedent(
    """
    import json, socket, sys, time
    from pathlib import Path
    args = sys.argv[1:]
    har = args[0]
    Path(sys.argv[0] + ".log").write_text(json.dumps({"argv": args, "har_entries":
        len(json.load(open(har))["log"]["entries"])}))
    s = socket.socket(); s.bind(("127.0.0.1", 0)); s.listen(1)
    print("replay server on http://127.0.0.1:%d" % s.getsockname()[1], flush=True)
    time.sleep(60)
    """
)

STUB_CAPTURE = textwrap.dedent(
    """
    import json, sys
    from pathlib import Path
    a = sys.argv[1:]
    Path(a[a.index("--dump-frame") + 1]).write_bytes(b"P6\\n1 1\\n255\\n\\0\\0\\0")
    print(json.dumps({"status": "ok", "argv": a}))
    """
)


class ReplayTest(unittest.TestCase):
    def test_replay_runs_the_capture_through_the_replay_server_with_the_archived_har(self):
        with tempfile.TemporaryDirectory() as td:
            d = Path(td)
            server = d / "server.py"
            server.write_text(STUB_SERVER, encoding="utf-8")
            capture = d / "capture.sh"
            capture.write_text("#!/bin/sh\nexec %s %s \"$@\"\n" % (sys.executable, d / "capture.py"), encoding="utf-8")
            capture.chmod(0o755)
            (d / "capture.py").write_text(STUB_CAPTURE, encoding="utf-8")
            with gzip.open(d / "rustkit.har.gz", "wt", encoding="utf-8") as f:
                json.dump(har(entry("https://a.test/", text="<html>x</html>")), f)
            out = d / "replay"
            res = realsite_har.replay_site(d / "rustkit.har.gz", "https://a.test/", str(capture), out,
                                           server=server, width=4, height=4, timeout_ms=5000)
            self.assertEqual(res["status"], "ok", res)
            argv = res["capture"]["argv"]
            proxy = argv[argv.index("--replay-proxy") + 1]
            self.assertRegex(proxy, r"^http://127\.0\.0\.1:\d+$")
            self.assertEqual(argv[argv.index("--url") + 1], "https://a.test/")
            log = json.loads((d / "server.py.log").read_text(encoding="utf-8"))
            self.assertEqual(log["har_entries"], 1, "the server got the decompressed HAR")
            self.assertIn("--miss-log", log["argv"])
            self.assertTrue((out / "rustkit.ppm").exists())
            self.assertTrue((out / "replay.json").exists())


if __name__ == "__main__":
    unittest.main()
