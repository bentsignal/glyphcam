"""Generate glyphcam icons (PNG/ICO/ICNS) from the bundled fonts.

python packaging/icons/make_icons.py   (needs Pillow)
"""
import os, random
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
MONO = os.path.join(ROOT, "assets", "fonts", "DejaVuSansMono.ttf")
KANA = os.path.join(ROOT, "assets", "fonts", "glyphcam-kana.ttf")

def icon(size=1024, rain=True):
    s = size
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, s - 1, s - 1], radius=int(s * 0.22), fill=255)
    bg = Image.new("RGBA", (s, s), (3, 14, 7, 255))
    # vertical gradient
    g = ImageDraw.Draw(bg)
    for y in range(s):
        t = y / s
        g.line([(0, y), (s, y)], fill=(2, int(10 + 18 * (1 - t)), int(6 + 6 * (1 - t)), 255))
    if rain:
        rnd = random.Random(7)
        kana = ImageFont.truetype(KANA, int(s * 0.075))
        chars = [chr(c) for c in range(0xFF66, 0xFF9D)]
        cols = 9
        for c in range(cols):
            x = int((c + 0.5) * s / cols - s * 0.03)
            head = rnd.randint(2, 12)
            for r in range(13):
                y = int(r * s / 12.5) - int(s * 0.02)
                d = head - r
                if d < 0 or d > 7:
                    continue
                a = int(150 * (1 - d / 8) ** 1.6) + 20
                col = (180, 255, 200, 255) if d == 0 else (20, 200, 80, a)
                g.text((x, y), rnd.choice(chars), font=kana, fill=col)
    img.paste(bg, (0, 0), mask)
    # big glowing "g"
    font = ImageFont.truetype(MONO, int(s * 0.78))
    layer = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(layer)
    bbox = d.textbbox((0, 0), "g", font=font)
    w, h = bbox[2] - bbox[0], bbox[3] - bbox[1]
    pos = ((s - w) // 2 - bbox[0], (s - h) // 2 - bbox[1] - int(s * 0.02))
    d.text(pos, "g", font=font, fill=(40, 255, 110, 255))
    glow = layer.filter(ImageFilter.GaussianBlur(s * 0.03))
    img = Image.alpha_composite(img, Image.composite(glow, Image.new("RGBA", (s, s)), mask))
    img = Image.alpha_composite(img, Image.composite(layer, Image.new("RGBA", (s, s)), mask))
    d = ImageDraw.Draw(img)
    d.text(pos, "g", font=font, fill=(150, 255, 180, 70))
    return img

def template(size):
    """Monochrome menu-bar icon (macOS template image)."""
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    font = ImageFont.truetype(MONO, int(size * 0.95))
    d = ImageDraw.Draw(img)
    bbox = d.textbbox((0, 0), "g", font=font)
    w, h = bbox[2] - bbox[0], bbox[3] - bbox[1]
    d.text(((size - w) // 2 - bbox[0], (size - h) // 2 - bbox[1]), "g", font=font, fill=(0, 0, 0, 255))
    return img

big = icon(1024)
os.makedirs(os.path.join(ROOT, "assets", "icons"), exist_ok=True)
big.resize((256, 256), Image.LANCZOS).save(os.path.join(ROOT, "assets", "icons", "glyphcam-256.png"))
icon(256, rain=False).resize((64, 64), Image.LANCZOS).save(os.path.join(ROOT, "assets", "icons", "tray-64.png"))
template(64).save(os.path.join(ROOT, "assets", "icons", "tray-template-64.png"))
big.save(os.path.join(ROOT, "packaging", "windows", "glyphcam.ico"),
         sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
big.save(os.path.join(ROOT, "packaging", "macos", "glyphcam.icns"))
big.resize((512, 512), Image.LANCZOS).save(os.path.join(ROOT, "packaging", "icons", "glyphcam-512.png"))
print("icons written")
