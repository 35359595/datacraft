"""Generate every DataCraft application icon from one geometric mark (requires Pillow)."""
import argparse
import io
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
BLUE = (53, 116, 240, 255)
SIZES = (16, 20, 24, 32, 40, 48, 64, 128, 256)
CENTER, TOP, BOTTOM, RX, RY, STROKE = 16, 9, 23, 8, 3, 2


def draw(size: int) -> Image.Image:
    scale = size * 4 / 32
    image = Image.new("RGBA", (size * 4, size * 4))
    canvas = ImageDraw.Draw(image)
    box = lambda values: tuple(round(v * scale) for v in values)
    canvas.rounded_rectangle(box((2, 2, 30, 30)), radius=round(7 * scale), fill=BLUE)
    left, right = CENTER - RX, CENTER + RX
    width = round(STROKE * scale)
    canvas.line(box((left, TOP, left, BOTTOM)), fill="white", width=width)
    canvas.line(box((right, TOP, right, BOTTOM)), fill="white", width=width)
    half = STROKE / 2
    for y in ((TOP + BOTTOM) // 2, BOTTOM):
        canvas.arc(box((left - half, y - RY - half, right + half, y + RY + half)), 0, 180, fill="white", width=width)
    canvas.ellipse(box((left - half, TOP - RY - half, right + half, TOP + RY + half)), outline="white", width=width)
    return image.resize((size, size), Image.Resampling.LANCZOS)


def generate() -> dict[str, bytes]:
    output = {}
    for name, size, fmt, options in [
        ("icon.png", 512, "PNG", {}),
        ("icon-32.png", 32, "PNG", {}),
        ("icon-64.png", 64, "PNG", {}),
        ("icon.ico", 256, "ICO", {"sizes": [(s, s) for s in SIZES], "append_images": [draw(s) for s in SIZES[:-1]]}),
        ("DataCraft.icns", 1024, "ICNS", {}),
    ]:
        buffer = io.BytesIO()
        draw(size).save(buffer, format=fmt, **options)
        output[name] = buffer.getvalue()
    color = "#" + "".join(f"{channel:02x}" for channel in BLUE[:3])
    output["icon.svg"] = f'''<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32">
  <rect x="2" y="2" width="28" height="28" rx="7" fill="{color}"/>
  <g fill="none" stroke="#fff" stroke-width="{STROKE}" stroke-linecap="round" stroke-linejoin="round">
    <ellipse cx="{CENTER}" cy="{TOP}" rx="{RX}" ry="{RY}"/>
    <path d="M{CENTER-RX} {TOP}V{BOTTOM}a{RX} {RY} 0 0 0 {RX*2} 0V{TOP} M{CENTER-RX} {(TOP+BOTTOM)//2}a{RX} {RY} 0 0 0 {RX*2} 0"/>
  </g>
</svg>
'''.encode()
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="check that committed assets match the generator")
    args = parser.parse_args()
    for name, content in generate().items():
        path = ROOT / "assets" / name
        if args.check:
            if not path.is_file() or path.read_bytes() != content:
                parser.error(f"{path} is out of date; run python scripts/make_icon.py")
        else:
            path.write_bytes(content)
    print("Application icons are up to date." if args.check else "Generated PNG, ICO, ICNS and SVG application icons.")


if __name__ == "__main__":
    main()
