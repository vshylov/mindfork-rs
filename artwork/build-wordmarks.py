#!/usr/bin/env python3
"""Генератор вордмарков mindfork: текст → кривые (self-contained SVG).

Зачем: SVG с `<text>` зависит от шрифта на машине зрителя. GitHub, чужой браузер
и просмотрщики в Linux нужного моноширинного шрифта не имеют — начертание
подменяется, и вордмарк выглядит сломанным. Поэтому глифы переводятся в `<path>`:
файл становится самодостаточным и рендерится одинаково везде.

Шрифт: **JetBrains Mono ExtraBold** (SIL Open Font License 1.1) — определён по
эталону `wordmark-example.png` подгонкой метрик (см. `artwork/README.md §Шрифт`).
OFL разрешает использование шрифта для создания артворка и распространение
полученных кривых; сам файл шрифта в репозиторий не кладём — он нужен только для
перегенерации.

Геометрия лockup'а (пропорции иконки к тексту) измерена с эталона — константы
`ASC_RATIO`/`GAP_RATIO`/`BASE_RATIO`/`TRACKING` ниже.

Запуск:
    python artwork/build-wordmarks.py [--font <путь к JetBrainsMono-ExtraBold.ttf>]

Без `--font` шрифт ищется в стандартных местах (системные шрифты, бандлы IDE
JetBrains). Перезаписывает `artwork/mindfork-wordmark*.svg`.
"""

from __future__ import annotations

import argparse
import glob
import os
import sys

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont

# --- Палитра бренда (см. artwork/README.md) ---------------------------------
ORANGE = "#c25a27"  # акцент: «fork» и ствол глифа
GRAY = "#5c6370"  # ветви глифа
PLATE = "#09090b"  # подложка иконки
LIGHT_TEXT = "#e4e4e7"  # «mind» на тёмном
DARK_TEXT = "#18181b"  # «mind» на светлом
MUTED = "#71717a"  # тэглайн

# --- Глиф иконки: 5 прямоугольников на сетке 16×16 ---------------------------
# Единственный источник истины — mindfork-icon-transparent.svg; здесь дубль для
# генерации. Тест widgets/logo.rs сверяет таблицу в коде с тем же SVG.
GLYPH = [
    (7, 2, 2, 12, ORANGE),
    (11, 2, 2, 5, GRAY),
    (9, 5, 2, 2, GRAY),
    (3, 7, 2, 5, GRAY),
    (5, 10, 2, 2, GRAY),
]

# --- Геометрия лockup'а, измеренная с wordmark-example.png --------------------
# Иконка в эталоне: 388×388 px; базовая линия текста, высота выносного элемента и
# зазор пересчитаны в доли от размера иконки, поэтому масштабируются на любой S.
ASC_RATIO = 0.5227  # высота выносного элемента ('d','f','k') / размер иконки
GAP_RATIO = 0.3608  # зазор «правый край иконки → левый край чернил текста» / S
BASE_RATIO = 0.7418  # базовая линия ниже верха иконки / S
TRACKING = -35  # межбуквенный трекинг, единицы шрифта (≈ -0.035em)

ASCENDER = 730  # высота 'd'/'f'/'k' в единицах шрифта
UPM = 1000
NAT_ADV = 600  # натуральный шаг моноширинного шрифта
ADV = NAT_ADV + TRACKING  # фактический шаг с трекингом
LSB_M = 38  # левый боковой отступ 'm' — начало чернил

FONT_CANDIDATES = [
    "C:/Windows/Fonts/JetBrainsMono-ExtraBold.ttf",
    "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-ExtraBold.ttf",
    "/usr/local/share/fonts/JetBrainsMono-ExtraBold.ttf",
    os.path.expanduser("~/.fonts/JetBrainsMono-ExtraBold.ttf"),
    "C:/Program Files/JetBrains/*/jbr/lib/fonts/JetBrainsMono-ExtraBold.ttf",
]


def find_font(explicit: str | None) -> str:
    if explicit:
        if not os.path.isfile(explicit):
            sys.exit(f"шрифт не найден: {explicit}")
        return explicit
    for pat in FONT_CANDIDATES:
        for hit in sorted(glob.glob(pat), reverse=True):
            if os.path.isfile(hit):
                return hit
    sys.exit(
        "JetBrainsMono-ExtraBold.ttf не найден.\n"
        "Скачать: https://github.com/JetBrains/JetBrainsMono/releases (OFL 1.1)\n"
        "или указать путь: --font <путь>"
    )


