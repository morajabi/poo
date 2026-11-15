# Generates the three icon concepts as Icon Composer bundles: python3 generate.py <out-dir>
import json, math, os, sys
OUT = sys.argv[1]
C = 512

def svg(body):
    return f'<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">{body}</svg>\n'

def pt(r, a):  # a in degrees, 0 = up, clockwise
    t = math.radians(a)
    return C + r*math.sin(t), C - r*math.cos(t)

def arc(r, a1, a2, w):
    """Filled path for an arc stroke of width w with round caps, clockwise a1->a2."""
    ro, ri = r + w/2, r - w/2
    large = 1 if (a2 - a1) % 360 > 180 else 0
    o1, o2 = pt(ro, a1), pt(ro, a2)
    i2, i1 = pt(ri, a2), pt(ri, a1)
    c = w/2
    f = lambda p: f"{p[0]:.2f},{p[1]:.2f}"
    return (f"M{f(o1)} A{ro},{ro} 0 {large} 1 {f(o2)} A{c},{c} 0 0 1 {f(i2)} "
            f"A{ri},{ri} 0 {large} 0 {f(i1)} A{c},{c} 0 0 1 {f(o1)} Z")

def sparkle(cx, cy, s, k=0.16):
    q = s*k
    return (f"M{cx},{cy-s} Q{cx+q},{cy-q} {cx+s},{cy} Q{cx+q},{cy+q} {cx},{cy+s} "
            f"Q{cx-q},{cy+q} {cx-s},{cy} Q{cx-q},{cy-q} {cx},{cy-s} Z")

def capsule(x, y, w, h):
    r = h/2
    return f"M{x+r},{y} H{x+w-r} A{r},{r} 0 0 1 {x+w-r},{y+h} H{x+r} A{r},{r} 0 0 1 {x+r},{y} Z"

def color(r, g, b, a=1.0):
    return f"srgb:{r:.5f},{g:.5f},{b:.5f},{a:.5f}"
def hexc(h, a=1.0):
    h = h.lstrip('#'); return color(*(int(h[i:i+2], 16)/255 for i in (0, 2, 4)), a)

WHITE = {"solid": color(1, 1, 1)}
def fills(light, dark=None, tinted=WHITE):
    s = [{"value": light}]
    if dark: s.append({"appearance": "dark", "value": dark})
    s.append({"appearance": "tinted", "value": tinted})
    return s

def group(name, layers, shadow=0.5, translucency=0.0, specular=True, blur=None):
    g = {"name": name, "layers": layers,
         "shadow": {"kind": "neutral", "opacity": shadow},
         "translucency": {"enabled": translucency > 0, "value": translucency},
         "specular": specular}
    if blur is not None: g["blur-material"] = blur
    return g

def layer(name, image, fill=None, opacity=None):
    l = {"name": name, "image-name": image}
    if fill: l["fill-specializations"] = fill
    if opacity is not None: l["opacity"] = opacity
    return l

def write(name, bg, groups, assets):
    d = os.path.join(OUT, f"{name}.icon"); os.makedirs(os.path.join(d, "Assets"), exist_ok=True)
    for fn, body in assets.items():
        open(os.path.join(d, "Assets", fn), "w").write(svg(body))
    doc = {"fill": bg, "groups": groups, "supported-platforms": {"squares": ["macOS"]}}
    open(os.path.join(d, "icon.json"), "w").write(json.dumps(doc, indent=2) + "\n")

# 1. Platter: a disk platter with a clean sweep arc and a spark of "clean".
write("Platter",
      {"linear-gradient": [hexc("#2BC4B4"), hexc("#0B7A8A")]},
      [group("Spark", [layer("Spark", "spark.svg", fills({"solid": color(1, 1, 1)}))], shadow=0.35),
       group("Sweep", [layer("Sweep", "sweep.svg", fills({"solid": color(1, 1, 1)}))], shadow=0.45),
       group("Platter", [layer("Platter", "platter.svg", fills({"solid": color(1, 1, 1)}))],
             shadow=0.3, translucency=0.55)],
      {"platter.svg": f'<path fill="#fff" fill-rule="evenodd" d="M{C},{C-318} a318,318 0 1 1 0,636 a318,318 0 1 1 0,-636 Z M{C},{C-54} a54,54 0 1 0 0,108 a54,54 0 1 0 0,-108 Z"/>',
       "sweep.svg": f'<path fill="#fff" d="{arc(204, -128, 52, 78)}"/>',
       "spark.svg": f'<path fill="#fff" d="{sparkle(752, 270, 112)}"/>'})

# 2. Sweep: three receding sweep lines and a spark.
ys = [372, 488, 604]; h = 84
write("Sweep",
      {"linear-gradient": [hexc("#FFB547"), hexc("#F2741B")]},
      [group("Spark", [layer("Spark", "spark.svg", fills({"solid": color(1, 1, 1)}))], shadow=0.35),
       group("Lines", [layer("Line 1", "line1.svg", fills({"solid": color(1, 1, 1)})),
                       layer("Line 2", "line2.svg", fills({"solid": color(1, 1, 1)}), opacity=0.8),
                       layer("Line 3", "line3.svg", fills({"solid": color(1, 1, 1)}), opacity=0.6)],
             shadow=0.45, translucency=0.15)],
      {"line1.svg": f'<path fill="#fff" d="{capsule(222, ys[0], 520, h)}"/>',
       "line2.svg": f'<path fill="#fff" d="{capsule(222, ys[1], 400, h)}"/>',
       "line3.svg": f'<path fill="#fff" d="{capsule(222, ys[2], 280, h)}"/>',
       "spark.svg": f'<path fill="#fff" d="{sparkle(704, 650, 110)}"/>'})

# 3. Gauge: a free-space gauge, the filled part bright, the track glassy.
write("Gauge",
      {"linear-gradient": [hexc("#4CD67A"), hexc("#16904A")]},
      [group("Spark", [layer("Spark", "spark.svg", fills({"solid": color(1, 1, 1)}))], shadow=0.35),
       group("Level", [layer("Level", "level.svg", fills({"solid": color(1, 1, 1)}))], shadow=0.45),
       group("Track", [layer("Track", "track.svg", fills({"solid": color(1, 1, 1)}))],
             shadow=0.2, translucency=0.6)],
      {"track.svg": f'<path fill="#fff" d="{arc(270, -135, 135, 104)}"/>',
       "level.svg": f'<path fill="#fff" d="{arc(270, -135, 60, 104)}"/>',
       "spark.svg": f'<path fill="#fff" d="{sparkle(C, C+10, 120)}"/>'})
