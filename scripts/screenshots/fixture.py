# Builds a neutral fixture report for README screenshots: made-up projects, realistic sizes.
# Usage: gen.py <template report.json (for bucket and plan definitions)> <out report.json>
import json, os, time, sys
TEMPLATE = sys.argv[1]; OUT = sys.argv[2]
H = os.path.expanduser("~"); now = int(time.time()); G = 1_000_000_000
tpl = json.load(open(TEMPLATE))
d = lambda days: now - int(days * 86400)
P = lambda p: f"{H}/dev/{p}"
items = []
def add(bucket, group, name, path, gb, tier, cost, days, reasons, rebuild=None, project=None, kind="delete", agent=None, **kw):
    b = int(gb * G)
    it = dict(id=f"fx{len(items)}", path=path, name=name, rule=f"{bucket}.{group.lower().replace(' ','-')}", bucket=bucket,
              group=group, bytes=b, total_bytes=b, files=int(gb * 9000) + 12, last_used=d(days), tier=tier, cost=cost,
              reasons=reasons, action={"kind": kind}, safeguards=[])
    if rebuild: it["rebuild"] = rebuild
    if project: it["project"] = project
    if agent: it["agent"] = agent
    it.update(kw); items.append(it)
# Not in use
add("build","Rust","atlas-api/target",P("atlas-api/target"),6.8,"cold","rebuild",19,["last used 19d ago (cold after 14d)"],"cargo build",P("atlas-api"))
add("build","Swift","orbit-ios/.build",P("orbit-ios/.build"),3.9,"idle","rebuild",4,["last used 4d ago, nobody working there"],"swift build",P("orbit-ios"))
add("build","Next.js","web-dash/.next",P("web-dash/.next"),1.2,"cold","rebuild",23,["last used 23d ago (cold after 14d)"],"next build",P("web-dash"))
add("xcode","DerivedData","Orbit",f"{H}/Library/Developer/Xcode/DerivedData/Orbit-gxkqbzt",8.4,"cold","rebuild",17,["project last built 17d ago"],"Build in Xcode or xcodebuild",P("orbit-ios"))
add("xcode","Simulators","iPhone 15 (iOS 17.5)","sim://4F1C",3.1,"cold","rebuild",61,["not booted in 2 months"],"Create a new simulator")
add("deps","Node","web-dash/node_modules",P("web-dash/node_modules"),0.94,"idle","redownload",23,["nobody working there"],"npm install",P("web-dash"))
add("caches","Rust","cargo registry",f"{H}/.cargo/registry",2.2,"idle","redownload",5,["no cargo build in 5d"],"cargo fetch")
add("worktrees","Worktrees","atlas-api-search",P("atlas-api-search"),1.4,"cold","rebuild",9,["work landed in main 9d ago"],"git worktree add",P("atlas-api-search"),kind="worktree-remove",agent="claude",repo=P("atlas-api"))
add("junk","Logs","Diagnostic reports",f"{H}/Library/Logs/DiagnosticReports",0.42,"junk","none",2,["crash logs older than a week"])
# In use and check first
add("build","Rust","ledger/target",P("ledger/target"),4.1,"warm","rebuild",0.2,["built 5h ago"],"cargo build",P("ledger"))
add("xcode","Simulator runtimes","iOS 18.2",f"/Library/Developer/CoreSimulator/Volumes/iOS_22C150",7.9,"warm","redownload",1,["used by 2 simulators"])
add("caches","Homebrew","Homebrew downloads",f"{H}/Library/Caches/Homebrew",1.6,"warm","redownload",2,["last brew install 2d ago"])
add("ai","Local models","ollama models",f"{H}/.ollama/models",12.4,"hot","redownload",0.02,["ollama is running"])
add("containers","Docker","Docker build cache","docker://build-cache",5.2,"warm","redownload",1,["built yesterday"])
add("deps","Node","ledger-ui/node_modules",P("ledger-ui/node_modules"),1.1,"hot","redownload",0.01,["dev server running"],"npm install",P("ledger-ui"))
add("other","Unclassified ignored","old-site/vendor",P("old-site/vendor"),2.3,"review","data",140,["ignored by git, no rule knows it"],project=P("old-site"))
# Report-only
add("system","Device backups","iPhone backups",f"{H}/Library/Application Support/MobileSync/Backup",18.6,"review","data",3,["2 device backups, newest 3 days ago"],report_only=True,manage_url="x-apple.systempreferences:com.apple.settings.Storage",action={"kind":"none"})
status = {"junk":"n","cold":"n","idle":"n","warm":"u","hot":"u","review":"r","protected":"r"}
bmeta = {b["id"]: b for b in tpl["buckets"]}
buckets = []
for bid, b in bmeta.items():
    its = [i for i in items if i["bucket"] == bid and not i.get("report_only")]
    nb = dict(b); nb["bytes"] = sum(i["bytes"] for i in its); nb["count"] = len(its)
    nb["tiers"] = {}; groups = {}
    for i in its:
        nb["tiers"][i["tier"]] = nb["tiers"].get(i["tier"], 0) + i["bytes"]
        g = groups.setdefault(i["group"], {"name": i["group"], "bytes": 0, "count": 0}); g["bytes"] += i["bytes"]; g["count"] += 1
    nb["groups"] = sorted(groups.values(), key=lambda g: -g["bytes"]); buckets.append(nb)
