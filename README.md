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

    mkdocs -- 15 packages declared, 65 files read, 0.4s
    Plan: drop 3, inline 4, keep 8.

      ACTION  PACKAGE             USES  LINES  USE       WHERE
      drop    colorama               0      0  none      --

      inline  markupsafe             1      1  trivial   mkdocs/utils/templates.py:55
      inline  pyyaml_env_tag         1      1  trivial   mkdocs/utils/yaml.py:120

      keep    click                  4     41  heavy     mkdocs/__main__.py +3 files
      keep    watchdog               2      2  light     mkdocs/livereload/__init__.py

## License

MIT
