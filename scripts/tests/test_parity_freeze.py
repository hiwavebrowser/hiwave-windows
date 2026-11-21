"""parity-freeze.js: rAF-driven drawing reaches its final frame in Chrome.

The oracle's init script (baselines/common/parity-freeze.js) froze Date.now
and called requestAnimationFrame callbacks synchronously, so a rAF loop timed
by the clock (Chart.js's animator) never finished and a frame-counted loop
recursed until the stack overflowed: the report page's charts stayed blank
in every Chrome capture (found by PR #1361). These tests run the board's own
Chromium arm (tools/parity_oracle/chrome_board.mjs, deterministic context)
on scripts/tests/fixtures/raf_canvas_loop.html and read the frame.

They need the pinned Chrome for Testing (PARITY_CHROME_PATH) and
tools/parity_oracle/node_modules; without them they skip.
"""
from __future__ import annotations

import json
import os
import struct
import sys
import tempfile
import unittest
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import chrome_board  # noqa: E402

FIXTURE = Path(__file__).resolve().parent / "fixtures" / "raf_canvas_loop.html"
ENDLESS = Path(__file__).resolve().parent / "fixtures" / "raf_endless_loop.html"
MAX_FRAMES = 300
FROZEN_TIME = 1704067200000
FRAME_MS = 1000 / 60


