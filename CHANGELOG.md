# Changelog

All notable changes to `scant` are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [PEP 440](https://peps.python.org/pep-0440/).

## [Unreleased]

### Added
- Marker-aware `inconclusive` verdict: a dependency gated to another platform or Python version (e.g. Superset's `waitress; sys_platform == "win32"`) can never be installed into the environment being scanned, so its absence from site-packages is not evidence of anything. These are now reported as `inconclusive` with the marker itself as the stated reason, rather than counted as `drop`. PEP 508 markers were previously parsed and discarded; they are now kept on each declared dependency and evaluated against the **environment scant was pointed at**, not the host running it -- a 3.9 virtualenv inspected from a 3.13 machine has to judge `python_version < "3.10"` as that virtualenv sees it. Where there is no interpreter to ask (a bare site-packages directory), the result is treated as unknown and the verdict is left unchanged; scant never infers "cannot install" from a failure to check. As with registration, a false marker only ever overrides `drop` -- platform-guarded code that does import the package still classifies normally, and a registered dependency stays `registered`, since registration is positive evidence where a false marker is only the absence of it. `inconclusive` is not a finding and does not affect the exit code. Verified against a real Apache Superset clone, where `waitress` moves out of `drop` with `only installs when sys_platform == 'win32'` as its stated reason, and no other verdict changes.

### Added
- Entry-point detection: a dependency that registers entry points in its `entry_points.txt` (a SQLAlchemy dialect, a `pytest11` plugin, a `console_scripts` command) is loaded by name at runtime rather than imported, so zero imports is expected rather than suspicious. These are now reported in their own `registered` group with the loading mechanism named in the `WHERE` column, instead of being flagged `drop`. Registration only ever overrides `drop` -- a registered package that *is* imported is still a fair `inline` candidate. Registered dependencies are not findings, so they no longer force exit code 1. Verified against a real changedetection.io clone, where `pytest-flask`, `pytest-mock`, `pytest-xdist`, and `jsonschema` moved out of `drop`, and against a real Apache Superset clone with all 165 declared dependencies installed, where `drop` falls from 78 to 19 with 50 reclassified as `registered` -- 36 of them `sqlalchemy.dialects`, the case this feature exists for.

### Changed
- The `ACTION` column is now sized to the widest verdict actually present rather than a fixed width, so reports without registered dependencies render exactly as before.

## [0.0.1a4]

### Added
- The not-found and ambiguous Python-environment errors now also check `$PATH` for a `python3`/`python` interpreter and, if found, list it as an explicit option (e.g. `scant . --env /usr/local/bin/python3`). Never used for auto-detection -- a system interpreter isn't tied to a specific project and could have unrelated packages installed -- but surfaced as a suggestion since installing straight into a container's system Python (no venv at all) is a common, real setup. Found via a real Apache Superset container run, where the ambiguous-env error listed two unrelated leftover venvs but gave no hint that the actual answer was the system Python.

## [0.0.1a3]

### Added
- ASCII-art logo and CI/PyPI-version/license status badges to `README.md`.

## [0.0.1a2]

### Added
- `--env` flag (replaces `--python`, kept as a backward-compatible alias): accepts a venv directory, a direct interpreter path, or a path found under `bin/`/`Scripts/`, resolved via a `sysconfig` subprocess call instead of guessing the `lib/pythonX.Y/site-packages` layout.
- Auto-detection now finds any `pyvenv.cfg`-marked directory (not just `.venv`/`venv`), and reports an explicit "ambiguous" outcome with a friendly, ASCII-tree formatted message when multiple candidates are found.

## [0.0.1a1]

### Fixed
- The cold-start warning ("did you mean to install them first?") now excludes `pip`/`setuptools`/`wheel` from its resolved-overlap check. Found against a real Apache Superset clone: Superset declares `pip` as one of its 165 dependencies, and every venv bundles `pip` by default, so that single incidental match silently defeated the warning and produced a confidently-wrong "all 165 dependencies unused" report instead of flagging the empty environment.

## [0.0.1a0]

First alpha. `scant` is a Rust CLI that scans a Python project and reports, per declared dependency, whether to **drop** it (never imported), **inline** it (used so little that carrying the dependency probably isn't worth it), or **keep** it.

### Added
- Manifest detection across `pyproject.toml`, `requirements.txt` (including pip-compile lockfile detection), `setup.cfg`, and `setup.py` (best-effort, static-only -- never executes the file).
- RECORD-based distribution-to-import-name resolution (e.g. `PyYAML` -> `yaml`, `pyyaml-env-tag` -> `yaml_env_tag`) instead of guessing from the package name.
- A single report table grouped by verdict (drop / inline / keep), with per-dependency usage counts and a `WHERE` column pointing at the exact file:line for lightly-used dependencies.
- Colored terminal output (auto-detects `NO_COLOR` and non-TTY destinations).
- Prebuilt wheels for Linux (x86_64/aarch64), macOS (universal2), and Windows (x86_64), published via PyPI trusted publishing.

### Validated against
- A real clone of [mkdocs/mkdocs](https://github.com/mkdocs/mkdocs): 15 declared dependencies, 3 flagged to drop (`babel`, `colorama`, `importlib-metadata`), 4 flagged to inline (`markupsafe`, `mergedeep`, `packaging`, `pyyaml-env-tag`), 8 kept.
