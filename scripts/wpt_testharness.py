"""Process isolation and result decoding for a WPT page."""
import json
import subprocess
import argparse
import functools
import threading
from pathlib import Path
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import quote, unquote


def run_page(command, page, timeout):
    result = {"test": page, "status": "ERROR", "subtests": []}
    try:
        process = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                 encoding="utf-8", errors="replace", timeout=timeout)
    except subprocess.TimeoutExpired:
        result["status"] = "TIMEOUT"
        return result
    if process.returncode == 3:
        return {**result, "status": "TIMEOUT"}
    if process.returncode == 4 or process.returncode < 0 or process.returncode >= 0x80000000:
        return {**result, "status": "CRASH"}
    if process.returncode:
        return result
    for line in process.stdout.splitlines():
        if line.startswith("WPT_ENCODED "):
            line = "WPT_RESULT " + unquote(line.removeprefix("WPT_ENCODED "))
        if line.startswith("WPT_RESULT "):
            try:
                record = json.loads(line.removeprefix("WPT_RESULT "))
            except ValueError:
                continue
            if isinstance(record, dict) and isinstance(record.get("subtests"), list):
                return {**record, "test": page}
    return result


class Handler(SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.path.partition("?")[0] == "/resources/testharness.js":
            report = Path(__file__).with_name("wpt_report.js").read_bytes()
            harness = Path(self.directory, "resources", "testharness.js")
            if not harness.is_file():
                self.send_error(404)
                return
            report = harness.read_bytes() + b"\n;" + report
            self.send_response(200)
            self.send_header("Content-Type", "text/javascript")
            self.send_header("Content-Length", str(len(report)))
            self.end_headers()
            self.wfile.write(report)
            return
        super().do_GET()


def main(reference_command=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", required=True, type=Path)
    parser.add_argument("--host", required=reference_command is None, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--timeout", type=int, default=30)
    parser.add_argument("pages", nargs="+")
    args = parser.parse_args()
    root = args.root.resolve(strict=True)
    host = args.host.resolve(strict=True) if args.host else None
    if args.timeout <= 0:
        parser.error("timeout must be positive")
    for page in args.pages:
        path = (root / page).resolve()
        if not path.is_relative_to(root) or not path.is_file():
            parser.error("page must be a file in the WPT root")
    handler = functools.partial(Handler, directory=str(root))
    with ThreadingHTTPServer(("127.0.0.1", 0), handler) as server:
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            with args.output.open("w", encoding="utf-8", newline="\n") as output:
                for page in args.pages:
                    url = f"http://127.0.0.1:{server.server_port}/{quote(page)}"
                    command = (reference_command(url, args.timeout) if reference_command else
                               [host, "--wpt", "--url", url, "--timeout-ms", str(args.timeout * 1000)])
                    row = run_page(command, page, args.timeout + 2)
                    print(json.dumps(row, ensure_ascii=False), file=output, flush=True)
        finally:
            server.shutdown()


if __name__ == "__main__":
    main()