def glyph_paths(font: TTFont, text: str, start_index: int = 0) -> str:
    """Кривые подстроки одним `d`, в единицах шрифта, с учётом трекинга.

    `start_index` — позиция подстроки в целом слове (чтобы «fork» встал на свои
    места, а не начинался с нуля).
    """
    gs = font.getGlyphSet()
    cmap = font.getBestCmap()
    pen = SVGPathPen(gs, ntos=lambda v: f"{v:g}")
    for i, ch in enumerate(text):
        dx = (start_index + i) * ADV
        gs[cmap[ord(ch)]].draw(TransformPen(pen, (1, 0, 0, 1, dx, 0)))
    return pen.getCommands()


def ink_width(text: str = "mindfork") -> float:
    """Ширина чернил слова в единицах шрифта (для раскладки и viewBox)."""
    # 'm'.xMin=38 … 'k'.xMax=580 (замерено по шрифту; см. artwork/README.md)
    return (len(text) - 1) * ADV + 580 - LSB_M


def icon(size: float, x: float, y: float, plate: bool, mono: bool) -> str:
    """Иконка: опциональная скруглённая подложка + глиф на сетке 16×16."""
    s = size / 16.0
    out = [f'  <g shape-rendering="crispEdges" transform="translate({x:g},{y:g})">']
    if plate:
        out.append(
            f'    <rect width="{size:g}" height="{size:g}" '
            f'rx="{size * 0.1875:g}" fill="{PLATE}"/>'
        )
    out.append(f'    <g transform="scale({s:g})">')
    for gx, gy, gw, gh, color in GLYPH:
        fill = "currentColor" if mono else color
        out.append(
            f'      <rect x="{gx}" y="{gy}" width="{gw}" height="{gh}" fill="{fill}"/>'
        )
    out.append("    </g>")
    out.append("  </g>")
    return "\n".join(out)


def wordmark_group(
    font: TTFont,
    size: float,
    ink_x: float,
    baseline: float,
    split: bool,
    text_fill: str,
) -> str:
    """Текст «mindfork» кривыми.

    `split=True` — два пути: «mind» цветом `text_fill` и «fork» акцентом; иначе
    одно слово в `currentColor` (одноцветный вариант). Кривые остаются в единицах
    шрифта, а масштаб и переворот оси Y задаёт трансформация группы — так `d`
    читаем и правится вручную.
    """
    scale = size / UPM
    # Перо в нуле даёт чернила от LSB_M — сдвигаем так, чтобы они легли на ink_x.
    tx = ink_x - LSB_M * scale
    out = [
        f'  <g transform="translate({tx:g},{baseline:g}) scale({scale:g},{-scale:g})">'
    ]
    if split:
        out.append(f'    <path fill="{text_fill}" d="{glyph_paths(font, "mind", 0)}"/>')
        out.append(f'    <path fill="{ORANGE}" d="{glyph_paths(font, "fork", 4)}"/>')
    else:
        out.append(f'    <path fill="currentColor" d="{glyph_paths(font, "mindfork")}"/>')
    out.append("  </g>")
    return "\n".join(out)


def svg(width: float, height: float, body: str, extra: str = "") -> str:
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width:g} {height:g}"'
        f' width="{width:g}" height="{height:g}" role="img" aria-label="mindfork"{extra}>\n'
        f"  <title>mindfork</title>\n{body}\n</svg>\n"
    )


def horizontal(font: TTFont, s: float, text_fill: str | None, mono: bool) -> str:
    """Горизонтальный лockup: иконка слева, «mindfork» справа."""
    size = ASC_RATIO * s / (ASCENDER / UPM)
    scale = size / UPM
    ink_x = s + GAP_RATIO * s
    baseline = BASE_RATIO * s
    w = ink_x + ink_width() * scale
    body = icon(s, 0, 0, plate=not mono, mono=mono)
    body += "\n" + wordmark_group(
        font, size, ink_x, baseline, split=not mono, text_fill=text_fill or LIGHT_TEXT
    )
    extra = ' fill="currentColor" color="#e4e4e7"' if mono else ""
    return svg(round(w, 1), s, body, extra)


# Вертикальный лockup: в отличие от горизонтального, эталона нет — пропорции
# выведены. Ширина слова = 2× размера иконки (иначе моноширинное слово из 8 букв
# уезжает в 3.2× и лockup становится нижне-тяжёлым); вертикальный зазор взят в той
# же доле от высоты выносного элемента, что и горизонтальный (0.3608/0.5227).
STACK_TEXT_RATIO = 2.0
GAP_PER_ASC = GAP_RATIO / ASC_RATIO


