"""The image readers must refuse what they cannot decode, not approximate it.

Run: python3 scripts/tests/test_parity_image.py

Gate B compares two frames pixel for pixel. Every way a reader could return
something plausible-but-wrong is a way the gate reports a number that means
nothing, so the tests below are organised around those ways: a format it does
not support, a truncated file, a header it half-understands.

The decoder itself is checked against the 32 committed Chrome baselines rather
than a synthetic PNG. A hand-rolled fixture exercises the filter types the
fixture author thought of; the real captures exercise the ones Chrome emits.
"""
import os
import struct
import sys
import tempfile
import zlib
from pathlib import Path

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from parity_image import (  # noqa: E402
    Image,
    UnsupportedImage,
    read_image,
    read_png,
    read_ppm,
)

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
BASELINES = REPO_ROOT / "baselines" / "chrome-148"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def make_png(width, height, nch, rows, depth=8, interlace=0, color=None):
    """Assemble a PNG from raw (already-filtered) scanlines."""
    color_by_nch = {1: 0, 2: 4, 3: 2, 4: 6}
    ctype = color_by_nch[nch] if color is None else color
    ihdr = struct.pack(">IIBBBBB", width, height, depth, ctype, 0, 0, interlace)
    raw = b"".join(rows)

    def chunk(tag, body):
        return (
            struct.pack(">I", len(body))
            + tag
            + body
            + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw))
        + chunk(b"IEND", b"")
    )


def write(tmp, name, data):
    path = Path(tmp) / name
    path.write_bytes(data)
    return path


def ppm(width, height, rgb, header=b"P6"):
    return header + b"\n%d %d\n255\n" % (width, height) + rgb


# ---------------------------------------------------------------------------
# PNG, against the real corpus
# ---------------------------------------------------------------------------


def test_every_committed_baseline_decodes_to_its_declared_viewport():
    """Dimensions must agree with the rects captured alongside them.

    If they ever disagree, every geometry AND paint comparison for that case is
    being made against a frame that is not the picture the rects describe.
    """
    import json

    pngs = sorted(BASELINES.glob("*/*/baseline.png"))
    assert len(pngs) == 32, f"expected 32 baselines, found {len(pngs)}"
    for path in pngs:
        image = read_png(path)
        rects = json.loads((path.parent / "layout-rects.json").read_text(encoding="utf-8"))
        viewport = rects["viewport"]
        assert image.size == (viewport["width"], viewport["height"]), (
            f"{path}: png {image.size} vs rects "
            f"{(viewport['width'], viewport['height'])}"
        )
        assert len(image.rgb) == image.width * image.height * 3


def test_each_filter_type_reconstructs_the_same_picture():
    """None/Sub/Up/Average/Paeth must all rebuild identical pixels.

    A filter implemented wrongly does not fail loudly — it shifts colours by a
    few units, which is exactly the magnitude Gate B's tolerance is meant to
    absorb, so the error would hide inside the thing it corrupts.
    """
    width, height = 4, 5
    want = bytes(range(width * height * 3))
    rows = [want[y * width * 3 : (y + 1) * width * 3] for y in range(height)]

    def refilter(ftype):
        out = []
        prev = bytes(width * 3)
        for row in rows:
            if ftype == 0:
                line = row
            elif ftype == 1:
                line = bytes(
                    (row[i] - (row[i - 3] if i >= 3 else 0)) & 0xFF
                    for i in range(len(row))
                )
            elif ftype == 2:
                line = bytes((a - b) & 0xFF for a, b in zip(row, prev))
            elif ftype == 3:
                line = bytes(
                    (row[i] - (((row[i - 3] if i >= 3 else 0) + prev[i]) >> 1)) & 0xFF
                    for i in range(len(row))
                )
            else:
                line = []
                for i in range(len(row)):
                    a = row[i - 3] if i >= 3 else 0
                    b = prev[i]
                    c = prev[i - 3] if i >= 3 else 0
                    p = a + b - c
                    pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                    pred = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                    line.append((row[i] - pred) & 0xFF)
                line = bytes(line)
            out.append(bytes([ftype]) + line)
            prev = row
        return out

    with tempfile.TemporaryDirectory() as tmp:
        for ftype in range(5):
            path = write(
                tmp, f"f{ftype}.png", make_png(width, height, 3, refilter(ftype))
            )
            assert read_png(path).rgb == want, f"filter {ftype} reconstructed wrongly"


def test_alpha_is_dropped_and_recorded_never_composited():
    with tempfile.TemporaryDirectory() as tmp:
        rows = [b"\x00" + bytes([10, 20, 30, 0, 40, 50, 60, 255])]
        path = write(tmp, "a.png", make_png(2, 1, 4, rows))
        image = read_png(path)
        assert image.rgb == bytes([10, 20, 30, 40, 50, 60])
        assert image.had_alpha is True


def test_a_16_bit_png_is_refused_for_being_16_bit():
    """The refusal must come from the depth check, not from a size accident.

    The weaker version of this test only asserted that SOMETHING was raised —
    and the mutation sweep showed it stayed green with the depth check deleted,
    because a 16-bit scanline happens to be the wrong length for the 8-bit path
    and the size check caught it instead. A 16-bit image whose byte count did
    line up would have decoded as garbage. Asserting the reason is what makes
    the depth check load-bearing.
    """
    with tempfile.TemporaryDirectory() as tmp:
        rows = [b"\x00" + bytes(12)]
        path = write(tmp, "d.png", make_png(2, 1, 3, rows, depth=16))
        try:
            read_png(path)
        except UnsupportedImage as exc:
            assert "bit depth" in str(exc), f"refused for the wrong reason: {exc}"
            return
        raise AssertionError("a 16-bit PNG was decoded as if it were 8-bit")


