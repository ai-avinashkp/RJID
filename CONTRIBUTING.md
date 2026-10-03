# Contributing to RJID

Thanks for your interest! Bug reports, ideas and pull requests are welcome.

## Before you open a pull request

RJID is source-available under the PolyForm Noncommercial License 1.0.0, and the
author may also offer it under commercial licenses. To keep that possible, every
contribution needs this agreement. Opening a pull request means you accept it:

> I wrote this contribution (or have the right to submit it). I license it to the
> RJID copyright holder under the same terms as the project, and I also grant the
> copyright holder a perpetual, worldwide, irrevocable, royalty-free license to use,
> modify, sublicense and relicense it, including under commercial terms.

If you can't agree to that, please open an issue describing the change instead.

## Ground rules

- **No code copied from GPL projects.** That includes Zed's own editor and
  workspace crates. RJID depends only on Zed's Apache-2.0 `gpui` crates; everything
  else is written from scratch.
- **New dependencies** must be permissively licensed (MIT, Apache-2.0, BSD, ISC or
  similar). Add them to `THIRD_PARTY_LICENSES.md`.
- **Never panic on user input or I/O.** Missing files, bad JSON and failed
  processes show a message and keep the IDE running.
- **Run `cargo test --workspace`** before submitting, and keep the build free of
  warnings.

## Building

```bash
cargo run -p rji-app            # debug build
cargo test --workspace          # unit tests
```

`docs/ROADMAP.md` explains how each feature was verified and the GPUI quirks to
know before changing the window layout.
