"""Build the bundled UI glyph atlas from Noto Sans CJK SC Regular (OFL-1.1)."""

import struct
import sys
from pathlib import Path

from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFont

font_path = Path(sys.argv[1])
root = Path(__file__).resolve().parents[1] / "src" / "gui"
size = 14
font = ImageFont.truetype(str(font_path), size)
with TTFont(font_path) as source:
    supported = set(source.getBestCmap())

# Cover common Simplified Chinese search text as well as the UI and title metadata.
chars = set()
for lead in range(0xA1, 0xFF):
    for trail in range(0xA1, 0xFF):
        chars.update(bytes([lead, trail]).decode("gb2312", errors="ignore"))
for name in ("i18n.rs", "titles.tsv"):
    chars.update((root / name).read_text(encoding="utf-8"))
chars.update("中文")
with (root / "kanji.glyphs").open("wb") as stream:
    for ch in sorted(chars):
        if ch.isascii() or ord(ch) not in supported:
            continue
        image = Image.new("L", (size, size))
        ImageDraw.Draw(image).text((size // 2, size // 2), ch, 255, font=font, anchor="mm")
        mask = [
            sum(1 << x for x in range(size) if image.getpixel((x, y)) >= 88)
            for y in range(size)
        ]
        stream.write(struct.pack("<I14H", ord(ch), *mask))
