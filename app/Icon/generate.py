# Generates Poo icon concepts (round 2) as Icon Composer bundles: python3 gen2.py <out-dir>
import json, math, os, sys
OUT = sys.argv[1]
C = 512

def svg(body, defs=""):
    d = f"<defs>{defs}</defs>" if defs else ""
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">{d}{body}</svg>\n'

def color(r, g, b, a=1.0): return f"srgb:{r:.5f},{g:.5f},{b:.5f},{a:.5f}"
def hexc(h, a=1.0):
    h = h.lstrip('#'); return color(*(int(h[i:i+2], 16)/255 for i in (0, 2, 4)), a)
def grad(a, b): return {"linear-gradient": [hexc(a), hexc(b)]}

def group(name, layers, shadow=0.5, translucency=0.0, specular=True, blur=None, opacity=None):
    g = {"name": name, "layers": layers, "shadow": {"kind": "neutral", "opacity": shadow},
         "translucency": {"enabled": translucency > 0, "value": translucency}, "specular": specular}
    if blur is not None: g["blur-material"] = blur
    if opacity is not None: g["opacity"] = opacity
    return g

def layer(name, image, fill=None, dark=None, tinted=None, opacity=None):
    l = {"name": name, "image-name": image}
    if fill:
        s = [{"value": fill}]
        if dark: s.append({"appearance": "dark", "value": dark})
        if tinted: s.append({"appearance": "tinted", "value": tinted})
        l["fill-specializations"] = s
    if opacity is not None: l["opacity"] = opacity
    return l

def write(name, bg, groups, assets, bg_dark=None):
    d = os.path.join(OUT, f"{name}.icon"); os.makedirs(os.path.join(d, "Assets"), exist_ok=True)
    for fn, body in assets.items():
        open(os.path.join(d, "Assets", fn), "w").write(body if body.startswith("<svg") else svg(body))
    doc = {"fill": bg, "groups": groups, "supported-platforms": {"squares": ["macOS"]}}
    if bg_dark: doc["fill-specializations"] = [{"value": bg}, {"appearance": "dark", "value": bg_dark}]; del doc["fill"]
    open(os.path.join(d, "icon.json"), "w").write(json.dumps(doc, indent=2) + "\n")

def f2(p): return f"{p[0]:.1f},{p[1]:.1f}"
def poly(pts): return "M" + " L".join(f2(p) for p in pts) + " Z"

def rrect(cx, cy, s, r, rot):
    """Rounded square centered at cx,cy, side s, corner r, rotated rot degrees."""
    return (f'<rect x="{cx-s/2:.1f}" y="{cy-s/2:.1f}" width="{s:.1f}" height="{s:.1f}" rx="{r:.1f}" '
            f'transform="rotate({rot:.1f} {cx:.1f} {cy:.1f})"/>')

def sparkle(cx, cy, s, k=0.16):
    q = s*k
    return (f"M{cx},{cy-s} Q{cx+q},{cy-q} {cx+s},{cy} Q{cx+q},{cy+q} {cx},{cy+s} "
            f"Q{cx-q},{cy+q} {cx-s},{cy} Q{cx-q},{cy-q} {cx},{cy-s} Z")

# ---------- 1. Flush: a glass vortex pulling build blocks in ----------
SWEEP = 0
def spiral_arm(a0, R=300, turns=0.62, w0=138, n=90):
    left, right = [], []
    for i in range(n + 1):
        t = i / n
        th = math.radians(a0) + t * turns * 2 * math.pi
        r = R * (1 - 0.86 * t)
        w = w0 * (1 - t) ** 0.9 + 6
        x, y = C + r * math.cos(th), C + r * math.sin(th)
        # tangent
        th2 = th + 0.01; r2 = R * (1 - 0.86 * (t + 0.01 / (turns * 2 * math.pi)))
        dx, dy = C + r2 * math.cos(th2) - x, C + r2 * math.sin(th2) - y
        L = math.hypot(dx, dy); nx, ny = -dy / L, dx / L
        left.append((x + nx * w / 2, y + ny * w / 2)); right.append((x - nx * w / 2, y - ny * w / 2))
    # round outer end
    c = (w0 + 6) / 2
    return ("M" + " L".join(f2(p) for p in left) + " L" + " L".join(f2(p) for p in reversed(right))
            + f" A{c:.1f},{c:.1f} 0 0 {SWEEP} {f2(left[0])} Z")

