"""Genera le icone di Eagle Voice: icona dell'app, icone del tray, SVG del segno.

Segno: un'aquila in volo fatta di punti quadrati bianchi su blu cobalto pieno, con due
linee sottili a mirino. Nelle misure piccole del tray i
punti non si leggono: lì l'aquila è una sagoma piena.

Uso: python -I tools\\genera-icone.py  (dalla radice del progetto, serve Pillow)
"""
from pathlib import Path
from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
ICONS = ROOT / "app" / "src-tauri" / "icons"
RES = ROOT / "app" / "src-tauri" / "resources"

COBALTO = (0x30, 0x39, 0xBB)
COBALTO_CHIARO = (0x8C, 0x94, 0xFF)  # registrazione sul tray scuro
NOTTE = (0x12, 0x17, 0x5C)  # aquila sul tray chiaro
BIANCO = (0xFF, 0xFF, 0xFF)
AMBRA = (0xF5, 0xA6, 0x23)
ROSSO = (0xE5, 0x48, 0x4D)
SS = 4  # sovracampionamento

# Aquila in volo vista da sotto, ali aperte con le penne a dita, coda a ventaglio,
# testa girata a sinistra. Coordinate in un riquadro 100x100.
AQUILA = [
    (42, 26), (45, 21), (41, 19), (35, 18), (40, 15), (46, 11), (52, 13), (55, 19),
    (58, 26), (68, 20), (80, 14), (92, 9), (99, 11), (93, 15), (99, 18), (92, 21),
    (98, 25), (90, 27), (94, 32), (85, 33), (76, 38), (66, 44), (60, 48), (60, 58),
    (66, 76), (58, 74), (50, 81), (42, 74), (34, 76), (40, 58), (40, 48), (34, 44),
    (24, 38), (15, 33), (6, 32), (10, 27), (2, 25), (8, 21), (1, 18), (7, 15),
    (1, 11), (8, 9), (20, 14), (32, 20),
]


def maschera(lato, riquadro):
    """Sagoma dell'aquila (L, 0-255) dentro riquadro=(x0, y0, x1, y1) di un'immagine lato x lato."""
    x0, y0, x1, y1 = riquadro
    ax0, ax1 = min(x for x, _ in AQUILA), max(x for x, _ in AQUILA)
    ay0, ay1 = min(y for _, y in AQUILA), max(y for _, y in AQUILA)
    k = min((x1 - x0) / (ax1 - ax0), (y1 - y0) / (ay1 - ay0))
    ox = x0 + ((x1 - x0) - (ax1 - ax0) * k) / 2
    oy = y0 + ((y1 - y0) - (ay1 - ay0) * k) / 2
    m = Image.new("L", (lato, lato), 0)
    ImageDraw.Draw(m).polygon([(ox + (x - ax0) * k, oy + (y - ay0) * k) for x, y in AQUILA], fill=255)
    return m


def punti(lato, riquadro, celle):
    """Celle della griglia (cx, cy, passo) coperte per più di metà dalla sagoma."""
    x0, y0, x1, y1 = riquadro
    passo = (x1 - x0) / celle
    m = maschera(lato, riquadro)
    px = m.load()
    out = []
    for j in range(celle):
        for i in range(celle):
            cx, cy = x0 + (i + 0.5) * passo, y0 + (j + 0.5) * passo
            dentro = sum(
                px[int(cx + dx * passo * 0.3), int(cy + dy * passo * 0.3)] > 127
                for dx in (-1, 0, 1) for dy in (-1, 0, 1)
            )
            if dentro >= 5:
                out.append((cx, cy, passo))
    return out


def icona_app(size, punto=None):
    big = max(size, 64) * SS
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, big - 1, big - 1], radius=big * 0.225, fill=COBALTO + (255,))
    # Mirino: due linee sottili che inquadrano l'aquila, come nel video.
    linea = max(1, round(big * 0.004))
    d.line([(big * 0.08, big * 0.70), (big * 0.92, big * 0.70)], fill=BIANCO + (110,), width=linea)
    d.line([(big * 0.74, big * 0.10), (big * 0.74, big * 0.90)], fill=BIANCO + (110,), width=linea)
    riq = (big * 0.10, big * 0.12, big * 0.90, big * 0.92)
    if size >= 48:
        for cx, cy, passo in punti(big, riq, 36):
            r = passo * 0.36
            d.rectangle([cx - r, cy - r, cx + r, cy + r], fill=BIANCO + (255,))
    else:
        img.paste(Image.new("RGBA", (big, big), BIANCO + (255,)), (0, 0), maschera(big, riq))
    if punto:
        r = big * 0.16
        d.ellipse([big - 2 * r - big * 0.02, big - 2 * r - big * 0.02, big - big * 0.02, big - big * 0.02],
                  fill=punto + (255,), outline=BIANCO + (255,), width=int(big * 0.035))
    return img.resize((size, size), Image.LANCZOS)


