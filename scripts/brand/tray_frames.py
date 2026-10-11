#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Regenerates Pip's menu bar frames (assets/tray/*.rgba).

The menu bar icon is a template image: only the alpha channel matters, macOS
tints it for light and dark menu bars. Each frame is 96x72 raw RGBA
(shown at 24x18 points). Needs: pip install playwright pillow && playwright install chromium

    python3 scripts/brand/tray_frames.py           # writes assets/tray/*.rgba
    python3 scripts/brand/tray_frames.py --preview # also writes tray-preview.html
"""
import os, sys
HEAD = """
<path d="M33.2 11 L31.4 0.8 L37 8.6 Z"/>
<path d="M36.6 9.2 L40.2 0.4 L41 10.4 Z"/>
<path d="M31 14 C31 10.6 33.8 8.8 36.8 9 C39.4 9.2 41 10.8 41.6 12.6 L45.8 14.6 C46.4 15 46.1 15.8 45.4 15.9 L41 16.6 C39.6 18.4 36.6 19.2 34.2 18.4 C32.2 17.7 31 16 31 14 Z"/>
"""
EYE = '<circle cx="38.6" cy="12.9" r="1.0" fill="#000"/>'
BODY = '<path d="M14 19 C14 15.5 19 14 25 14.2 C30 14.4 33 15.6 34.5 17.5 C35.5 19 34.6 21.6 32 22.6 C28.5 23.8 21 24 17.5 23.2 C15.2 22.7 14 21 14 19 Z"/>'
TAIL = '<path d="M15.5 18.6 C10 18.6 5 16.6 2.2 12.8 C1.3 11.5 2 9.6 3.6 9.9 C6.2 10.4 8.6 12.4 12 13.7 C13.6 14.3 15.2 14.6 16.8 14.8 Z"/>'
def leg(x, y, ang, length=10.5, w=2.4):
    return f'<rect x="{x-w/2}" y="{y}" width="{w}" height="{length}" rx="{w/2}" transform="rotate({ang} {x} {y})"/>'
RUN = [
    [(17.5,20,42),(20.5,20,26),(30,19.5,-26),(32.5,19.5,-42)],
    [(17.5,20,12),(20.5,20,-4),(30,19.5,8),(32.5,19.5,-8)],
    [(17.5,20,-30),(20.5,20,-16),(30,19.5,30),(32.5,19.5,16)],
    [(17.5,20,-6),(20.5,20,10),(30,19.5,-10),(32.5,19.5,6)],
]
def run_frame(i, bob):
    legs = "".join(leg(x,y,a) for x,y,a in RUN[i])
    tail_rot = [-8, -2, 6, 0][i]
    return f'<g transform="translate(0 {bob})"><g transform="rotate({tail_rot} 16 17)">{TAIL}</g>{BODY}{legs}{HEAD}</g>'
SLEEP = """
<path d="M8 31 C8 23 14 18 22 18 C31 18 38 23 38 29 C38 31.5 36.5 33 34 33 L11 33 C9 33 8 32.2 8 31 Z"/>
<path d="M37.6 29.4 C40.4 25.4 44.8 25.8 45 29.4 C45.2 32.8 41 34.6 34 34.6 L13 34.6 C11.8 34.6 11.6 33.6 12.8 33.4 C21 32.4 29.6 32.6 34.6 31.6 Z"/>
<path d="M5.8 30 C5.4 26.4 7.8 23.8 11.2 23.8 C14.6 23.8 16.8 26 16.8 28.8 C16.8 31.6 14.2 33.2 11.2 33.2 C8.2 33.2 6.2 32 5.8 30 Z"/>
<path d="M7.4 25.6 L5.2 16.8 L11.2 23.6 Z"/>
<path d="M11.2 24.2 L13 15.6 L15.8 25 Z"/>
<text x="31" y="13" font-family="Helvetica, Arial" font-weight="bold" font-size="9">z</text>
<text x="38" y="7.5" font-family="Helvetica, Arial" font-weight="bold" font-size="6.5">z</text>
"""
SLEEP_EYE = '<path d="M8.4 28.4 Q10.2 29.9 12 28.4" stroke="#000" stroke-width="1.1" fill="none" stroke-linecap="round"/>'
def svg(inner, eye, scale):
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="{48*scale}" height="{36*scale}" viewBox="0 0 48 36">
<defs><mask id="m"><rect width="48" height="36" fill="#fff"/>{eye.replace('#000','#000')}</mask></defs>
<g mask="url(#m)" fill="#000">{inner}</g></svg>'''
frames = {f"run{i}": (run_frame(i, [0, -0.6, -1.4, -0.6][i]), EYE) for i in range(4)}
frames["sleep"] = (SLEEP, SLEEP_EYE)


def main():
    root = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..")
    out = os.path.join(root, "assets", "tray")
    os.makedirs(out, exist_ok=True)
    from playwright.sync_api import sync_playwright
    from PIL import Image
    import io
    with sync_playwright() as p:
        b = p.chromium.launch()
        pg = b.new_page(viewport={"width": 96, "height": 72})
        for name, (inner, eye) in frames.items():
            pg.set_content('<body style="margin:0;background:transparent">' + svg(inner, eye, 2) + "</body>")
            png = pg.screenshot(omit_background=True, clip={"x": 0, "y": 0, "width": 96, "height": 72})
            alpha = Image.open(io.BytesIO(png)).convert("RGBA").getchannel("A")
            raw = bytes(v for a in alpha.tobytes() for v in (0, 0, 0, a))
            with open(os.path.join(out, name + ".rgba"), "wb") as f:
                f.write(raw)
            print("wrote", name)
        b.close()
    if "--preview" in sys.argv:
        html = '<body style="background:#2b2b2b;display:flex;gap:24px;padding:24px">' + "".join(
            svg(i, e, 4).replace('fill="#000">', 'fill="#fff">', 1) for i, e in frames.values()) + "</body>"
        open("tray-preview.html", "w").write(html)


if __name__ == "__main__":
    main()