arms = "".join(f'<path d="{spiral_arm(a)}"/>' for a in (0, 120, 240))
cubes_spec = [  # (angle deg, radius, size, color): build blocks tumbling in through the gaps
    (58, 372, 84, "#FFB23F"), (178, 380, 72, "#FF6B8A"), (298, 372, 66, "#4FE0A8"),
    (100, 300, 40, "#FFD84D"), (220, 302, 36, "#7FC8FF"), (340, 300, 32, "#FF9E5E")]
cubes = "".join(f'<g fill="{c}">{rrect(C+r*math.cos(math.radians(a)), C+r*math.sin(math.radians(a)), s, s*0.24, a*0.7)}</g>'
                for a, r, s, c in cubes_spec)
write("Flush", grad("#6E6CFF", "#3420B8"),
      [group("Blocks", [layer("Blocks", "blocks.svg")], shadow=0.45),
       group("Vortex", [layer("Vortex", "vortex.svg", fill={"solid": color(1, 1, 1)}, dark={"linear-gradient": [hexc("#E4E1FF"), hexc("#A9A2FF")]})], shadow=0.3, translucency=0.35)],
      {"vortex.svg": f'<g fill="#fff">{arms}<circle cx="{C}" cy="{C}" r="36"/></g>', "blocks.svg": cubes})

# ---------- 2. Beetle: a dung beetle rolling a ball of build junk ----------
ball_c, ball_r = (418, 548), 250
bits = []
import random
random.seed(7)
pal = ["#FF8A3D", "#FFC93C", "#FF5E7E", "#3FD0A4", "#6AA8FF", "#B78CFF"]
k = 0
for gy in range(-4, 5):
    for gx in range(-4, 5):
        x = ball_c[0] + gx * 58 + (gy % 2) * 29; y = ball_c[1] + gy * 54
        if math.hypot(x - ball_c[0], y - ball_c[1]) < ball_r - 40:
            bits.append(f'<g fill="{pal[k % len(pal)]}">{rrect(x, y, 40, 10, random.uniform(-25, 25))}</g>'); k += 3
ball = f'<circle cx="{ball_c[0]}" cy="{ball_c[1]}" r="{ball_r}" fill="#fff"/>'
# beetle: head down at right, rear up against the ball, hind legs on the ball
bx, by, rot = 742, 664, -38
beetle_body = (
    f'<g transform="rotate({rot} {bx} {by})" fill="#1D2B3A">'
    f'<ellipse cx="{bx}" cy="{by}" rx="142" ry="102"/>'          # elytra
    f'<path d="M{bx+118},{by-60} Q{bx+190},{by-62} {bx+196},{by} Q{bx+190},{by+62} {bx+118},{by+60} Z"/>'  # pronotum
    f'<path d="M{bx+186},{by-34} Q{bx+250},{by-30} {bx+252},{by} Q{bx+250},{by+30} {bx+186},{by+34} Z"/>'  # head
    f'</g>')
legs = (f'<g stroke="#1D2B3A" stroke-width="26" stroke-linecap="round" stroke-linejoin="round" fill="none">'
        f'<path d="M640,640 L600,560 L638,470"/>'      # hind leg up on ball
        f'<path d="M668,700 L606,742 L596,820"/>'      # mid leg down
        f'<path d="M790,760 L760,830 L800,858"/>'      # front leg to ground
        f'<path d="M860,708 L900,790 L944,808"/>'
        f'</g>')
shine = (f'<g transform="rotate({rot} {bx} {by})"><path d="M{bx-96},{by-40} Q{bx-20},{by-92} {bx+70},{by-62}" '
         f'stroke="#fff" stroke-opacity="0.5" stroke-width="18" stroke-linecap="round" fill="none"/>'
         f'<line x1="{bx-142}" y1="{by}" x2="{bx+118}" y2="{by}" stroke="#0E1620" stroke-width="8"/></g>')