def icona_tray(colore, a_punti=False, punto=None, size=64):
    big = size * SS
    img = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    riq = (0, 0, big, big)
    if a_punti:
        d = ImageDraw.Draw(img)
        for cx, cy, passo in punti(big, riq, 14):
            r = passo * 0.42
            d.rectangle([cx - r, cy - r, cx + r, cy + r], fill=colore + (255,))
    else:
        img.paste(Image.new("RGBA", (big, big), colore + (255,)), (0, 0), maschera(big, riq))
    if punto:
        r = big * 0.17
        ImageDraw.Draw(img).ellipse([big - 2 * r, big - 2 * r, big - 1, big - 1], fill=punto + (255,))
    return img.resize((size, size), Image.LANCZOS)


def svg_segno():
    lato = 1024
    riq = (lato * 0.10, lato * 0.12, lato * 0.90, lato * 0.92)
    rects = []
    for cx, cy, passo in punti(lato, riq, 36):
        r = passo * 0.36
        rects.append(f'<rect x="{cx - r:.1f}" y="{cy - r:.1f}" width="{2 * r:.1f}" height="{2 * r:.1f}"/>')
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1024 1024">\n'
        '  <rect width="1024" height="1024" rx="230" fill="#3039BB"/>\n'
        '  <g stroke="#FFFFFF" stroke-opacity="0.43" stroke-width="4">'
        '<line x1="82" y1="717" x2="942" y2="717"/><line x1="758" y1="102" x2="758" y2="922"/></g>\n'
        '  <g fill="#FFFFFF">\n    ' + "\n    ".join(rects) + "\n  </g>\n</svg>\n"
    )


def main():
    # Icona dell'app in tutte le misure che Tauri usa su Windows.
    misure = {
        "32x32.png": 32, "64x64.png": 64, "128x128.png": 128, "128x128@2x.png": 256,
        "icon.png": 512, "logo.png": 1024, "StoreLogo.png": 50,
        "Square30x30Logo.png": 30, "Square44x44Logo.png": 44, "Square71x71Logo.png": 71,
        "Square89x89Logo.png": 89, "Square107x107Logo.png": 107, "Square142x142Logo.png": 142,
        "Square150x150Logo.png": 150, "Square284x284Logo.png": 284, "Square310x310Logo.png": 310,
    }
    for nome, s in misure.items():
        icona_app(s).save(ICONS / nome)
    # .ico: le misure piccole con la sagoma piena, le grandi a punti.
    ico = [icona_app(s) for s in (16, 24, 32, 48, 64, 128, 256)]
    ico[-1].save(ICONS / "icon.ico", sizes=[(i.width, i.height) for i in ico], append_images=ico[:-1])
    (ICONS / "eagle-voice.svg").write_text(svg_segno(), encoding="utf-8")

    # Tray: tema scuro di Windows = aquila chiara; tema chiaro (`_dark`) = aquila blu notte.
    icona_tray(BIANCO).save(RES / "tray_idle.png")
    icona_tray(NOTTE).save(RES / "tray_idle_dark.png")
    icona_tray(COBALTO_CHIARO).save(RES / "tray_recording.png")
    icona_tray(COBALTO).save(RES / "tray_recording_dark.png")
    icona_tray(COBALTO_CHIARO, a_punti=True).save(RES / "tray_transcribing.png")
    icona_tray(COBALTO, a_punti=True).save(RES / "tray_transcribing_dark.png")
    icona_tray(BIANCO, punto=AMBRA).save(RES / "tray_idle_warning.png")
    icona_tray(NOTTE, punto=AMBRA).save(RES / "tray_idle_warning_dark.png")
    # Tema "colorato": l'icona dell'app in piccolo.
    icona_app(64).save(RES / "eaglevoice.png")
    icona_app(64, punto=ROSSO).save(RES / "recording.png")
    icona_app(64).save(RES / "transcribing.png")
    icona_app(64, punto=AMBRA).save(RES / "eaglevoice_warning.png")
    print("icone scritte in", ICONS, "e", RES)


if __name__ == "__main__":
    main()
