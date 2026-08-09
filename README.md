# scant

Find the Python dependencies you barely use.

Most tools tell you what's unused. scant also tells you what's barely used — a dependency pulled in for one function, called on a couple of lines, that's probably not worth carrying.

**Status:** pre-alpha, targeting v0.0.1. See [PLAN.md](PLAN.md).

## Install

Not yet published.

    pipx install scant

## Usage

    scant .
    scant . --max-lines 5

## Example output

    numpy
      imports:    1
      files:      1
      lines:      1
      usage:      trivial
      verdict:    inline

## License

MIT