write("Beetle", grad("#FFE08A", "#FFA33A"),
      [group("Beetle", [layer("Beetle", "beetle.svg")], shadow=0.6),
       group("Bits", [layer("Bits", "bits.svg")], shadow=0.2),
       group("Ball", [layer("Ball", "ball.svg", fill={"solid": color(1, 1, 1)})], shadow=0.5, translucency=0.45)],
      {"ball.svg": ball, "bits.svg": "".join(bits), "beetle.svg": legs + beetle_body + shine})

# ---------- 3. Puff: a fluffy dust bunny mascot, pleased with itself ----------
def fluff(cx, cy, R, bumps=14, amp=26):
    pts = []
    for i in range(bumps):
        a0 = 2 * math.pi * i / bumps; a1 = 2 * math.pi * (i + 1) / bumps; am = (a0 + a1) / 2
        p0 = (cx + R * math.cos(a0), cy + R * math.sin(a0))
        pm = (cx + (R + amp * 2.2) * math.cos(am), cy + (R + amp * 2.2) * math.sin(am))
        pts.append((p0, pm))
    d = f"M{f2(pts[0][0])}"
    for i, (p0, pm) in enumerate(pts):
        p1 = pts[(i + 1) % bumps][0]
        d += f" Q{f2(pm)} {f2(p1)}"
    return d + " Z"
face = ('<g fill="#2B2350">'
        '<ellipse cx="438" cy="520" rx="34" ry="44"/><ellipse cx="590" cy="520" rx="34" ry="44"/>'
        '</g>'
        '<path d="M474,606 Q514,640 554,606" stroke="#2B2350" stroke-width="20" stroke-linecap="round" fill="none"/>'
        '<g fill="#FF8FB1" opacity="0.8"><ellipse cx="380" cy="600" rx="40" ry="24"/><ellipse cx="648" cy="600" rx="40" ry="24"/></g>'
        '<g fill="#fff"><circle cx="450" cy="504" r="11"/><circle cx="602" cy="504" r="11"/></g>')
write("Puff", grad("#7FD6FF", "#3D7BFF"),
      [group("Spark", [layer("Spark", "spark.svg", fill={"solid": color(1, 1, 1)})], shadow=0.3),
       group("Face", [layer("Face", "face.svg")], shadow=0.15),
       group("Puff", [layer("Puff", "puff.svg", fill={"solid": color(1, 1, 1)})], shadow=0.5, translucency=0.15)],
      {"puff.svg": f'<path fill="#fff" d="{fluff(C, 556, 262)}"/>', "face.svg": face,
       "spark.svg": f'<path fill="#fff" d="{sparkle(790, 236, 92)}"/><path fill="#fff" d="{sparkle(700, 150, 40)}"/>'})

# ---------- 4. Broom: a broom flicking build blocks off the edge ----------
def broom():
    # handle from top-right to the bristles, bristles a fan at lower-left
    handle = '<path d="M792,176 L560,520" stroke="#8A4B2A" stroke-width="44" stroke-linecap="round"/>'
    band = '<g transform="rotate(34 560 520)"><rect x="470" y="490" width="180" height="70" rx="22" fill="#E2473B"/></g>'
    br = []
    for i in range(9):
        a = math.radians(124 + i * 7.5)
        x0, y0 = 560 + 30 * math.cos(a - 0.3), 560 + 30 * math.sin(a - 0.3)
        L = 300 - abs(i - 4) * 14
        x1, y1 = 560 + L * math.cos(a), 560 + L * math.sin(a)
        br.append(f'<path d="M{x0:.0f},{y0:.0f} L{x1:.0f},{y1:.0f}" stroke="#FFC745" stroke-width="40" stroke-linecap="round"/>')
    return handle + "".join(br) + band
flick = [(300, 300, 70, "#FF6B8A", -20), (200, 430, 54, "#5BE3B0", 25), (226, 260, 40, "#7FC8FF", 10), (150, 330, 30, "#FFD84D", -30)]
write("Broom", grad("#3FE0C0", "#109C8C"),
      [group("Bits", [layer("Bits", "bits.svg")], shadow=0.4),
       group("Broom", [layer("Broom", "broom.svg")], shadow=0.55)],
      {"broom.svg": broom(), "bits.svg": "".join(f'<g fill="{c}">{rrect(x, y, s, s*0.25, r)}</g>' for x, y, s, c, r in flick)})
