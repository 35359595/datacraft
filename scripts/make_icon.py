"""Generates assets/DataCraft.icns (database cylinder on a rounded background)."""
import pathlib, subprocess, tempfile
from PIL import Image, ImageDraw

ROOT = pathlib.Path(__file__).resolve().parent.parent
S = 1024

def draw(size: int) -> Image.Image:
    k = 4  # supersampling
    n = size * k
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    m = int(n * 0.09)
    # background with a vertical gradient
    grad = Image.new("RGBA", (n, n))
    gd = ImageDraw.Draw(grad)
    for y in range(n):
        t = y / n
        gd.line([(0, y), (n, y)], fill=(int(32 + 20 * t), int(120 - 50 * t), int(214 - 60 * t), 255))
    mask = Image.new("L", (n, n), 0)
    ImageDraw.Draw(mask).rounded_rectangle([m, m, n - m, n - m], radius=int(n * 0.2), fill=255)
    img.paste(grad, (0, 0), mask)
    # cylinder
    cx, w = n // 2, int(n * 0.46)
    top, h, eh = int(n * 0.25), int(n * 0.5), int(n * 0.13)
    x0, x1 = cx - w // 2, cx + w // 2
    body = (236, 242, 250, 255)
    edge = (40, 90, 170, 255)
    d.rectangle([x0, top + eh // 2, x1, top + h], fill=body)
    d.ellipse([x0, top + h - eh // 2, x1, top + h + eh // 2], fill=body)
    for i in (1, 2):
        y = top + h * i // 3
        d.arc([x0, y - eh // 2, x1, y + eh // 2], 0, 180, fill=edge, width=int(n * 0.014))
    d.ellipse([x0, top, x1, top + eh], fill=(255, 255, 255, 255), outline=edge, width=int(n * 0.014))
    return img.resize((size, size), Image.LANCZOS)

with tempfile.TemporaryDirectory() as tmp:
    iconset = pathlib.Path(tmp) / "DataCraft.iconset"
    iconset.mkdir()
    for s in (16, 32, 128, 256, 512):
        draw(s).save(iconset / f"icon_{s}x{s}.png")
        draw(s * 2).save(iconset / f"icon_{s}x{s}@2x.png")
    draw(512).save(ROOT / "assets" / "icon.png")
    subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(ROOT / "assets" / "DataCraft.icns")], check=True)
print("ok")