def test_an_interlaced_png_is_refused_rather_than_read_as_progressive():
    with tempfile.TemporaryDirectory() as tmp:
        rows = [b"\x00" + bytes(6)]
        path = write(tmp, "i.png", make_png(2, 1, 3, rows, interlace=1))
        try:
            read_png(path)
        except UnsupportedImage:
            return
        raise AssertionError("an interlaced PNG was decoded as progressive")


def test_a_palette_png_is_refused_rather_than_read_as_indices():
    with tempfile.TemporaryDirectory() as tmp:
        rows = [b"\x00" + bytes(2)]
        path = write(tmp, "p.png", make_png(2, 1, 1, rows, color=3))
        try:
            read_png(path)
        except UnsupportedImage:
            return
        raise AssertionError("a palette PNG was read as grayscale indices")


def test_an_unknown_filter_type_is_refused():
    with tempfile.TemporaryDirectory() as tmp:
        path = write(tmp, "u.png", make_png(2, 1, 3, [b"\x09" + bytes(6)]))
        try:
            read_png(path)
        except UnsupportedImage:
            return
        raise AssertionError("an unknown filter byte was silently accepted")


def test_a_short_idat_is_refused_rather_than_padded():
    """One scanline short must not decode as a black final row."""
    with tempfile.TemporaryDirectory() as tmp:
        path = write(tmp, "s.png", make_png(2, 2, 3, [b"\x00" + bytes(6)]))
        try:
            read_png(path)
        except UnsupportedImage:
            return
        raise AssertionError("a truncated PNG was padded instead of refused")


def test_a_non_png_is_refused_by_signature():
    with tempfile.TemporaryDirectory() as tmp:
        path = write(tmp, "n.png", b"not a png at all")
        try:
            read_png(path)
        except UnsupportedImage:
            return
        raise AssertionError("a non-PNG passed the signature check")


# ---------------------------------------------------------------------------
# PPM — the RustKit side
# ---------------------------------------------------------------------------


def test_a_binary_ppm_round_trips():
    rgb = bytes([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12])
    with tempfile.TemporaryDirectory() as tmp:
        image = read_ppm(write(tmp, "f.ppm", ppm(2, 2, rgb)))
    assert image.size == (2, 2)
    assert image.rgb == rgb
    assert image.pixel(1, 1) == (10, 11, 12)


def test_an_ascii_ppm_round_trips():
    with tempfile.TemporaryDirectory() as tmp:
        body = b"P3\n2 1\n255\n1 2 3 4 5 6\n"
        image = read_ppm(write(tmp, "a.ppm", body))
    assert image.rgb == bytes([1, 2, 3, 4, 5, 6])


def test_comments_in_the_ppm_header_are_skipped():
    with tempfile.TemporaryDirectory() as tmp:
        body = b"P6\n# written by parity-capture\n2 1\n255\n" + bytes(6)
        assert read_ppm(write(tmp, "c.ppm", body)).size == (2, 1)


def test_a_truncated_ppm_is_refused_rather_than_padded():
    """A frame that stopped mid-write must not score as a mostly-black render."""
    with tempfile.TemporaryDirectory() as tmp:
        path = write(tmp, "t.ppm", ppm(4, 4, bytes(10)))
        try:
            read_ppm(path)
        except UnsupportedImage:
            return
        raise AssertionError("a short PPM was padded instead of refused")


def test_a_non_255_maxval_is_refused_rather_than_rescaled():
    with tempfile.TemporaryDirectory() as tmp:
        path = write(tmp, "m.ppm", b"P6\n2 1\n65535\n" + bytes(6))
        try:
            read_ppm(path)
        except UnsupportedImage:
            return
        raise AssertionError("a non-255 maxval was read as if it were 8-bit")


def test_a_png_handed_to_the_ppm_reader_is_refused():
    """Discovery globs for frame.ppm; a stray file must not decode as garbage."""
    with tempfile.TemporaryDirectory() as tmp:
        path = write(tmp, "x.ppm", make_png(2, 1, 3, [b"\x00" + bytes(6)]))
        try:
            read_ppm(path)
        except UnsupportedImage:
            return
        raise AssertionError("a PNG was accepted by the PPM reader")


def test_read_image_dispatches_on_extension_and_refuses_the_unknown():
    with tempfile.TemporaryDirectory() as tmp:
        assert read_image(write(tmp, "f.ppm", ppm(1, 1, bytes(3)))).size == (1, 1)
        try:
            read_image(write(tmp, "f.bmp", bytes(64)))
        except UnsupportedImage:
            return
        raise AssertionError("an unknown extension was decoded anyway")


def test_a_pixel_buffer_that_does_not_match_the_declared_size_is_refused():
    try:
        Image(4, 4, bytes(3))
    except UnsupportedImage:
        return
    raise AssertionError("Image accepted a buffer of the wrong length")


if __name__ == "__main__":
    assert BASELINES.exists(), f"no baselines at {BASELINES}"
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            fn()
            print(f"ok  {name}")
    print("PASS: the image readers refuse what they cannot decode")
