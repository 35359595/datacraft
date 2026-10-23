"""Export every DataCraft application icon from the imagegen master (requires Pillow)."""
import argparse
import io
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
SIZES = (16, 20, 24, 32, 40, 48, 64, 128, 256)
MASTER = ROOT / "assets" / "icon-master.png"


def generate() -> dict[str, bytes]:
    with Image.open(MASTER) as source:
        if source.width != source.height or source.width < 1024:
            raise ValueError("The application icon master must be square and at least 1024 px.")
        master = source.convert("RGBA")
    def draw(size):
        return master.resize((size, size), Image.Resampling.LANCZOS)
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
    print("Application icons are up to date." if args.check else "Exported PNG, ICO and ICNS application icons from icon-master.png.")


if __name__ == "__main__":
    main()
