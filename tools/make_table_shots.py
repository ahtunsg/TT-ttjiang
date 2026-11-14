# Larger, screen-like table shots: this is what an Excel/CRM screenshot looks like
# after the user drags a region, so the detector runs at full resolution.
import os
from PIL import Image, ImageDraw, ImageFont

FONT = "C:/Windows/Fonts/msyh.ttc"
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "compare")

HEAD = ["客户名称", "订单号", "下单日期", "数量", "单价", "金额"]
ROWS = [
    ["东软集团股份有限公司", "SO-2024-0091", "2024-03-11", "120", "48.00", "5,760.00"],
    ["上海某某信息技术", "SO-2024-0092", "2024-03-12", "36", "1,160.17", "41,766.12"],
    ["北京云图科技", "SO-2024-0093", "2024-03-14", "8", "99.00", "792.00"],
    ["广州华南电子", "SO-2024-0094", "2024-03-15", "240", "12.50", "3,000.00"],
    ["成都天启网络", "SO-2024-0095", "2024-03-18", "15", "2,400.00", "36,000.00"],
]


def shot(name, size, pad, gap, border=True, scale=1):
    font = ImageFont.truetype(FONT, size)
    row_h = int(size * 1.85)
    colw = []
    for c in range(len(HEAD)):
        w = max(font.getlength(HEAD[c]), max(font.getlength(r[c]) for r in ROWS))
        colw.append(int(w) + 2 * pad)
    W = sum(colw) + gap * (len(colw) - 1) + 2 * pad + 20
    H = row_h * (len(ROWS) + 1) + 2 * pad
    img = Image.new("RGB", (int(W), int(H)), (255, 255, 255))
    d = ImageDraw.Draw(img)
    y = pad
    for i in range(len(ROWS) + 1):
        cells = HEAD if i == 0 else ROWS[i - 1]
        x = pad + 10
        for c, txt in enumerate(cells):
            d.text((x + pad, y + (row_h - size) / 2 - 2), txt, font=font, fill=(15, 15, 15))
            if border:
                d.line([(x, y), (x, y + row_h)], fill=(150, 150, 150))
            x += colw[c] + gap
        if border:
            d.line([(pad + 10, y), (x - gap, y)], fill=(150, 150, 150))
        y += row_h
    if border:
        d.line([(pad + 10, y), (x - gap, y)], fill=(150, 150, 150))
    if scale != 1:
        img = img.resize((int(img.width * scale), int(img.height * scale)), Image.LANCZOS)
    path = os.path.join(OUT, name)
    img.save(path)
    print(name, img.size, "font", size, "gap", gap, "border", border)


shot("shot_grid.png", 16, 8, 16, True)        # default Excel grid
shot("shot_grid_large.png", 26, 12, 24, True)  # zoomed / HiDPI
shot("shot_plain.png", 17, 8, 20, False)      # no rules
shot("shot_6col_big.png", 20, 10, 14, True)   # dense 6 columns
