"""Merge the per-repo result.json files into one markdown summary.

Exits 1 if any repo failed operationally (scant exit 2, an unrecognized report, or a job
that produced no result at all). Dependency-install failures are not counted as failures:
they are environmental, and scant now reports uninstallable packages as `unknown` rather
than `drop`, which is exactly the behaviour this run exists to confirm at scale.
"""

import json
import sys
from pathlib import Path

VERDICTS = ("drop", "inline", "registered", "unknown", "keep")


def load(results_dir, expected):
    rows = []
    for entry in expected:
        path = results_dir / f"{entry['id']}.json"
        if path.is_file():
            rows.append(json.loads(path.read_text()))
        else:
            rows.append({"id": entry["id"], "repo": entry["repo"], "status": "no-result"})
    return rows


def cell(row, key):
    return str(row[key]) if key in row else "--"


def main():
    results_dir = Path(sys.argv[1])
    expected = json.loads(Path(sys.argv[2]).read_text())
    rows = load(results_dir, expected)

    out = ["## scant field test", ""]
    version = next((r["scant_version"] for r in rows if r.get("scant_version")), "unknown")
    out += [f"scant `{version}` against {len(rows)} real repositories.", ""]
    out += ["| Repo | Declared | Installed | Files | drop | inline | registered | unknown | keep | Time | Status |"]
    out += ["|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|"]
    for row in rows:
        installed = f"{cell(row, 'installed')}/{cell(row, 'install_attempted')}" if row.get("install_attempted") else "--"
        seconds = f"{row['seconds']:.1f}s" if "seconds" in row else "--"
        status = {"ok": "ok", "error": "**operational error**", "unparsed": "**unparsed output**", "no-result": "**no result**"}.get(row.get("status"), row.get("status", "?"))
        out.append(
            f"| [{row['repo']}](https://github.com/{row['repo']}) | {cell(row, 'declared')} | {installed} | "
            f"{cell(row, 'files')} | " + " | ".join(cell(row, v) for v in VERDICTS) + f" | {seconds} | {status} |"
        )

    totals = {v: sum(row.get(v, 0) for row in rows) for v in VERDICTS}
    out += ["", "**Totals** -- " + ", ".join(f"{v} {n}" for v, n in totals.items()) + ".", ""]

    # The inline list is the whole reason this tool exists, so it gets named packages rather than a count
    out += ["### Inline candidates (the differentiator)", ""]
    for row in rows:
        names = row.get("inline_names") or []
        if names:
            out.append(f"- **{row['repo']}** -- " + ", ".join(f"`{n}`" for n in names))
    if not any(row.get("inline_names") for row in rows):
        out.append("_None found._")

    out += ["", "### Flagged to drop", ""]
    for row in rows:
        names = row.get("drop_names") or []
        if names:
            out.append(f"- **{row['repo']}** -- " + ", ".join(f"`{n}`" for n in names))
    if not any(row.get("drop_names") for row in rows):
        out.append("_None found._")

    notes = [(row["repo"], note) for row in rows for note in row.get("notes", [])]
    if notes:
        out += ["", "### Warnings reported by scant", ""] + [f"- **{repo}** -- {note}" for repo, note in notes]

    print("\n".join(out))

    broken = [row["id"] for row in rows if row.get("status") != "ok"]
    if broken:
        print(f"\n::error::field test failed for: {', '.join(broken)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
