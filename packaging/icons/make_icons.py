#!/usr/bin/env python3
"""生成 IOT_Power 图标：SVG（Linux scalable）、各尺寸 PNG（Linux hicolor / 窗口图标）、多尺寸 ICO（Windows）。

图形：深色圆角面板 + 显示屏（电压 / 电流两条曲线，配色同程序曲线）+ 红黑接线柱与电源灯。
≤32 px 用简化图形（面板 + 闪电），否则细节糊成一团。
依赖 Pillow：python3 packaging/icons/make_icons.py
"""

from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent
PNG_SIZES = [16, 24, 32, 48, 64, 128, 256, 512]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
SS = 4  # 超采样倍数

# 以 256×256 为设计坐标
BG_TOP, BG_BOTTOM = (0x2E, 0x37, 0x44), (0x14, 0x19, 0x20)
BORDER = (0x4A, 0x55, 0x63)
SCREEN, SCREEN_EDGE = (0x0A, 0x0E, 0x13), (0x33, 0x3B, 0x46)
GRID = (0x1B, 0x23, 0x2C)
VOLT, CURR = (0xFF, 0x7A, 0x6B), (0x3F, 0xD0, 0xC9)
POS, NEG, RING = (0xE6, 0x39, 0x46), (0x22, 0x25, 0x2A), (0xB8, 0xC0, 0xC8)
LED, BOLT = (0x3D, 0xDC, 0x97), (0xFF, 0xC8, 0x3D)

PANEL = (12, 12, 244, 244, 52)
SCREEN_BOX = (36, 40, 220, 146, 14)
VOLT_PTS = [(48, 122), (78, 122), (88, 70), (150, 70), (160, 122), (208, 122)]
CURR_PTS = [(48, 132), (84, 132), (90, 84), (98, 104), (148, 104), (154, 132), (208, 132)]
POSTS = [(84, 196, POS), (172, 196, NEG)]
POST_R, RING_W = 22, 6
LED_BOX = (122, 188, 134, 204)
BOLT_PTS = [(146, 40), (82, 140), (124, 140), (108, 216), (176, 110), (134, 110), (150, 40)]


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def panel_mask(size, k):
    m = Image.new("L", (size, size), 0)
    x0, y0, x1, y1, r = PANEL
    ImageDraw.Draw(m).rounded_rectangle([x0 * k, y0 * k, x1 * k, y1 * k], radius=r * k, fill=255)
    return m


def gradient(size):
    g = Image.new("RGBA", (size, size))
    d = ImageDraw.Draw(g)
    for y in range(size):
        d.line([(0, y), (size, y)], fill=lerp(BG_TOP, BG_BOTTOM, y / max(size - 1, 1)) + (255,))
    return g


def draw_full(size):
    k = size / 256
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    img.paste(gradient(size), (0, 0), panel_mask(size, k))
    d = ImageDraw.Draw(img)
    x0, y0, x1, y1, r = PANEL
    d.rounded_rectangle([x0 * k, y0 * k, x1 * k, y1 * k], radius=r * k, outline=BORDER, width=max(1, round(3 * k)))

    sx0, sy0, sx1, sy1, sr = SCREEN_BOX
    d.rounded_rectangle([sx0 * k, sy0 * k, sx1 * k, sy1 * k], radius=sr * k, fill=SCREEN, outline=SCREEN_EDGE,
                        width=max(1, round(3 * k)))
    for gx in range(68, 220, 32):
        d.line([(gx * k, (sy0 + 8) * k), (gx * k, (sy1 - 8) * k)], fill=GRID, width=max(1, round(2 * k)))
    for gy in (72, 104):
        d.line([((sx0 + 8) * k, gy * k), ((sx1 - 8) * k, gy * k)], fill=GRID, width=max(1, round(2 * k)))
    for pts, color in ((VOLT_PTS, VOLT), (CURR_PTS, CURR)):
        d.line([(x * k, y * k) for x, y in pts], fill=color, width=round(8 * k), joint="curve")

    for cx, cy, color in POSTS:
        d.ellipse([(cx - POST_R) * k, (cy - POST_R) * k, (cx + POST_R) * k, (cy + POST_R) * k], fill=RING)
        ir = POST_R - RING_W
        d.ellipse([(cx - ir) * k, (cy - ir) * k, (cx + ir) * k, (cy + ir) * k], fill=color)
        hr = 5
        d.ellipse([(cx - hr) * k, (cy - hr) * k, (cx + hr) * k, (cy + hr) * k], fill=RING)
    lx0, ly0, lx1, ly1 = LED_BOX
    d.rounded_rectangle([lx0 * k, ly0 * k, lx1 * k, ly1 * k], radius=3 * k, fill=LED)
    return img


