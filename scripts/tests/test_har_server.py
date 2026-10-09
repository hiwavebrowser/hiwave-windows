"""
test_har_server.py — Guard tests for Package Z2-M4 Local HAR Replay Server.

Validates:
1. In-memory indexing of HAR entries (exact, path, slash-tolerant, host+path).
2. Content body extraction (text and base64-decoded binary).
3. Live HTTP request serving on loopback with X-Original-URL routing.
4. Clean shutdown and error handling.
"""

import base64
import json
import sys
import unittest
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError

REPO = Path(__file__).resolve().parent.parent.parent
sys.path.insert(0, str(REPO))
sys.path.insert(0, str(REPO / "tools" / "parity_oracle"))

from tools.parity_oracle.har_server import (
    HarArchive,
    start_har_server_in_thread,
    normalize_url,
)


class TestHarServer(unittest.TestCase):
    def setUp(self):
        self.sample_har = {
            "log": {
                "version": "1.2",
                "entries": [
                    {
                        "request": {
                            "method": "GET",
                            "url": "https://example.com/test",
                            "headers": [{"name": "Host", "value": "example.com"}],
                        },
                        "response": {
                            "status": 200,
                            "statusText": "OK",
                            "headers": [
                                {"name": "Content-Type", "value": "text/html; charset=utf-8"},
                                {"name": "X-Custom-Header", "value": "hello-parity"},
                            ],
                            "content": {
                                "size": 25,
                                "mimeType": "text/html",
                                "text": "<html>Hello HAR</html>",
                            },
                        },
                    },
                    {
                        "request": {
                            "method": "GET",
                            "url": "https://cdn.example.com/image.png",
                            "headers": [{"name": "Host", "value": "cdn.example.com"}],
                        },
                        "response": {
                            "status": 200,
                            "statusText": "OK",
                            "headers": [
                                {"name": "Content-Type", "value": "image/png"},
                            ],
                            "content": {
                                "size": 4,
                                "mimeType": "image/png",
                                "encoding": "base64",
                                "text": base64.b64encode(b"\x89PNG\r\n\x1a\n").decode("ascii"),
                            },
                        },
                    },
                ]
            }
        }
        self.archive = HarArchive(self.sample_har)

    def test_normalize_url(self):
        self.assertEqual(normalize_url("https://Example.COM/foo#bar"), "https://example.com/foo")
        self.assertEqual(normalize_url("http://example.com"), "http://example.com/")

    def test_find_entry_exact(self):
        entry = self.archive.find_entry("GET", "https://example.com/test")
        self.assertIsNotNone(entry)
        self.assertEqual(entry["response"]["status"], 200)

    def test_find_entry_fuzzy(self):
        # Slash tolerance
        entry = self.archive.find_entry("GET", "https://example.com/test/")
        self.assertIsNotNone(entry)
        self.assertEqual(entry["response"]["status"], 200)

        # Host + path reconstruction
        entry = self.archive.find_entry("GET", "/test", host="example.com", path="/test")
        self.assertIsNotNone(entry)
        self.assertEqual(entry["response"]["status"], 200)

        # Missing entry
        missing = self.archive.find_entry("GET", "https://example.com/not-exists")
        self.assertIsNone(missing)

    def test_live_server_serving(self):
        # Write temporary HAR file
        har_file = REPO / "target" / "test_sample.har"
        har_file.parent.mkdir(parents=True, exist_ok=True)
        har_file.write_text(json.dumps(self.sample_har), encoding="utf-8")

        server, thread, server_url = start_har_server_in_thread(har_file)
        try:
            # 1. Fetch with X-Original-URL header
            req = Request(f"{server_url}/anything")
            req.add_header("X-Original-URL", "https://example.com/test")
            with urlopen(req, timeout=5) as resp:
                self.assertEqual(resp.status, 200)
                body = resp.read().decode("utf-8")
                self.assertEqual(body, "<html>Hello HAR</html>")
                self.assertEqual(resp.headers.get("X-Custom-Header"), "hello-parity")

            # 2. Fetch binary base64 decoded content
            req2 = Request(f"{server_url}/image.png")
            req2.add_header("Host", "cdn.example.com")
            with urlopen(req2, timeout=5) as resp:
                self.assertEqual(resp.status, 200)
                bytes_body = resp.read()
                self.assertTrue(bytes_body.startswith(b"\x89PNG"))

            # 3. Request unknown URL -> 404
            req3 = Request(f"{server_url}/not-found")
            with self.assertRaises(HTTPError) as ctx:
                urlopen(req3, timeout=5)
            self.assertEqual(ctx.exception.code, 404)

        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)


if __name__ == "__main__":
    unittest.main()
