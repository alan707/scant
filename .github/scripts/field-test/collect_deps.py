"""Print one PEP 508 requirement per line for a project, mirroring what scant itself reads.

Only used by the field test's fallback installer: `uv sync`/`uv pip install -r` resolve
atomically, so one unbuildable package (mysqlclient without MySQL headers) aborts all of
them and leaves scant with an empty environment. Falling back to a package-at-a-time loop
trades resolution correctness for coverage, which is the right trade when the goal is
"resolve as many import names as possible", not "produce a working app".
"""

import sys
import tomllib
from pathlib import Path


def from_pyproject(path):
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    project = data.get("project", {})
    reqs = list(project.get("dependencies", []))
    # scant unions every optional-dependencies group into the direct set, so the field test installs the same set it will be judged against
    for group in project.get("optional-dependencies", {}).values():
        reqs.extend(group)
    return reqs


def from_requirements(path):
    reqs = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.split("#", 1)[0].strip()
        # -r/-c includes and bare flags are installer directives, not requirements
        if not line or line.startswith("-"):
            continue
        reqs.append(line)
    return reqs


def main():
    root = Path(sys.argv[1])
    pyproject = root / "pyproject.toml"
    requirements = root / "requirements.txt"
    if pyproject.is_file():
        reqs = from_pyproject(pyproject)
        if reqs:
            print("\n".join(reqs))
            return
    if requirements.is_file():
        print("\n".join(from_requirements(requirements)))


if __name__ == "__main__":
    main()
