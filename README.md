![scant](scant-logo.png)

[![CI](https://github.com/alan707/scant/actions/workflows/ci.yml/badge.svg)](https://github.com/alan707/scant/actions/workflows/ci.yml)
[![image](https://img.shields.io/pypi/v/scant.svg)](https://pypi.org/project/scant/)
[![image](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Find the Python dependencies you barely use.

Most tools tell you what's unused. `scant` also tells you what's barely used. Think of those dependencies pulled in for one function, called on a couple of lines, that might be stopping you from your next Python upgrade (true story).

Built for plain English: flags are spelled out in full (`--threshold-lines`, not `-L`), and every result or error reads in words a non-developer can follow. The idea is to continue making the CLI very nice to look at. Like candy for your eyes.

Written in rust ⚡️

## Install

    pipx install scant

## Usage

    scant .
    scant . --threshold-lines 5
    scant . --env path/to/venv
    scant . --env path/to/venv/bin/python

A dependency is "barely used" when it stays under *all three* thresholds:
`--threshold-lines` (default 3), `--threshold-files` (2), and
`--threshold-symbols` (1). One symbol imported on two lines of one file is a
candidate to inline; the same symbol used across nine files is not.

Exit codes: `0` nothing to act on, `1` findings, `2` something went wrong
(no manifest, no Python environment). `registered` and `unknown` are not
findings and don't affect the exit code, so `scant .` works as a CI gate
without failing on dependencies it merely can't judge.

`scant` needs to read your dependencies' *installed* metadata to map a declared
name to what you actually import (e.g. `PyYAML` -> `yaml`) -- there's no
reliable way to do that without them being installed somewhere. It looks, in
order, at: `--env` if you passed it, `$VIRTUAL_ENV`, `$CONDA_PREFIX`, then any
`pyvenv.cfg`-marked folder under the scanned path (so `.venv`, `venv`, `env`,
`venv311`, etc. are all found automatically -- no need to name it right).
`--env` also accepts a direct interpreter path, not just a directory
(`--env .venv/bin/python`), which is handy inside containers or CI images
that install packages straight into a system Python.

If none of that resolves -- or more than one folder looks like a venv --
`scant` explains what it checked and how to point it at the right one,
rather than guessing:

    $ scant .
    Couldn't find a Python environment for this project.

    scant needs to read your dependencies' installed metadata to map declared
    names to imports (e.g. "PyYAML" -> "yaml") -- there's no reliable way to
    do that without them actually being installed somewhere.

    Checked:
      ├── $VIRTUAL_ENV   not set
      ├── $CONDA_PREFIX  not set
      └── ./   no .venv, venv, or other pyvenv.cfg-marked folder

    To fix this:
      1. Activate a virtualenv with your dependencies installed, then re-run scant
      2. Or point at one directly:    scant . --env path/to/venv
      3. Or point at an interpreter:  scant . --env path/to/venv/bin/python

## Example output

    mkdocs -- 15 packages declared, 65 files read, 0.1s
    Plan: drop 0, inline 4, unknown 3, keep 8.

      ACTION   PACKAGE             USES  LINES  USE       WHERE
      inline   markupsafe             1      1  trivial   mkdocs/utils/templates.py:55
      inline   mergedeep              1      1  trivial   mkdocs/utils/yaml.py:149
      inline   pyyaml_env_tag         1      1  trivial   mkdocs/utils/yaml.py:120

      unknown  babel                  0      0  none      declared, but not installed in this environment
      unknown  colorama               0      0  none      only installs when platform_system == 'Windows'

      keep     click                  4     41  heavy     mkdocs/__main__.py +3 files
      keep     watchdog               2      2  light     mkdocs/livereload/__init__.py

## What the verdicts mean

**drop** -- declared, installed, and never imported. The only destructive
recommendation `scant` makes, so it requires positive proof the package is
there and unused, never merely that nothing was found.

**inline** -- used so little that copying the code in probably beats carrying
the dependency. This is the signal `scant` exists for; the `WHERE` column
gives you the exact line.

**keep** -- genuinely used.

**registered** -- never imported, but something loads it by name at runtime:
a SQLAlchemy dialect, a pytest plugin, a Django app in `INSTALLED_APPS`, a
shell command, or a driver another package imports on your behalf. The
`WHERE` column names the mechanism, so you can check the claim rather than
take it.

**unknown** -- `scant` can't judge it. Gated to another platform
(`only installs when sys_platform == 'win32'`), not installed, or installed
with metadata that reveals no import names. Saying so is the honest answer;
"drop" would be a guess dressed as a finding.

Every verdict shows the numbers behind it. There is no score to trust blindly.

## Dependencies loaded by something else

Some packages are never imported by your code because another package imports
them for you -- a database driver reached through Django or SQLAlchemy is the
usual case. `scant` finds most of these from installed metadata alone. For the
rest, the only record is the other package's own source:

    scant . --safe-to-scan-site-packages

Off by default. It reads the source of packages you already import, runs only
over dependencies already heading for `drop`, and reports what it finds with a
file and line (`imported by django/db/backends/postgresql/base.py:26`). It
never counts anything it reads there as your usage.

## License

MIT
