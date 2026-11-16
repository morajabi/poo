# Generates squat vanilla Poo variants (Puff face on a chocolate ground) as Icon Composer bundles:
#   python3 gen4.py <out-dir>
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
def layer(name, image, fill=None, dark=None, tinted=None, opacity=None):
    l = {"name": name, "image-name": image}
    s = []
    if fill: s += [{"value": fill}, {"appearance": "dark", "value": dark or fill}]
    if tinted: s.append({"appearance": "tinted", "value": tinted})
    if s: l["fill-specializations"] = s
    if opacity is not None: l["opacity"] = opacity
    return l

def sparkle(cx, cy, s, k=0.16):
    q = s*k
    return (f"M{cx},{cy-s} Q{cx+q},{cy-q} {cx+s},{cy} Q{cx+q},{cy+q} {cx},{cy+s} "
            f"Q{cx-q},{cy+q} {cx-s},{cy} Q{cx-q},{cy-q} {cx},{cy-s} Z")

# Puff's face, unchanged, split into layers so tinted mode can keep it legible.
EYES = ('<ellipse cx="438" cy="520" rx="34" ry="44"/><ellipse cx="590" cy="520" rx="34" ry="44"/>'
        '<path d="M474,606 Q514,640 554,606" stroke="#2B2350" stroke-width="20" stroke-linecap="round" fill="none"/>')
BLUSH = '<ellipse cx="380" cy="600" rx="40" ry="24"/><ellipse cx="648" cy="600" rx="40" ry="24"/>'
GLINTS = '<circle cx="450" cy="504" r="11"/><circle cx="602" cy="504" r="11"/>'
def place(body, cy, s):
    return f'<g transform="translate({C} {cy}) scale({s}) translate(-514 -560)">{body}</g>'

def capsule(cx, y, w, h, r=None):
    r = h/2 if r is None else r; x = cx - w/2
    return (f"M{x+r},{y} H{x+w-r} A{r},{r} 0 0 1 {x+w},{y+r} V{y+h-r} A{r},{r} 0 0 1 {x+w-r},{y+h} "
            f"H{x+r} A{r},{r} 0 0 1 {x},{y+h-r} V{y+r} A{r},{r} 0 0 1 {x+r},{y} Z")

def squircle(cx, cy, w, h, n=3.2, k=120):
    pts = []
    for i in range(k):
        t = 2*math.pi*i/k; c, s = math.cos(t), math.sin(t)
        pts.append((cx + w/2*math.copysign(abs(c)**(2/n), c), cy + h/2*math.copysign(abs(s)**(2/n), s)))
    return "M" + " L".join(f2(p) for p in pts) + " Z"

def dome(cx, ybase, w, h, foot=60):
    """Mochi-like: flat-ish rounded base, soft dome top."""
    x0, x1 = cx - w/2, cx + w/2
    return (f"M{x0+foot},{ybase} C{x0-10},{ybase} {x0},{ybase-h*0.55} {x0+w*0.12},{ybase-h*0.78} "
            f"C{x0+w*0.28},{ybase-h*1.02} {x1-w*0.28},{ybase-h*1.02} {x1-w*0.12},{ybase-h*0.78} "
            f"C{x1},{ybase-h*0.55} {x1+10},{ybase} {x1-foot},{ybase} Z")

def curl(x, y, s=1.0, lean=1.0):
    """Small soft curl whose base sits at (x, y), with a rounded tip leaning right."""
    def p(dx, dy): return f"{x+dx*s*lean:.1f},{y+dy*s:.1f}"
    return (f'<path d="M{p(-70,14)} C{p(-74,-50)} {p(-26,-88)} {p(28,-104)} '
            f'C{p(52,-111)} {p(70,-96)} {p(58,-80)} C{p(44,-62)} {p(70,-30)} {p(66,8)} '
            f'C{p(60,36)} {p(-64,42)} {p(-70,14)} Z"/>')

def groove(cx, y, w):
    return (f'<path d="M{cx-w/2+46},{y} Q{cx},{y+44} {cx+w/2-46},{y}" stroke="#000" stroke-width="22" '
            f'stroke-linecap="round" fill="none"/>')

# Each shape returns (body svg, grooves svg, face cy, face scale, sparkle center)
def tiers2():           # two chubby tiers and a small curl
    b = [(C, 520, 720, 330), (C, 360, 480, 220)]
    body = "".join(f'<path d="{capsule(*t)}"/>' for t in b) + curl(C, 380, 0.9)
    return body, groove(C, 558, 480), 690, 1.12, (812, 268)
def tiers2_dome():      # two tiers, soft dome top, no curl
    body = f'<path d="{capsule(C, 520, 720, 330)}"/><path d="{dome(C, 590, 480, 290, 80)}"/>'
    return body, groove(C, 558, 480), 690, 1.12, (812, 268)
def mochi():            # one round squat body with a tiny curl
    body = f'<path d="{squircle(C, 600, 700, 500)}"/>' + curl(C, 376, 0.8)
    return body, "", 620, 1.2, (812, 250)
