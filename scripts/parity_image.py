#!/usr/bin/env python3
"""parity_image.py — stdlib-only RGB reader for the two capture formats.

Gate B (paint) has to hold one Chrome frame and one RustKit frame in memory at
the same time and compare them per channel. The two sides do not share a
format:

    Chrome   baselines/chrome-148/<scope>/<case>/baseline.png   PNG, 8-bit
    RustKit  <run>/<case>/<viewport>/iter-<n>/capture/frame.ppm  PPM (P6/P3)

Nothing else in scripts/ decodes an image — the existing pixel diff is done in
Rust by parity-capture — so there is no Pillow or numpy anywhere in the repo's
Python tooling, and this module does not add one. A gate that needs a pip
install to run is a gate that gets skipped, and a skipped gate reports the same
"nothing wrong" as a green one.

Both readers are deliberately strict. An image this module cannot decode raises
`UnsupportedImage` rather than returning something plausible: Gate B turns that
into UNMEASURED, which fails. Guessing at a malformed frame is how a broken
capture pipeline scores green.
"""

import struct
import zlib
from pathlib import Path
from typing import Tuple


class UnsupportedImage(Exception):
    """The file is not an image this module will decode.

    Raised rather than handled with a fallback. A caller that catches this and
    treats the case as unmeasured is correct; a caller that catches it and
    skips the case is reintroducing the bug this whole campaign is about.
    """


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"

# Bytes per pixel by PNG color type. Types 3 (palette) and 16-bit depths are
# not in the corpus and are refused rather than half-supported.
PNG_CHANNELS = {0: 1, 2: 3, 4: 2, 6: 4}


class Image:
    """An 8-bit RGB image. Alpha is dropped, not composited.

    Neither capture path emits transparency — Chrome captures on an opaque
    page and RustKit's PPM has no alpha channel at all — so dropping it cannot
    lose information here. If a future capture does carry alpha, the drop would
    silently compare premultiplied garbage, which is why `had_alpha` is kept.
    """

    __slots__ = ("width", "height", "rgb", "had_alpha")

    def __init__(self, width: int, height: int, rgb: bytes, had_alpha: bool = False):
        if len(rgb) != width * height * 3:
            raise UnsupportedImage(
                f"pixel buffer is {len(rgb)} bytes, expected {width * height * 3}"
            )
        self.width = width
        self.height = height
        self.rgb = rgb
        self.had_alpha = had_alpha

    @property
    def size(self) -> Tuple[int, int]:
        return (self.width, self.height)

    def pixel(self, x: int, y: int) -> Tuple[int, int, int]:
        i = (y * self.width + x) * 3
        return (self.rgb[i], self.rgb[i + 1], self.rgb[i + 2])


# ---------------------------------------------------------------------------
# PNG
# ---------------------------------------------------------------------------


def _unfilter(raw: bytes, width: int, height: int, nch: int) -> bytearray:
    """Reverse the per-scanline PNG filters.

    Written for clarity over cleverness, with the two cheap filter types
    (None/Up) taking fast paths because Chrome's captures are dominated by
    them. The per-byte loops for Sub/Average/Paeth are slow in Python but run
    once per gate invocation, not per comparison.
    """
    stride = width * nch
    out = bytearray(stride * height)
    prev = bytes(stride)
    pos = 0
    for y in range(height):
        ftype = raw[pos]
        pos += 1
        line = bytearray(raw[pos : pos + stride])
        pos += stride
        if len(line) != stride:
            raise UnsupportedImage("truncated PNG scanline")

        if ftype == 0:
            pass
        elif ftype == 1:
            for x in range(nch, stride):
                line[x] = (line[x] + line[x - nch]) & 0xFF
        elif ftype == 2:
            line = bytearray((a + b) & 0xFF for a, b in zip(line, prev))
        elif ftype == 3:
            for x in range(stride):
                left = line[x - nch] if x >= nch else 0
                line[x] = (line[x] + ((left + prev[x]) >> 1)) & 0xFF
        elif ftype == 4:
            for x in range(stride):
                a = line[x - nch] if x >= nch else 0
                b = prev[x]
                c = prev[x - nch] if x >= nch else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                if pa <= pb and pa <= pc:
                    pred = a
                elif pb <= pc:
                    pred = b
                else:
                    pred = c
                line[x] = (line[x] + pred) & 0xFF
        else:
            raise UnsupportedImage(f"unknown PNG filter type {ftype}")

        out[y * stride : (y + 1) * stride] = line
        prev = bytes(line)
    return out


def _to_rgb(pixels: bytearray, nch: int) -> bytes:
    if nch == 3:
        return bytes(pixels)
    if nch == 4:
        del pixels[3::4]
        return bytes(pixels)
    if nch == 1:
        out = bytearray(len(pixels) * 3)
        out[0::3] = pixels
        out[1::3] = pixels
        out[2::3] = pixels
        return bytes(out)
    if nch == 2:  # gray + alpha
        gray = pixels[0::2]
        out = bytearray(len(gray) * 3)
        out[0::3] = gray
        out[1::3] = gray
        out[2::3] = gray
        return bytes(out)
    raise UnsupportedImage(f"cannot convert {nch}-channel data to RGB")


