# Changelog

All notable changes to `scant` are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [PEP 440](https://peps.python.org/pep-0440/).

## [Unreleased]

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
