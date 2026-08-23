"""Turn one scant run into a result.json row for the field-test summary.

scant has no --format json yet (Phase 2, unbuilt), so this parses the human report.
It is deliberately strict: an unrecognized header or plan line becomes status "unparsed"
rather than a row of zeros, because a silently-empty row in the summary table would read
as "scant found nothing" when it actually means "the output shape changed".
"""

import json
import os
import re
import sys

ANSI = re.compile(r"\x1b\[[0-9;]*m")
HEADER = re.compile(r"^(?P<name>.+?) -- (?P<declared>\d+) packages declared, (?P<files>\d+) files read, (?P<seconds>[\d.]+)s$")
PLAN = re.compile(r"^Plan: drop (?P<drop>\d+), inline (?P<inline>\d+)(?:, registered (?P<registered>\d+))?(?:, unknown (?P<unknown>\d+))?, keep (?P<keep>\d+)\.$")
ROW = re.compile(r"^ {2}(?P<verdict>drop|inline|registered|unknown|keep)\s+(?P<name>\S+)\s+(?P<uses>\d+)\s+(?P<lines>\d+)\s+(?P<band>\S+)\s*(?P<where>.*?)\s*$")


def main():
    text = ANSI.sub("", sys.stdin.read())
    result = {
        "id": os.environ["FT_ID"],
        "repo": os.environ["FT_REPO"],
        "exit_code": int(os.environ["FT_EXIT"]),
        "install_mode": os.environ.get("FT_INSTALL_MODE", ""),
        "installed": int(os.environ.get("FT_INSTALLED", "0")),
        "install_attempted": int(os.environ.get("FT_ATTEMPTED", "0")),
        "scant_version": os.environ.get("FT_SCANT_VERSION", ""),
        "status": "ok",
    }

    for line in text.splitlines():
        if m := HEADER.match(line):
            result["declared"] = int(m["declared"])
            result["files"] = int(m["files"])
            result["seconds"] = float(m["seconds"])
        elif m := PLAN.match(line):
            for verdict in ("drop", "inline", "registered", "unknown", "keep"):
                result[verdict] = int(m[verdict] or 0)

    rows = [m.groupdict() for m in (ROW.match(line) for line in text.splitlines()) if m]
    result["inline_names"] = [r["name"] for r in rows if r["verdict"] == "inline"]
    result["drop_names"] = [r["name"] for r in rows if r["verdict"] == "drop"]
    result["notes"] = [line.removeprefix("NOTE -- ") for line in text.splitlines() if line.startswith("NOTE -- ")]

    # exit 2 is scant's operational-error code -- a genuine failure, unlike 0/1 which just mean "ran fine"
    if result["exit_code"] > 1:
        result["status"] = "error"
    elif "declared" not in result or "drop" not in result:
        result["status"] = "unparsed"

    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
