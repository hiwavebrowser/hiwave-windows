#!/usr/bin/env python3
"""
har_server.py — Local HTTP replay server for pinned HAR archives.

Part of Package Z2-M4 (Time-Stable Real-Site Board & Deterministic Replay).
Reads a .har JSON archive, indexes network entries by (method, url), and serves
deterministic responses to RustKit/parity-capture or Playwright on loopback.

Usage:
  python har_server.py <path_to_har_file> [--host 127.0.0.1] [--port 0]

Outputs:
  Prints "HAR_SERVER_READY http://<host>:<port>" to stdout on startup and flushes.
"""

import argparse
import base64
import json
import os
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Dict, List, Optional, Tuple
from urllib.parse import urlparse, urlunparse

# Hop-by-hop headers that should not be forwarded
HOP_BY_HOP_HEADERS = {
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
    "content-length",
}


def normalize_url(url_str: str) -> str:
    """Normalize URL by stripping fragments and standardizing port/host casing."""
    try:
        parsed = urlparse(url_str)
        # Drop fragment
        scheme = parsed.scheme.lower()
        netloc = parsed.netloc.lower()
        path = parsed.path or "/"
        return urlunparse((scheme, netloc, path, parsed.params, parsed.query, ""))
    except Exception:
        return url_str.split("#")[0]


class HarArchive:
    """In-memory index of HAR network entries for rapid exact and fuzzy lookup."""

    def __init__(self, data: dict):
        self.entries: List[dict] = []
        self.by_exact: Dict[Tuple[str, str], dict] = {}
        self.by_path: Dict[Tuple[str, str], List[dict]] = {}
        self._load(data)

    @classmethod
    def from_file(cls, path: str | Path) -> "HarArchive":
        path = Path(path)
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            data = json.load(f)
        return cls(data)

    def _load(self, data: dict):
        log = data.get("log", {})
        entries = log.get("entries", [])
        self.entries = entries

        for entry in entries:
            req = entry.get("request", {})
            method = req.get("method", "GET").upper()
            raw_url = req.get("url", "")
            norm_url = normalize_url(raw_url)

            # Store in exact index
            key = (method, norm_url)
            if key not in self.by_exact:
                self.by_exact[key] = entry

            # Also index by path for fallback
            try:
                parsed = urlparse(norm_url)
                path_key = (method, parsed.path or "/")
                self.by_path.setdefault(path_key, []).append(entry)
            except Exception:
                pass

    def find_entry(
        self, method: str, url: str, host: str = "", path: str = ""
    ) -> Optional[dict]:
        method = method.upper()
        norm_url = normalize_url(url)

        # 1. Exact match
        entry = self.by_exact.get((method, norm_url))
        if entry:
            return entry

        # 2. Match without trailing slash difference
        if norm_url.endswith("/"):
            entry = self.by_exact.get((method, norm_url[:-1]))
        else:
            entry = self.by_exact.get((method, norm_url + "/"))
        if entry:
            return entry

        # 3. Match using host + path if provided
        if host and path:
            candidate_https = normalize_url(f"https://{host}{path}")
            entry = self.by_exact.get((method, candidate_https))
            if entry:
                return entry

            candidate_http = normalize_url(f"http://{host}{path}")
            entry = self.by_exact.get((method, candidate_http))
            if entry:
                return entry

        # 4. Fallback: match by path and host suffix
        parsed_req = urlparse(norm_url)
        path_key = (method, parsed_req.path or "/")
        candidates = self.by_path.get(path_key, [])
        for cand in candidates:
            cand_url = cand.get("request", {}).get("url", "")
            cand_parsed = urlparse(cand_url)
            if host and (cand_parsed.netloc.lower() == host.lower()):
                return cand
            if parsed_req.netloc and (cand_parsed.netloc.lower() == parsed_req.netloc.lower()):
                return cand

        # If only one candidate matches the path, use it as fallback
        if len(candidates) == 1:
            return candidates[0]

        return None


