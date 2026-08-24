![scant](scant-logo.png)

[![CI](https://github.com/alan707/scant/actions/workflows/ci.yml/badge.svg)](https://github.com/alan707/scant/actions/workflows/ci.yml)
[![image](https://img.shields.io/pypi/v/scant.svg)](https://pypi.org/project/scant/)
[![image](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Find the Python dependencies you barely use.

Most tools tell you what's unused. `scant` also tells you what's barely used. Think of those dependencies pulled in for one function, called on a couple of lines, that might be stopping you from your next Python upgrade (true story).

Built for plain English: flags are spelled out in full (`--threshold-lines`, not `-L`), and every result or error reads in words a non-developer can follow. The idea is to continue making the CLI very nice to look at. Like candy for your eyes.

Written in rust ⚡️

## Try it out

`scant` reads your dependencies' installed metadata to map a declared name to
what you actually import (`PyYAML` -> `yaml`), so it needs a project whose
dependencies are installed in a virtual environment. If you're working on one,
you already have that. If not, mkdocs is small and quick:

    git clone --depth 1 https://github.com/mkdocs/mkdocs
    cd mkdocs

    uv venv                 # create .venv
    uv pip install -e .     # install mkdocs's own dependencies into it

Then run `scant` against it, without installing `scant` at all:

    uvx scant .

    mkdocs -- 15 packages declared, 65 files read, 0.0s
    Plan: drop 0, inline 4, unknown 3, keep 8.

## Install

    pipx install scant
    uv tool install scant     # same thing, if you already use uv

`scant` is a tool, not a library. You never import it. Installing it with
plain `pip` puts it inside whichever environment is active, which is usually
the project venv it is about to read; `pipx` and `uv tool` keep it on `$PATH`
and out of the way.

You can also run it as a Github Action to confirm the dependencies you have are
actually necessary.

## Usage

    scant .                      # the project in this folder
    scant path/to/project
    scant . --threshold-lines 5  # widen what counts as barely used

A dependency is "barely used" when it stays under *all three* thresholds:
`--threshold-lines` (default 3), `--threshold-files` (2), and
`--threshold-symbols` (1). A candidate to inline is one that has max 3 lines
of code and could be potentially blocking you from upgrading python versions.

Exit codes: `0` nothing to act on, `1` findings, `2` something went wrong
(no manifest, no Python environment). `registered` and `unknown` are not
findings and don't affect the exit code, so `scant .` works as a CI gate
without failing on dependencies it merely can't judge.

## Example output

Here is [PostHog](https://github.com/PostHog/posthog): It has 192 dependencies,
19,340 Python files. It only took 2.9s to run!

    posthog -- 192 packages declared, 19340 files read, 2.9s
    Plan: drop 1, inline 37, registered 11, unknown 1, keep 142.

      ACTION      PACKAGE                     USES  LINES  USE       WHERE
      drop        dagster-cloud                  0      0  none      --

      inline      css-inline                     1      1  trivial   posthog/email.py:46
      inline      django-cors-headers            1      1  trivial   posthog/settings/web.py:275
      inline      disposable-email-domains       2      2  trivial   posthog/models/integration/email.py:44

      registered  dagster-celery                 0      0  none      console_scripts: dagster-celery
      registered  celery-redbeat                 0      0  none      named as a string in posthog/management/commands/run_autoreload_celery.py:34

      unknown     tbb                            0      0  none      only installs when platform_machine == 'x86_64' and sys_platform == 'linux'

      keep        asgiref                      295   1690  heavy     ee/api/conversation.py +293 files
      keep        django-structlog               2      2  light     posthog/celery.py

Look at `css-inline`. PostHog carries an entire dependency for one line in one
file.  This is why `scant` recommends removing that dependency and adding the underlying
code into the repo.

## What the verdicts mean

**drop**: declared, installed, and never imported. The only destructive
recommendation `scant` makes. This is similar to other python dependency
finders.

**inline**: used so little that copying the code into your codebase probably
makes more sense than carrying another dependency. This is the whole reason
`scant` exists, and what separates it from unused-dependency finders like
deptry.

**keep**: genuinely used.

**registered**: never imported, but something loads it by name at runtime:
a SQLAlchemy dialect, a pytest plugin, a Django app in `INSTALLED_APPS`, a
shell command, or a driver another package imports on your behalf.

**unknown**: `scant` can't judge it because it doesn't have the right OS or
platform (`only installs when sys_platform == 'win32'`), not installed, or installed
with metadata that reveals no import names.

Every verdict shows the numbers behind it.

## Dependencies loaded by something else

Some packages are never imported by your code because another package imports
them for you. A database driver reached through Django or SQLAlchemy is the
usual case. `scant` finds most of these from installed metadata alone. For the
rest, the only record is the other package's own source:

    scant . --safe-to-scan-site-packages

Off by default because you might be running `scant` in an environment with random
dependencies installed. If you know the site-packages are the ones from the repo,
you can use that flag.

## Pointing at a different environment

Usually you don't have to. `scant` looks at `$VIRTUAL_ENV`, `$CONDA_PREFIX`,
then any `pyvenv.cfg`-marked folder under the scanned path, so `.venv`,
`venv`, `env`, `venv311` etc are all found without being named.

When that isn't what you want, `--env` takes a virtualenv, a conda prefix, a
bare site-packages folder, or a direct interpreter path:

    scant . --env path/to/venv
    scant . --env path/to/venv/bin/python

The interpreter form is the one for containers and CI images that install
packages straight into a system Python, with no virtualenv at all.

If nothing resolves, or if more than one folder looks like a venv, `scant`
says what it checked and how to point it at the right one, rather than
guessing:

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

## License

MIT
