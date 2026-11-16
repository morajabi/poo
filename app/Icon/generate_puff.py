# Generates Puff character variants (shapes x colors) as Icon Composer bundles: python3 gen3.py <out-dir>
import json, math, os, sys
OUT = sys.argv[1]
C = 512

def svg(body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">{body}</svg>\n'
def color(r, g, b, a=1.0): return f"srgb:{r:.5f},{g:.5f},{b:.5f},{a:.5f}"
def hexc(h, a=1.0):
    h = h.lstrip('#'); return color(*(int(h[i:i+2], 16)/255 for i in (0, 2, 4)), a)
def grad(a, b): return {"linear-gradient": [hexc(a), hexc(b)]}
def solid(h): return {"solid": hexc(h)}
def f2(p): return f"{p[0]:.1f},{p[1]:.1f}"

def group(name, layers, shadow=0.5, translucency=0.0, specular=True):
    return {"name": name, "layers": layers, "shadow": {"kind": "neutral", "opacity": shadow},
            "translucency": {"enabled": translucency > 0, "value": translucency}, "specular": specular}
def layer(name, image, fill=None, dark=None, opacity=None):
    l = {"name": name, "image-name": image}
    if fill:
        l["fill-specializations"] = [{"value": fill}, {"appearance": "dark", "value": dark or fill}]
    if opacity is not None: l["opacity"] = opacity
    return l

def sparkle(cx, cy, s, k=0.16):
    q = s*k
    return (f"M{cx},{cy-s} Q{cx+q},{cy-q} {cx+s},{cy} Q{cx+q},{cy+q} {cx},{cy+s} "
            f"Q{cx-q},{cy+q} {cx-s},{cy} Q{cx-q},{cy-q} {cx},{cy-s} Z")

# The face, exactly as in Puff (centered near 514,553); shapes move it, never reshape it.
FACE = ('<g fill="#2B2350">'
        '<ellipse cx="438" cy="520" rx="34" ry="44"/><ellipse cx="590" cy="520" rx="34" ry="44"/>'
        '</g>'
        '<path d="M474,606 Q514,640 554,606" stroke="#2B2350" stroke-width="20" stroke-linecap="round" fill="none"/>'
        '<g fill="#FF8FB1" opacity="0.8"><ellipse cx="380" cy="600" rx="40" ry="24"/><ellipse cx="648" cy="600" rx="40" ry="24"/></g>'
        '<g fill="#fff"><circle cx="450" cy="504" r="11"/><circle cx="602" cy="504" r="11"/></g>')
def face(dy=0): return f'<g transform="translate(0 {dy})">{FACE}</g>'

# ---- shapes: (body path svg, groove svg or "", face dy, sparkle spot) ----
def fluff(cx, cy, R, bumps=14, amp=26):
    pts = []
    for i in range(bumps):
        a0 = 2*math.pi*i/bumps; a1 = 2*math.pi*(i+1)/bumps; am = (a0+a1)/2
        pts.append(((cx+R*math.cos(a0), cy+R*math.sin(a0)), (cx+(R+amp*2.2)*math.cos(am), cy+(R+amp*2.2)*math.sin(am))))
    d = f"M{f2(pts[0][0])}"
    for i, (p0, pm) in enumerate(pts):
        d += f" Q{f2(pm)} {f2(pts[(i+1) % bumps][0])}"
    return d + " Z"

def capsule(cx, y, w, h):
    r = h/2; x = cx - w/2
    return f"M{x+r},{y} H{x+w-r} A{r},{r} 0 0 1 {x+w-r},{y+h} H{x+r} A{r},{r} 0 0 1 {x+r},{y} Z"

def swirl():
    tiers = [(C, 588, 650, 262), (C, 436, 486, 206), (C, 304, 330, 172)]
    body = "".join(f'<path d="{capsule(*t)}"/>' for t in tiers)
    # the little curl on top, leaning right
    body += '<path d="M436,346 C424,262 500,214 604,182 C584,232 600,270 590,312 C580,352 452,372 436,346 Z"/>'
    grooves = ""
    for cx, y, w, h in tiers[1:]:
        x0, x1, yb = cx - w/2 + 46, cx + w/2 - 46, y + h - 22
        grooves += f'<path d="M{x0},{yb} Q{cx},{yb+46} {x1},{yb}" stroke="#000" stroke-width="18" stroke-linecap="round" fill="none"/>'
    return body, grooves, 166, (812, 272)

def drop():
    d = "M512,196 C590,300 790,420 790,606 A278,278 0 0 1 234,606 C234,420 434,300 512,196 Z"
    return f'<path d="{d}"/>', "", 70, (800, 250)

def puff():
    return f'<path d="{fluff(C, 556, 262)}"/>', "", 0, (790, 236)

SHAPES = {"Puff": puff, "Swirl": swirl, "Drop": drop}

def write(name, shape, bg, body_fill, groove_hex=None, spark=True, body_dark=None):
    body, grooves, dy, sp = SHAPES[shape]()
    d = os.path.join(OUT, f"{name}.icon"); os.makedirs(os.path.join(d, "Assets"), exist_ok=True)
    assets = {"body.svg": f'<g fill="#fff">{body}</g>', "face.svg": face(dy)}
    groups = []
    if spark:
        assets["spark.svg"] = f'<path fill="#fff" d="{sparkle(*sp, 92)}"/><path fill="#fff" d="{sparkle(sp[0]-90, sp[1]-86, 40)}"/>'
        groups.append(group("Spark", [layer("Spark", "spark.svg", solid("#FFFFFF"))], shadow=0.3))
    groups.append(group("Face", [layer("Face", "face.svg")], shadow=0.15))
    body_layers = []
    if grooves:
        assets["grooves.svg"] = grooves
        body_layers.append(layer("Grooves", "grooves.svg", solid(groove_hex), opacity=0.55))
    body_layers.append(layer("Body", "body.svg", body_fill, body_dark))
    groups.append(group("Body", body_layers, shadow=0.5, translucency=0.1))
    for fn, b in assets.items():
        open(os.path.join(d, "Assets", fn), "w").write(svg(b))
    doc = {"fill": bg, "groups": groups, "supported-platforms": {"squares": ["macOS"]}}
    open(os.path.join(d, "icon.json"), "w").write(json.dumps(doc, indent=2) + "\n")

CARAMEL = grad("#E0A46A", "#B9763F"); TOFFEE = grad("#D08A4E", "#9C5A2C")
CHOC = grad("#A8724C", "#7A4A2C"); COCOA = grad("#B88466", "#8A5A40"); VANILLA = grad("#FFFBF3", "#F3E2C8")
CREAM_BG = grad("#FFF4E2", "#F7D7A8"); BUTTER = grad("#FFF0A8", "#FFC84A"); SKY = grad("#A8E4FF", "#4C9DFF")
MINT = grad("#C4F7E2", "#46CFA0"); PEACH = grad("#FFE0C9", "#FF9F7E"); CHOC_BG = grad("#8E5B3C", "#5A341F")
BLUE = grad("#7FD6FF", "#3D7BFF")

V = [
    ("SwirlCaramelCream", "Swirl", CREAM_BG, CARAMEL, "#7A4420", True),
    ("SwirlChocolateButter", "Swirl", BUTTER, CHOC, "#3E2212", True),
    ("SwirlToffeeSky", "Swirl", SKY, TOFFEE, "#5A3014", False),
    ("SwirlCocoaMint", "Swirl", MINT, COCOA, "#4A2A18", True),
    ("SwirlVanillaChocolate", "Swirl", CHOC_BG, VANILLA, "#C9A57A", True),
    ("SwirlCaramelPeach", "Swirl", PEACH, CARAMEL, "#7A4420", False),
    ("PuffCaramelCream", "Puff", CREAM_BG, CARAMEL, None, True),
    ("PuffCocoaPeach", "Puff", PEACH, COCOA, None, True),
    ("PuffChocolateSky", "Puff", SKY, CHOC, None, False),
    ("DropToffeeButter", "Drop", BUTTER, TOFFEE, None, True),
    ("DropChocolateMint", "Drop", MINT, CHOC, None, False),
    ("PuffCloudBlue", "Puff", BLUE, solid("#FFFFFF"), None, True),
]
for name, shape, bg, body, groove, spark in V:
    write(name, shape, bg, body, groove, spark)
