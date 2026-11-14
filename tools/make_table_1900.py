# Full-window table screenshot: 1900px wide, 15px text. This is the case where
# DetMaxSide used to shrink the image to 1280 and loss the narrow columns.
import os
from PIL import Image, ImageDraw, ImageFont

FONT = "C:/Windows/Fonts/msyh.ttc"
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "compare")

HEAD = ["客户名称", "订单号", "下单日期", "数量", "单价", "金额", "状态"]
NAMES = ["东软集团股份有限公司", "上海某某信息技术有限公司", "北京云图科技", "广州华南电子",
         "成都天启网络", "杭州数联智能", "南京紫金软件", "武汉光谷数据", "西安丝路科技", "苏州工业云"]
IMG = Image.new("RGB", (1900, 700), (255, 255, 255))
d = ImageDraw.Draw(IMG)
font = ImageFont.truetype(FONT, 15)
small = ImageFont.truetype(FONT, 14)

x0, y0 = 24, 40
row_h = 40
colw = [250, 210, 170, 90, 150, 160, 120]
HEAD_W = 200

def cell(x, y, text, w, right=False):
    d.text((x + 12, y + 12), text, font=font, fill=(25, 25, 25))

# header band
d.rectangle([x0, y0, 1900 - 24, y0 + row_h], fill=(238, 242, 248))
x = x0
for c, name in enumerate(HEAD):
    cell(x, y0, name, colw[c])
    x += colw[c]
for i, name in enumerate(NAMES):
    y = y0 + row_h * (i + 1)
    x = x0
    vals = [name, "SO-2025-%04d" % (120 + i), "2025-0%d-1%d" % (1 + i % 9, i % 10),
            str([120, 36, 8, 240, 15, 60, 4, 99, 33, 7][i]), "%.2f" % (48 + i * 7.5),
            format(5760 + i * 313.7, ",.2f"), ["已完成", "进行中", "待审核"][i % 3]]
    for c, v in enumerate(vals):
        cell(x, y, v, colw[c])
        x += colw[c]
    d.line([(x0, y), (1900 - 24, y)], fill=(214, 214, 214))
# vertical rules
x = x0
for c in range(len(HEAD) + 1):
    d.line([(x, y0), (x, y0 + row_h * (len(NAMES) + 1))], fill=(214, 214, 214))
    if c < len(HEAD):
        x += colw[c]

path = os.path.join(OUT, "table_1900.png")
IMG.save(path)
print(path, IMG.size)
