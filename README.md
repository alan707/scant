# scant

Find the Python dependencies you barely use.

Most tools tell you what's unused. scant also tells you what's barely used. Think of those dependencies pulled in for one function, called on a couple of lines, that might be stopping you from your next Python upgrade (true story).

Built for plain English: flags are spelled out in full (`--threshold-lines`, not `-L`), and every result or error reads in words a non-developer can follow. The idea is to continue making the CLI very nice to look at. Like candy for your eyes.

**Status:** pre-alpha, targeting v0.0.1. See [PLAN.md](PLAN.md).

Written in rust ⚡️

## Install

Not yet published.

    pipx install scant

## Usage

    scant .
    scant . --threshold-lines 5

## Example output

    numpy
      imports:    1
      files:      1
      lines:      1
      usage:      trivial
      verdict:    inline

## License

MIT