class HarRequestHandler(BaseHTTPRequestHandler):
    """HTTP request handler serving responses from a HarArchive."""

    server: "HarServer"

    # Suppress default log messages on stderr
    def log_message(self, format, *args):
        if os.environ.get("HAR_SERVER_VERBOSE") == "1":
            sys.stderr.write(f"[har_server] {self.address_string()} - {format % args}\n")

    def do_HEAD(self):
        self._handle_request(send_body=False)

    def do_GET(self):
        self._handle_request(send_body=True)

    def do_POST(self):
        self._handle_request(send_body=True)

    def _handle_request(self, send_body: bool = True):
        # Extract target URL:
        # Check X-Original-URL header (set by parity-capture --replay-proxy)
        orig_url = self.headers.get("X-Original-URL") or self.headers.get("x-original-url")
        host_header = self.headers.get("Host") or self.headers.get("host") or ""

        if orig_url:
            target_url = orig_url
        elif self.path.startswith("http://") or self.path.startswith("https://"):
            target_url = self.path
        elif host_header:
            target_url = f"https://{host_header}{self.path}"
        else:
            target_url = self.path

        entry = self.server.archive.find_entry(
            method=self.command,
            url=target_url,
            host=host_header.split(":")[0],
            path=self.path,
        )

        if not entry:
            body = f"HAR Entry Not Found: {self.command} {target_url}\n".encode("utf-8")
            self.send_response(404, "Not Found")
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Connection", "close")
            self.end_headers()
            if send_body:
                self.wfile.write(body)
            return

        resp = entry.get("response", {})
        status = int(resp.get("status", 200))
        status_text = resp.get("statusText", "OK") or "OK"

        # Extract body
        content = resp.get("content", {})
        encoding = content.get("encoding", "").lower()
        raw_text = content.get("text", "")

        body_bytes = b""
        if encoding == "base64" and raw_text:
            try:
                body_bytes = base64.b64decode(raw_text)
            except Exception:
                body_bytes = raw_text.encode("utf-8", errors="replace")
        elif raw_text:
            body_bytes = raw_text.encode("utf-8", errors="replace")

        # Send response status
        self.send_response(status, status_text)

        # Forward headers from HAR entry, filtering out hop-by-hop headers
        has_content_type = False
        for hdr in resp.get("headers", []):
            name = hdr.get("name", "").strip()
            value = hdr.get("value", "").strip()
            if not name:
                continue
            lower_name = name.lower()
            if lower_name in HOP_BY_HOP_HEADERS:
                continue
            if lower_name == "content-type":
                has_content_type = True
            # Some HAR records keep content-encoding: gzip even though text was uncompressed
            if lower_name == "content-encoding" and encoding != "base64":
                continue
            self.send_header(name, value)

        if not has_content_type:
            mime = content.get("mimeType") or "application/octet-stream"
            self.send_header("Content-Type", mime)

        # Set accurate content-length
        self.send_header("Content-Length", str(len(body_bytes)))
        self.send_header("Connection", "close")
        self.end_headers()

        if send_body and body_bytes:
            try:
                self.wfile.write(body_bytes)
            except (BrokenPipeError, ConnectionResetError):
                pass


class HarServer(ThreadingHTTPServer):
    """Multi-threaded HTTP server for HAR replaying."""

    def __init__(self, server_address: Tuple[str, int], archive: HarArchive):
        self.archive = archive
        super().__init__(server_address, HarRequestHandler)

    def get_url(self) -> str:
        host, port = self.server_address
        return f"http://{host}:{port}"


def start_har_server_in_thread(
    har_path: str | Path, host: str = "127.0.0.1", port: int = 0
) -> Tuple[HarServer, threading.Thread, str]:
    """Start HAR server in a daemon thread. Returns (server, thread, server_url)."""
    archive = HarArchive.from_file(har_path)
    server = HarServer((host, port), archive)
    server_url = server.get_url()
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread, server_url


def main():
    parser = argparse.ArgumentParser(description="Deterministic HAR replay server")
    parser.add_argument("har_file", help="Path to .har archive file")
    parser.add_argument("--host", default="127.0.0.1", help="Bind host (default: 127.0.0.1)")
    parser.add_argument("--port", type=int, default=0, help="Bind port (0 for auto)")
    args = parser.parse_args()

    if not os.path.exists(args.har_file):
        sys.stderr.write(f"Error: HAR file not found: {args.har_file}\n")
        sys.exit(1)

    try:
        archive = HarArchive.from_file(args.har_file)
        server = HarServer((args.host, args.port), archive)
        url = server.get_url()
        # Readiness sentinel for child processes / callers
        print(f"HAR_SERVER_READY {url}", flush=True)

        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass
        finally:
            server.server_close()
    except Exception as e:
        sys.stderr.write(f"Error starting HAR server: {e}\n")
        sys.exit(1)


if __name__ == "__main__":
    main()
