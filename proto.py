#!/usr/bin/env python3
"""Prototype of the ballast algorithm: find unused + lightly-used deps.

Validates the design before porting to Rust. Uses stdlib ast (the Rust
version will use ruff_python_parser). Name resolution here is the
heuristic + override path (no venv available), which is exactly the
'approximate' fallback tier from the plan.
"""
import ast, sys, tomllib, re
from pathlib import Path
from collections import defaultdict

# dist -> import names. The curated override table for known offenders.
OVERRIDES = {
    "pillow": ["PIL"], "pyyaml": ["yaml"], "beautifulsoup4": ["bs4"],
    "scikit-learn": ["sklearn"], "python-dateutil": ["dateutil"],
    "opencv-python": ["cv2"], "python-dotenv": ["dotenv"], "attrs": ["attr", "attrs"],
    "setuptools": ["setuptools", "pkg_resources"], "msgpack-python": ["msgpack"],
    "typing-extensions": ["typing_extensions"], "pyzmq": ["zmq"],
    "configargparse": ["configargparse", "ConfigArgParse"],
    "geventhttpclient": ["geventhttpclient"], "musicbrainzngs": ["musicbrainzngs"],
    "pyacoustid": ["acoustid"], "python-mpd2": ["mpd"], "pyxdg": ["xdg"],
    "unidecode": ["unidecode"], "ghp-import": ["ghp_import"],
    "markdown-it-py": ["markdown_it"], "backports-datetime-fromisoformat": ["backports"],
    "docutils": ["docutils"], "feedgenerator": ["feedgenerator"],
    "watchfiles": ["watchfiles"], "blinker": ["blinker"],
}
STDLIB = set(sys.stdlib_module_names)

def normalize(name):
    return re.sub(r"[-_.]+", "-", name).lower()

def dist_to_imports(dist):
    n = normalize(dist)
    if n in OVERRIDES:
        return OVERRIDES[n]
    return [n.replace("-", "_")]

def parse_manifest(root):
    """Return {normalized_dist: origin} for directly declared deps."""
    deps = {}
    py = root / "pyproject.toml"
    if py.exists():
        data = tomllib.loads(py.read_text())
        proj = data.get("project", {})
        for d in proj.get("dependencies", []):
            deps[normalize(req_name(d))] = "dependencies"
        for extra, lst in proj.get("optional-dependencies", {}).items():
            for d in lst:
                deps.setdefault(normalize(req_name(d)), f"optional[{extra}]")
    return deps

def req_name(spec):
    """Strip version specifiers, extras, markers from a PEP 508 requirement."""
    s = spec.split(";")[0].strip()          # drop marker
    s = re.split(r"[<>=!~\[ (]", s)[0]      # drop version/extras
    return s.strip()

FIRST_PARTY_SKIP = {".git", "build", "dist", "__pycache__", ".venv", "venv",
                    "site-packages", "node_modules", ".tox", "test", "tests", "docs"}

def source_files(root):
    for p in root.rglob("*.py"):
        parts = set(p.parts)
        if parts & FIRST_PARTY_SKIP:
            continue
        if any(x.startswith("test_") for x in p.parts):
            continue
        yield p

def analyze(root):
    declared = parse_manifest(root)
    if not declared:
        return None
    # import name -> dist
    imp2dist = {}
    for dist in declared:
        for imp in dist_to_imports(dist):
            imp2dist[imp] = dist

    first_party = {p.name for p in root.iterdir() if p.is_dir() and (p / "__init__.py").exists()}

    # dist -> {files:set, symbols:set, lines:set of (file,lineno)}
    usage = defaultdict(lambda: {"files": set(), "symbols": set(), "lines": set()})
    parsed = skipped = 0

    for f in source_files(root):
        try:
            tree = ast.parse(f.read_text(encoding="utf-8", errors="replace"))
            parsed += 1
        except SyntaxError:
            skipped += 1
            continue

        # pass 1: collect bindings from imports
        bindings = {}   # local name -> (dist, symbol)
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                for a in node.names:
                    top = a.name.split(".")[0]
                    if top in STDLIB or top in first_party:
                        continue
                    dist = imp2dist.get(top)
                    if dist:
                        local = (a.asname or a.name).split(".")[0]
                        bindings[local] = (dist, top)
                        usage[dist]["files"].add(f)
                        usage[dist]["lines"].add((f, node.lineno))
            elif isinstance(node, ast.ImportFrom):
                if node.level:      # relative -> first party
                    continue
                top = (node.module or "").split(".")[0]
                if top in STDLIB or top in first_party:
                    continue
                dist = imp2dist.get(top)
                if dist:
                    for a in node.names:
                        local = a.asname or a.name
                        bindings[local] = (dist, a.name)
                        usage[dist]["symbols"].add(a.name)
                    usage[dist]["files"].add(f)
                    usage[dist]["lines"].add((f, node.lineno))

        # pass 2: count usage sites of those bindings
        for node in ast.walk(tree):
            if isinstance(node, ast.Name) and node.id in bindings:
                dist, sym = bindings[node.id]
                usage[dist]["lines"].add((f, node.lineno))
            elif isinstance(node, ast.Attribute):
                # foo.bar -> record which attribute of a module binding
                v = node.value
                if isinstance(v, ast.Name) and v.id in bindings:
                    dist, _ = bindings[v.id]
                    usage[dist]["symbols"].add(node.attr)
                    usage[dist]["lines"].add((f, node.lineno))

    return declared, usage, parsed, skipped

def report(name, root, max_lines=3, max_files=2, max_symbols=1):
    res = analyze(root)
    if not res:
        print(f"{name}: no PEP 621 manifest"); return
    declared, usage, parsed, skipped = res
    unused, light, healthy = [], [], 0
    for dist, origin in sorted(declared.items()):
        u = usage.get(dist)
        if not u or not u["files"]:
            unused.append((dist, origin))
        else:
            nl, nf, ns = len(u["lines"]), len(u["files"]), len(u["symbols"])
            if nl <= max_lines and nf <= max_files and ns <= max_symbols:
                light.append((dist, nf, ns, nl, sorted(u["lines"])[:3]))
            else:
                healthy += 1
    print(f"\n{'='*62}\n{name}  ({parsed} files parsed, {skipped} skipped)")
    print(f"  declared {len(declared)} | healthy {healthy} | unused {len(unused)} | light {len(light)}")
    if unused:
        print("  UNUSED:")
        for d, o in unused: print(f"    {d:<28} [{o}]")
    if light:
        print("  BARELY USED:")
        for d, nf, ns, nl, locs in light:
            where = ", ".join(f"{p.relative_to(root)}:{ln}" for p, ln in locs)
            print(f"    {d:<28} {nf}f · {ns}sym · {nl}ln   {where}")

if __name__ == "__main__":
    base = Path("/home/user/candidates")
    names = sys.argv[1:] or ["pelican", "mkdocs", "beets", "locust"]
    for name in names:
        report(name, base / name)
