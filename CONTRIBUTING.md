# Contributing to Kagaz

Thanks for helping. A few ground rules keep the project fast and trustworthy.

## Code

- Rust 2021, `cargo fmt` and `cargo clippy -- -D warnings` clean before a PR.
- The core crate must build on Linux, Windows and macOS. Platform-specific
  code lives behind `cfg` gates with a shared trait in front of it.
- No new runtime dependency without a one-line justification in the PR.
  Binary size and start-up time are features.
- Anything that talks to a device gets a test against a recorded exchange
  (see `crates/kagaz-core/tests/fixtures`) so it can run without hardware.

## Driver database (`drivers/`)

One TOML file per model family. Only links to the vendor's own servers; never
mirror binaries here. Include the checksum the vendor publishes when there is
one, and the exact manual steps the vendor's instructions require.

## Commits and pull requests

Plain, descriptive messages that say what changed and why. Do not add
"Co-Authored-By" trailers or tool attribution lines of any kind.

## Reporting a device that does not work

Run `kagaz discover --report` and attach the output. It contains the device's
advertised capabilities and nothing personal.
