"""Assemble one page of evidence: scant's own output, verbatim, for every repo in the run.

Deliberately not a summary table. The point of the field test is to show that scant runs on
real projects across a wide size range, and scant's own header line already states the size
and the time ("mlflow -- 62 packages declared, 2630 files read, 0.4s"). Re-deriving those
numbers into a table of our own would be weaker evidence than the thing itself, so the only
framing here is the repo name, how much of the manifest actually installed, and the order:
smallest tree first, so the range reads as a ladder.

Exits 1 if any repo failed operationally (scant exit 2, an unrecognized report, or a job that
produced no result at all). Dependency-install failures are not counted as failures: they are
environmental, and scant now reports uninstallable packages as `unknown` rather than `drop`.
"""

import json
import sys
from pathlib import Path


def load(results_dir, expected):
    rows = []
    for entry in expected:
        path = results_dir / f"{entry['id']}.json"
        if path.is_file():
            row = json.loads(path.read_text())
        else:
            row = {"id": entry["id"], "repo": entry["repo"], "status": "no-result"}
        report = results_dir / f"{entry['id']}.txt"
        row["report"] = report.read_text().rstrip() if report.is_file() else "(no output captured)"
        rows.append(row)
    return rows


def main():
    results_dir = Path(sys.argv[1])
    expected = json.loads(Path(sys.argv[2]).read_text())
    rows = load(results_dir, expected)
    # Smallest tree first; anything that never produced a report sorts last
    rows.sort(key=lambda r: r.get("files", float("inf")))

    version = next((r["scant_version"] for r in rows if r.get("scant_version")), "scant")
    noun = "project" if len(rows) == 1 else "projects"
    out = ["## scant field test", "", f"`{version}` against {len(rows)} real Python {noun}, smallest tree first.", ""]

    for row in rows:
        out.append(f"### [{row['repo']}](https://github.com/{row['repo']})")
        if row.get("install_attempted"):
            out.append(f"_{row['installed']} of {row['install_attempted']} declared packages installed via `{row['install_mode']}`._")
        if row.get("status") != "ok":
            out.append(f"**Failed: {row['status']}.**")
        out += ["", "```", row["report"], "```", ""]

    print("\n".join(out))

    broken = [row["id"] for row in rows if row.get("status") != "ok"]
    if broken:
        print(f"\n::error::field test failed for: {', '.join(broken)}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