def read_png(path: Path) -> Image:
    data = path.read_bytes()
    if data[:8] != PNG_SIGNATURE:
        raise UnsupportedImage(f"{path} is not a PNG")

    width = height = None
    nch = 0
    idat = bytearray()
    offset = 8
    while offset + 8 <= len(data):
        length = struct.unpack(">I", data[offset : offset + 4])[0]
        ctype = data[offset + 4 : offset + 8]
        body = data[offset + 8 : offset + 8 + length]
        if ctype == b"IHDR":
            width, height, depth, color, comp, filt, interlace = struct.unpack(
                ">IIBBBBB", body
            )
            if depth != 8:
                raise UnsupportedImage(f"{path}: bit depth {depth}, only 8 supported")
            if interlace != 0:
                raise UnsupportedImage(f"{path}: interlaced PNG")
            if color not in PNG_CHANNELS:
                raise UnsupportedImage(f"{path}: color type {color}")
            nch = PNG_CHANNELS[color]
        elif ctype == b"IDAT":
            idat += body
        elif ctype == b"IEND":
            break
        offset += 12 + length

    if width is None:
        raise UnsupportedImage(f"{path}: no IHDR")
    if not idat:
        raise UnsupportedImage(f"{path}: no IDAT")

    try:
        raw = zlib.decompress(bytes(idat))
    except zlib.error as exc:
        raise UnsupportedImage(f"{path}: corrupt IDAT ({exc})") from exc

    if len(raw) != (width * nch + 1) * height:
        raise UnsupportedImage(f"{path}: decompressed size does not match IHDR")

    pixels = _unfilter(raw, width, height, nch)
    return Image(width, height, _to_rgb(pixels, nch), had_alpha=nch in (2, 4))


# ---------------------------------------------------------------------------
# PPM
# ---------------------------------------------------------------------------


def _ppm_tokens(data: bytes, start: int, count: int):
    """Read `count` whitespace-separated header tokens, skipping # comments."""
    tokens = []
    i = start
    while len(tokens) < count:
        if i >= len(data):
            raise UnsupportedImage("truncated PPM header")
        ch = data[i : i + 1]
        if ch == b"#":
            while i < len(data) and data[i : i + 1] not in (b"\n", b"\r"):
                i += 1
        elif ch.isspace():
            i += 1
        else:
            j = i
            while j < len(data) and not data[j : j + 1].isspace() and data[j : j + 1] != b"#":
                j += 1
            tokens.append(data[i:j])
            i = j
    return tokens, i


def read_ppm(path: Path) -> Image:
    data = path.read_bytes()
    magic = data[:2]
    if magic not in (b"P6", b"P3"):
        raise UnsupportedImage(f"{path}: not a P6/P3 PPM")

    (w_tok, h_tok, max_tok), pos = _ppm_tokens(data, 2, 3)
    width, height, maxval = int(w_tok), int(h_tok), int(max_tok)
    if maxval != 255:
        raise UnsupportedImage(f"{path}: maxval {maxval}, only 255 supported")
    if width <= 0 or height <= 0:
        raise UnsupportedImage(f"{path}: {width}x{height}")

    if magic == b"P6":
        # Exactly one whitespace byte separates the header from binary data.
        body = data[pos + 1 :]
        need = width * height * 3
        if len(body) < need:
            raise UnsupportedImage(
                f"{path}: {len(body)} bytes of pixel data, expected {need}"
            )
        return Image(width, height, bytes(body[:need]))

    values = data[pos:].split()
    need = width * height * 3
    if len(values) < need:
        raise UnsupportedImage(
            f"{path}: {len(values)} samples, expected {need}"
        )
    return Image(width, height, bytes(int(v) for v in values[:need]))


def read_image(path: Path) -> Image:
    """Dispatch on extension, then verify by magic inside the reader."""
    suffix = path.suffix.lower()
    if suffix == ".png":
        return read_png(path)
    if suffix in (".ppm", ".pnm"):
        return read_ppm(path)
    raise UnsupportedImage(f"{path}: unknown image extension {suffix!r}")


# ---------------------------------------------------------------------------
# PNG writing
# ---------------------------------------------------------------------------


def write_png(path: Path, image: Image) -> None:
    """Write an 8-bit RGB PNG. Stdlib only, same reason as the readers.

    Gate C publishes a heatmap per case, and a forensic board whose images
    cannot be opened is a board nobody reads. Filter type 0 (None) on every
    row: heatmaps are large flat regions of the background colour, which zlib
    already collapses, and a real filter would buy compression at the cost of
    another thing to get wrong in a file that exists to be looked at.
    """
    raw = bytearray()
    stride = image.width * 3
    for y in range(image.height):
        raw.append(0)  # filter: None
        start = y * stride
        raw += image.rgb[start : start + stride]

    def chunk(tag: bytes, payload: bytes) -> bytes:
        return (
            struct.pack(">I", len(payload))
            + tag
            + payload
            + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", image.width, image.height, 8, 2, 0, 0, 0)
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "wb") as handle:
        handle.write(PNG_SIGNATURE)
        handle.write(chunk(b"IHDR", ihdr))
        handle.write(chunk(b"IDAT", zlib.compress(bytes(raw), 6)))
        handle.write(chunk(b"IEND", b""))
