# Build synthetic table screenshots to reproduce "multi-column collapsed to one column".
# Column gap and font size are the two parameters that actually break SplitCells.
import os
from PIL import Image, ImageDraw, ImageFont

FONT = "C:/Windows/Fonts/msyh.ttc"
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "compare")

HEAD = ["项目", "数量", "单价", "金额"]
ROWS = [
    ["办公用品", "12", "48.00", "576.00"],
    ["差旅费", "3", "1,160.17", "3,480.50"],
    ["软件订阅", "24", "99.00", "2,376.00"],
    ["合计", "", "", "6,432.50"],
]


def table(name, size, pad, gap, border, width=None):
    font = ImageFont.truetype(FONT, size)
    row_h = int(size * 1.9)
    # column widths from content
    colw = []
    for c in range(len(HEAD)):
        w = max(font.getlength(HEAD[c]), max(font.getlength(r[c]) for r in ROWS))
        colw.append(int(w) + 2 * pad)
    W = sum(colw) + gap * (len(colw) - 1) + 2 * pad + 20
    H = row_h * (len(ROWS) + 1) + 2 * pad
    if width:
        W = width
    img = Image.new("RGB", (int(W), int(H)), (255, 255, 255))
    d = ImageDraw.Draw(img)

    y = pad
    for i in range(len(ROWS) + 1):
        cells = HEAD if i == 0 else ROWS[i - 1]
        x = pad + 10
        for c, txt in enumerate(cells):
            d.text((x + pad, y + (row_h - size) / 2 - 2), txt,
                   font=font, fill=(20, 20, 20))
            if border:
                d.line([(x, y), (x, y + row_h)], fill=(160, 160, 160))
            x += colw[c] + gap
        if border:
            d.line([(pad + 10, y), (x - gap, y)], fill=(160, 160, 160))
        y += row_h
    if border:
        d.line([(pad + 10, y), (x - gap, y)], fill=(160, 160, 160))
    path = os.path.join(OUT, name)
    img.save(path)
    print(name, img.size, "font", size, "gap", gap, "border", border)


table("tbl_tight.png", 15, 6, 14, True)     # small font, narrow gap
table("tbl_wide.png", 24, 10, 40, True)     # large font, wide gap
table("tbl_noborder.png", 18, 8, 18, False)  # no rules at all
table("tbl_4col_dense.png", 14, 4, 10, True)