def mochi_plain():      # mochi, no curl, a little swirl groove on top
    body = f'<path d="{squircle(C, 590, 720, 520)}"/>'
    sw = ('<path d="M420,410 Q512,370 604,410" stroke="#000" stroke-width="18" stroke-linecap="round" fill="none"/>'
          '<path d="M466,376 Q512,356 558,376" stroke="#000" stroke-width="16" stroke-linecap="round" fill="none"/>')
    return body, sw, 625, 1.2, (812, 250)
def tiers3_squat():     # three low tiers
    b = [(C, 560, 720, 290), (C, 420, 530, 190), (C, 320, 340, 150)]
    body = "".join(f'<path d="{capsule(*t)}"/>' for t in b) + curl(C, 336, 0.7)
    return body, groove(C, 588, 530) + groove(C, 448, 340), 712, 1.05, (816, 262)
def bun_bigface():      # two fat tiers, biggest face
    b = [(C, 470, 760, 380), (C, 320, 470, 220)]
    body = "".join(f'<path d="{capsule(*t)}"/>' for t in b) + curl(C, 338, 0.85)
    return body, groove(C, 518, 470), 670, 1.28, (820, 236)
def dumpling():         # bottom-heavy dumpling with a soft peak
    d = ("M200,800 C150,800 140,700 168,640 C214,540 330,470 420,430 C470,408 490,350 512,300 "
         "C534,350 554,408 604,430 C694,470 810,540 856,640 C884,700 874,800 824,800 Z")
    return f'<path d="{d}"/>', "", 650, 1.15, (812, 262)
def tiers2_lean():      # two tiers, curl lying over to the side
    b = [(C, 520, 720, 330), (C, 360, 480, 220)]
    body = "".join(f'<path d="{capsule(*t)}"/>' for t in b)
    body += '<path d="M560,372 C540,300 620,262 720,286 C690,300 676,330 690,366 C650,384 590,388 560,372 Z"/>'
    return body, groove(C, 558, 480), 690, 1.12, (240, 272)
def dollop():           # one wide tier, a round dollop on top
    body = f'<path d="{capsule(C, 470, 740, 360)}"/><circle cx="{C}" cy="420" r="150"/>' + curl(C, 290, 0.6)
    return body, groove(C, 492, 300), 655, 1.22, (816, 250)

VANILLA = grad("#FFFBF3", "#F3E2C8")
BG = {"Choc": grad("#8E5B3C", "#5A341F"), "Deep": grad("#6E4129", "#3A2013"),
      "Warm": grad("#A8693F", "#6E3B1E"), "Soft": grad("#86573B", "#6A4029")}
GROOVE = "#C9A57A"

def write(name, shape, bg="Choc", spark=True):
    body, grooves, fcy, fs, sp = shape()
    d = os.path.join(OUT, f"{name}.icon"); os.makedirs(os.path.join(d, "Assets"), exist_ok=True)
    assets = {"body.svg": f'<g fill="#fff">{body}</g>',
              "eyes.svg": place(f'<g fill="#2B2350">{EYES}</g>', fcy, fs),
              "blush.svg": place(f'<g fill="#FF8FB1">{BLUSH}</g>', fcy, fs),
              "glints.svg": place(f'<g fill="#fff">{GLINTS}</g>', fcy, fs)}
    groups = []
    if spark:
        assets["spark.svg"] = f'<path fill="#fff" d="{sparkle(*sp, 88)}"/><path fill="#fff" d="{sparkle(sp[0]-86 if sp[0] > C else sp[0]+86, sp[1]-82, 38)}"/>'
        groups.append(group("Spark", [layer("Spark", "spark.svg", solid("#FFFFFF"))], shadow=0.3))
    groups.append(group("Face", [
        layer("Glints", "glints.svg", solid("#FFFFFF"), tinted=solid("#FFFFFF")),
        layer("Eyes", "eyes.svg", solid("#2B2350"), tinted=solid("#000000")),
        layer("Blush", "blush.svg", tinted=solid("#7A7A7A"), opacity=0.8)], shadow=0.15))
    body_layers = []
    if grooves:
        assets["grooves.svg"] = grooves
        body_layers.append(layer("Grooves", "grooves.svg", solid(GROOVE), tinted=solid("#9A9A9A"), opacity=0.3))
    body_layers.append(layer("Body", "body.svg", VANILLA, tinted=solid("#FFFFFF")))
    groups.append(group("Body", body_layers, shadow=0.5, translucency=0.1))
    for fn, b in assets.items():
        open(os.path.join(d, "Assets", fn), "w").write(svg(b))
    doc = {"fill": BG[bg], "groups": groups, "supported-platforms": {"squares": ["macOS"]}}
    open(os.path.join(d, "icon.json"), "w").write(json.dumps(doc, indent=2) + "\n")

V = [("TwoTier", tiers2), ("TwoTierDome", tiers2_dome), ("Mochi", mochi), ("MochiPlain", mochi_plain),
     ("ThreeTierSquat", tiers3_squat), ("BunBigFace", bun_bigface), ("Dumpling", dumpling),
     ("TwoTierLean", tiers2_lean), ("Dollop", dollop)]
for n, f in V: write(n, f)
write("TwoTierNoSpark", tiers2, spark=False)
for bg in ("Deep", "Warm", "Soft"): write(f"TwoTier{bg}", tiers2, bg)
for bg in ("Deep", "Warm"): write(f"BunBigFace{bg}", bun_bigface, bg)