def stacked(font: TTFont, s: float) -> str:
    """Вертикальный лockup: иконка сверху, «mindfork» под ней по центру."""
    w_text = STACK_TEXT_RATIO * s
    size = w_text / ink_width() * UPM
    scale = size / UPM
    asc = ASCENDER * scale
    w = max(w_text, s)
    baseline = s + GAP_PER_ASC * asc + asc
    body = icon(s, (w - s) / 2, 0, plate=True, mono=False)
    body += "\n" + wordmark_group(
        font, size, (w - w_text) / 2, baseline, split=True, text_fill=LIGHT_TEXT
    )
    # запас снизу под выносной вылет 'o'/'d' (-10 единиц), иначе их срежет viewBox
    return svg(round(w, 1), round(baseline + 10 * scale, 1), body)


def tagline(font: TTFont, tag_font: TTFont, s: float, text: str) -> str:
    """Горизонтальный лockup + тэглайн под словом, выключенный по его ширине."""
    size = ASC_RATIO * s / (ASCENDER / UPM)
    scale = size / UPM
    ink_x = s + GAP_RATIO * s
    baseline = BASE_RATIO * s
    w_text = ink_width() * scale
    w = ink_x + w_text

    # Тэглайн: подбираем кегль и трекинг так, чтобы строка выключилась ровно по
    # ширине вордмарка (классический приём — тэглайн «под словом», край в край).
    gs = tag_font.getGlyphSet()
    cmap = tag_font.getBestCmap()
    n = len(text)
    tag_size = size * 0.26
    tsc = tag_size / UPM
    # ширина = (n-1)*(600+track) + xMax_last - xMin_first, решаем относительно track
    first = cmap[ord(text[0])]
    last = cmap[ord(text[-1])]
    from fontTools.pens.boundsPen import BoundsPen

    bp = BoundsPen(gs)
    gs[first].draw(bp)
    xmin = bp.bounds[0]
    bp = BoundsPen(gs)
    gs[last].draw(bp)
    xmax = bp.bounds[2]
    track = (w_text / tsc - (xmax - xmin)) / (n - 1) - NAT_ADV

    pen = SVGPathPen(gs, ntos=lambda v: f"{v:g}")
    for i, ch in enumerate(text):
        gs[cmap[ord(ch)]].draw(
            TransformPen(pen, (1, 0, 0, 1, i * (NAT_ADV + track), 0))
        )
    tag_base = baseline + size * 0.42
    tx = ink_x - xmin * tsc

    body = icon(s, 0, 0, plate=True, mono=False)
    body += "\n" + wordmark_group(font, size, ink_x, baseline, split=True, text_fill=LIGHT_TEXT)
    body += (
        f'\n  <g transform="translate({tx:g},{tag_base:g}) scale({tsc:g},{-tsc:g})">\n'
        f'    <path fill="{MUTED}" d="{pen.getCommands()}"/>\n  </g>'
    )
    return svg(round(w, 1), round(tag_base + size * 0.1, 1), body)


def main() -> None:
    # Консоль Windows по умолчанию cp1252/cp866 — русский вывод её роняет.
    for stream in (sys.stdout, sys.stderr):
        try:
            stream.reconfigure(encoding="utf-8", errors="replace")
        except (AttributeError, OSError):
            pass

    ap = argparse.ArgumentParser(description="Сборка вордмарков mindfork (текст → кривые)")
    ap.add_argument("--font", help="путь к JetBrainsMono-ExtraBold.ttf")
    ap.add_argument("--tag-font", help="путь к JetBrainsMono-Bold.ttf (тэглайн)")
    args = ap.parse_args()

    path = find_font(args.font)
    tag_path = args.tag_font or path.replace("ExtraBold", "Bold")
    if not os.path.isfile(tag_path):
        tag_path = path
    font, tag_font = TTFont(path), TTFont(tag_path)
    print(f"шрифт:   {path}")
    print(f"тэглайн: {tag_path}")

    here = os.path.dirname(os.path.abspath(__file__))
    files = {
        "mindfork-wordmark.svg": horizontal(font, 48, LIGHT_TEXT, mono=False),
        "mindfork-wordmark-dark.svg": horizontal(font, 48, LIGHT_TEXT, mono=False),
        "mindfork-wordmark-light.svg": horizontal(font, 48, DARK_TEXT, mono=False),
        "mindfork-wordmark-mono.svg": horizontal(font, 48, None, mono=True),
        "mindfork-wordmark-stacked.svg": stacked(font, 64),
        "mindfork-wordmark-tagline.svg": tagline(
            font, tag_font, 52, "TUI · RUST · LOCAL LLM"
        ),
    }
    for name, content in files.items():
        with open(os.path.join(here, name), "w", encoding="utf-8", newline="\n") as fh:
            fh.write(content)
        print(f"  ✓ {name}  ({len(content)} байт)")


if __name__ == "__main__":
    main()