plans = []
for p in tpl["plans"]:
    tiers = {"quick": ["junk"], "stale": ["junk","cold","idle"]}.get(p["id"], ["junk","cold","idle","warm"])
    sel = [i for i in items if i["tier"] in tiers and not i.get("report_only")]
    q = dict(p); q["bytes"] = sum(i["bytes"] for i in sel); q["count"] = len(sel); plans.append(q)
projects = []
for name, kind, stacks, days, status_ in [("atlas-api","repo",["rust"],19,"stale"),("orbit-ios","repo",["swift"],4,"idle"),("web-dash","repo",["node","next"],23,"stale"),
                                           ("ledger","repo",["rust"],0.2,"active"),("ledger-ui","repo",["node"],0.01,"active"),("atlas-api-search","worktree",["rust"],9,"idle"),("old-site","repo",["php"],140,"stale")]:
    path = P(name); its = [i for i in items if i.get("project") == path]
    pr = dict(path=path, name=name, kind=kind, stacks=stacks, last_activity=d(days), artifact_bytes=sum(i["bytes"] for i in its),
              item_count=len(its), policy="auto", status=status_)
    if kind == "worktree": pr.update(repo=P("atlas-api"), agent="claude", branch="feature/search", merged=True, dirty=False)
    else: pr.update(repo=path, branch="main")
    projects.append(pr)
total = 494_330_000_000; free = 86_000_000_000
r = dict(schema=tpl["schema"], version=tpl["version"], scanned_at=now, duration_ms=840, disk=dict(total=total, free=free, used=total-free),
         roots=[f"{H}/dev"], total_bytes=sum(i["bytes"] for i in items if not i.get("report_only")), buckets=buckets, plans=plans,
         projects=projects, items=items, warnings=[], detected=["rust","swift","node","xcode"],
         growth=dict(day_bytes=2_100_000_000, week_bytes=6_800_000_000), snapshots=dict(count=2, bytes=9_400_000_000))
json.dump(r, open(OUT, "w"))
for p in plans:
    if p["id"] == "stale":
        sel = [i for i in items if i["tier"] in ("junk","cold","idle") and not i.get("report_only")]
        json.dump(dict(plan="stale", name=p["name"], description=p["description"], bytes=p["bytes"], count=p["count"], items=sel), open(OUT.replace("report.json","plan-stale.json"), "w"))
print(len(items), "items")