def read_png_rgba(path: Path):
    """(width, height, rows of RGBA bytes) for an 8-bit, non-interlaced RGB(A) PNG."""
    data = path.read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos, idat, width = 8, b"", None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos:pos + 4])
        kind = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
            assert depth == 8 and color in (2, 6) and interlace == 0, "unsupported PNG"
            bpp = 4 if color == 6 else 3
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    raw = zlib.decompress(idat)
    stride = width * bpp
    rows, prev, i = [], bytearray(stride), 0
    for _ in range(height):
        ftype, line = raw[i], bytearray(raw[i + 1:i + 1 + stride])
        i += 1 + stride
        for x in range(stride):
            a = line[x - bpp] if x >= bpp else 0
            b = prev[x]
            c = prev[x - bpp] if x >= bpp else 0
            if ftype == 1:
                line[x] = (line[x] + a) & 0xFF
            elif ftype == 2:
                line[x] = (line[x] + b) & 0xFF
            elif ftype == 3:
                line[x] = (line[x] + (a + b) // 2) & 0xFF
            elif ftype == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[x] = (line[x] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 0xFF
        rows.append(bytes(line) if bpp == 4 else bytes(
            v for px in range(width) for v in (*line[px * 3:px * 3 + 3], 255)))
        prev = line
    return width, height, rows


def pixel(rows, x: int, y: int):
    return tuple(rows[y][x * 4:x * 4 + 4])


def pinned_chrome() -> bool:
    p = os.environ.get("PARITY_CHROME_PATH")
    return bool(p) and Path(p).exists()


@unittest.skipUnless(pinned_chrome(), "PARITY_CHROME_PATH (pinned Chrome for Testing) not set")
@unittest.skipUnless((chrome_board.ORACLE_DIR / "node_modules").is_dir(), "no tools/parity_oracle/node_modules")
class RafDrivenDrawingReachesItsLastFrame(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        out = Path(cls.tmp.name)
        cmd = ["node", str(chrome_board.CHROMIUM_ARM), str(FIXTURE), str(out), "480", "160"]
        cls.record = chrome_board._run(cmd, timeout=120)
        cls.png = out / "chrome.png"
        cls.log = (cls.record.get("ipc_calls") or [{}])[0]

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_the_capture_ran_on_the_pin_without_page_errors(self):
        self.assertEqual(self.record.get("status"), "ok", json.dumps(self.record)[:400])
        self.assertEqual(self.record.get("browser"), chrome_board.PINNED_CHROME_VERSION)
        self.assertEqual(self.record.get("page_errors"), [])

    def test_a_frame_counted_loop_shows_its_last_frame(self):
        _, _, rows = read_png_rgba(self.png)
        self.assertEqual(pixel(rows, 120, 70), (0, 255, 0, 255))
        self.assertEqual(pixel(rows, 215, 115), (0, 255, 0, 255))  # the bar's far end too
        self.assertEqual(self.log.get("framesByFrames"), 60)

    def test_a_clock_timed_loop_shows_its_last_frame(self):
        # Chart.js's animator: progress = (Date.now() - start) / duration.
        _, _, rows = read_png_rgba(self.png)
        self.assertEqual(pixel(rows, 340, 70), (0, 255, 0, 255))
        self.assertEqual(self.log.get("framesByClock"), 60)
        self.assertEqual(self.log.get("dateAtEnd"), FROZEN_TIME + 1000)

    def test_the_clock_is_frozen_until_a_frame_runs_then_steps_one_frame_at_a_time(self):
        self.assertEqual(self.log.get("dateAtLoad"), FROZEN_TIME)
        self.assertEqual(self.log.get("dateAgain"), FROZEN_TIME)
        ts = self.log.get("timestamps") or []
        self.assertEqual(len(ts), 60)
        self.assertAlmostEqual(ts[0], FRAME_MS, places=6)
        for prev, cur in zip(ts, ts[1:]):
            self.assertAlmostEqual(cur - prev, FRAME_MS, places=6)

    def test_the_frame_timestamp_is_on_the_performance_now_scale(self):
        # Virtual frame time since the time origin, like performance.now();
        # on the epoch scale, ts - performance.now() was about 1.7e12.
        self.assertAlmostEqual(self.log.get("firstFrameTs"), FRAME_MS, places=6)
        self.assertLess(abs(self.log["firstFrameTs"] - self.log["perfAtFirstFrame"]), 60_000)

    def test_a_callback_cancelled_by_an_earlier_one_in_the_same_frame_never_runs(self):
        self.assertIs(self.log.get("siblingCancelledRan"), False)

    def test_frame_callbacks_run_before_the_resize_observation_of_the_same_update(self):
        # Chart.js's line chart is never drawn if its animator's frames run
        # ahead of the resize observation (#1361); frames must be Chrome's own.
        self.assertEqual(self.log.get("order"), ["A1", "ro"] + [f"A{i}" for i in range(2, 11)])

    def test_the_capture_waited_for_the_frames_to_finish(self):
        frames = self.record.get("animation_frames", {})
        self.assertEqual(frames.get("settled"), True)
        self.assertIs(frames.get("capped"), False)

    def test_a_cancelled_callback_never_runs(self):
        self.assertIs(self.log.get("cancelledRan"), False)


@unittest.skipUnless(pinned_chrome(), "PARITY_CHROME_PATH (pinned Chrome for Testing) not set")
@unittest.skipUnless((chrome_board.ORACLE_DIR / "node_modules").is_dir(), "no tools/parity_oracle/node_modules")
class AnEndlessLoopStopsAtTheBudget(unittest.TestCase):
    """A spinner page: the frame budget, not the wall clock, decides the frame
    captured, and the capture says the budget was hit."""

    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        out = Path(cls.tmp.name)
        cmd = ["node", str(chrome_board.CHROMIUM_ARM), str(ENDLESS), str(out), "240", "140"]
        cls.record = chrome_board._run(cmd, timeout=120)
        cls.png = out / "chrome.png"
        cls.log = (cls.record.get("ipc_calls") or [{}])[0]

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_the_capture_reports_the_budget_was_hit(self):
        self.assertEqual(self.record.get("status"), "ok", json.dumps(self.record)[:400])
        self.assertEqual(self.record.get("animation_frames"),
                         {"frames": MAX_FRAMES, "settled": True, "capped": True})

    def test_the_loop_ran_exactly_the_budget_and_the_frame_shows_it(self):
        self.assertEqual(self.log.get("frames"), MAX_FRAMES)
        _, _, rows = read_png_rgba(self.png)
        self.assertEqual(pixel(rows, 120, 70), (0, 255, 0, 255))


if __name__ == "__main__":
    unittest.main()