def draw_small(size):
    k = size / 256
    img = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    img.paste(gradient(size), (0, 0), panel_mask(size, k))
    d = ImageDraw.Draw(img)
    x0, y0, x1, y1, r = PANEL
    d.rounded_rectangle([x0 * k, y0 * k, x1 * k, y1 * k], radius=r * k, outline=BORDER, width=max(1, round(6 * k)))
    d.polygon([(x * k, y * k) for x, y in BOLT_PTS], fill=BOLT)
    return img


def render(size):
    big = size * SS
    img = draw_small(big) if size <= 32 else draw_full(big)
    return img.resize((size, size), Image.LANCZOS)


def svg():
    def pts(p):
        return " ".join(f"{x},{y}" for x, y in p)

    def hexc(c):
        return "#%02x%02x%02x" % c

    x0, y0, x1, y1, r = PANEL
    sx0, sy0, sx1, sy1, sr = SCREEN_BOX
    grid = "".join(
        f'<line x1="{gx}" y1="{sy0 + 8}" x2="{gx}" y2="{sy1 - 8}"/>' for gx in range(68, 220, 32)
    ) + "".join(f'<line x1="{sx0 + 8}" y1="{gy}" x2="{sx1 - 8}" y2="{gy}"/>' for gy in (72, 104))
    posts = "".join(
        f'<circle cx="{cx}" cy="{cy}" r="{POST_R}" fill="{hexc(RING)}"/>'
        f'<circle cx="{cx}" cy="{cy}" r="{POST_R - RING_W}" fill="{hexc(c)}"/>'
        f'<circle cx="{cx}" cy="{cy}" r="5" fill="{hexc(RING)}"/>'
        for cx, cy, c in POSTS
    )
    lx0, ly0, lx1, ly1 = LED_BOX
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="256" height="256" viewBox="0 0 256 256">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="{hexc(BG_TOP)}"/>
      <stop offset="1" stop-color="{hexc(BG_BOTTOM)}"/>
    </linearGradient>
  </defs>
  <rect x="{x0}" y="{y0}" width="{x1 - x0}" height="{y1 - y0}" rx="{r}" fill="url(#bg)" stroke="{hexc(BORDER)}" stroke-width="3"/>
  <rect x="{sx0}" y="{sy0}" width="{sx1 - sx0}" height="{sy1 - sy0}" rx="{sr}" fill="{hexc(SCREEN)}" stroke="{hexc(SCREEN_EDGE)}" stroke-width="3"/>
  <g stroke="{hexc(GRID)}" stroke-width="2">{grid}</g>
  <g fill="none" stroke-width="8" stroke-linejoin="round" stroke-linecap="round">
    <polyline points="{pts(VOLT_PTS)}" stroke="{hexc(VOLT)}"/>
    <polyline points="{pts(CURR_PTS)}" stroke="{hexc(CURR)}"/>
  </g>
  {posts}
  <rect x="{lx0}" y="{ly0}" width="{lx1 - lx0}" height="{ly1 - ly0}" rx="3" fill="{hexc(LED)}"/>
</svg>
"""


def main():
    png_dir = OUT / "png"
    png_dir.mkdir(exist_ok=True)
    images = {}
    for s in PNG_SIZES:
        images[s] = render(s)
        images[s].save(png_dir / f"{s}x{s}.png", optimize=True)
    images[256].save(OUT / "iot-power.ico", sizes=[(s, s) for s in ICO_SIZES],
                     append_images=[images[s] for s in ICO_SIZES if s != 256])
    (OUT / "iot-power.svg").write_text(svg(), encoding="utf-8")
    # 预览：各尺寸并排
    w = sum(PNG_SIZES[:-1]) + 16 * len(PNG_SIZES)
    sheet = Image.new("RGBA", (w, 272), (0xEE, 0xEE, 0xEE, 255))
    x = 8
    for s in PNG_SIZES[:-1]:
        sheet.alpha_composite(images[s], (x, 264 - s))
        x += s + 16
    sheet.save(OUT / "preview.png")
    print("written to", OUT)


if __name__ == "__main__":
    main()
