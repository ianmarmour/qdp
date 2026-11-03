# Contributing to QDP

Thanks for helping with QDP. This page covers how to report problems, set up
a development machine, and get a change merged.

## Reporting a problem

Open an [issue](https://github.com/ianmarmour/qdp/issues) with:

- what you did, what you expected, and what happened instead;
- the host's and the client's systems and versions, and which app or command
  you used;
- the logs of both sides, ideally with `QDP_LOG=debug` (see
  [Troubleshooting](docs/troubleshooting.md#logs)).

## Setting up

[Building QDP from source](docs/building.md) lists what each platform needs,
and the [architecture overview](docs/architecture.md) explains how the crates
and apps fit together. Each crate and app has a README of its own, linked from
there.

## Making a change

- **Keep changes small and focused.** One change per pull request, with
  meaningful commit messages. Don't refactor unrelated code along the way.
  Keep pull requests under 2000 lines when possible.
- **Describe it.** Every pull request's description includes an easy-to-read,
  bulleted list of the changes it makes, and how you tested them.
- **Test it.** Unit tests live inline in each source file, in a
  `#[cfg(test)] mod tests` module; integration tests live in each crate's
  `tests` directory.
- **Check it.** Before opening a pull request, run:

  ```bash
  cargo fmt --all --check
  cargo clippy --workspace --all-targets --locked -- -D warnings
  cargo test --workspace --locked
  ```

  CI doesn't run on pull requests by itself: a maintainer starts it on your
  branch ([Releasing](docs/releasing.md#when-ci-runs) explains why).
- **Change the specification with the protocol.** A change to what goes over
  the wire also changes the chapter of the [specification](docs/spec/README.md)
  that defines it, in the same pull request.

## Code style

- **Readability first.** Value readability over cleverness or brevity.
- **Documentation.** Use rustdoc (`///` and `//!`) on all code, internal and
  public.
- **Constants over magic values.** Give magic values named constants.
- **Types.** Prefer `#[derive(...)]` over manual trait implementations,
  newtypes that give meaning to primitive values, and enums over `bool` or
  `String` for closed sets of values.
- **Traits.** Introduce a trait only when it has more than one concrete
  implementation.
- **Macros.** Avoid writing our own.
- **Logging.** Use `tracing`.
- **Reuse.** Reuse existing code instead of duplicating logic.
- **Imports.** Organize them uniformly across files, and import only what you
  need.

### Documentation

- **Minimal over maximal.** Say what something does and how to use it, not how
  it's built.
- **No implementation details.** Leave out file paths in the code, type,
  function and module names, log targets, and anything else that changes when
  the code does.
- **One source of truth.** Point to `qdp help` for command options, to the
  annotated example configuration for settings, and to rustdoc for APIs,
  rather than repeating them.
- **Uniform.** Give a new document the shape of the others of its kind: every
  crate README alike, every app README alike, every install page alike.
- **Dependencies.** Every app and crate README has a Dependencies table with
  the columns Dependency, Version, Platform and Needed for, one fact per cell.
  A crate's lists the tools and native libraries it needs beyond
  `Cargo.toml`, which already gives its Rust version; an app's lists what it
  builds with. A `+` marks a minimum version, and a bare number an exact one.
  A change to one of those versions updates the table in the same pull
  request.

### Dependencies

- Prefer the standard library.
- Avoid external dependencies where possible. When one is needed, prefer
  established crates from major companies or well-known ecosystems, such as
  `tokio`, `thiserror` or `serde`.
- Enable only the features a dependency needs.

[`AGENTS.md`](AGENTS.md) holds the same rules for coding agents.

## License

QDP is licensed under the [Apache License 2.0](LICENSE). Unless you say
otherwise, any contribution you submit is licensed under it too, as section 5
of the license sets out.
