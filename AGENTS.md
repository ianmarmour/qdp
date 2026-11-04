# AGENTS.md

Guidelines for agents contributing to this repository: an open source
implementation of the Quick Desktop Protocol (QDP) in Rust.

## Code Style

- **Readability first.** Always value readability over cleverness or brevity.
- **Documentation.** Use rustdoc formatting (`///` and `//!`) on all code,
  both internal and public.
- **Constants over magic values.** Store magic values in named constants.
- **Derive traits.** Prefer `#[derive(...)]` over manual trait
  implementations whenever possible.
- **Newtypes.** Use newtypes wherever possible to give meaning and type
  safety to primitive values.
- **Enums for closed sets.** Prefer enums over `bool` or `String` when
  representing a closed set of values.
- **Traits.** Use traits thoughtfully and don't overuse them. Only introduce a
  trait when there is more than one concrete implementation.
- **Macros.** Avoid writing our own macros whenever possible.
- **Logging.** Use `tracing` wherever applicable.
- **Reuse.** Reuse existing code where possible instead of duplicating logic.
- **Imports.** Organize imports uniformly across files, and only import what
  you need where possible.

## Dependencies

- Prefer the standard library (`std`) whenever possible.
- Avoid external dependencies whenever possible.
- When a dependency is necessary, prefer established crates maintained by
  major companies or well known ecosystems (e.g. `tokio`, `thiserror`,
  `serde`).
- Enable only the features a dependency actually requires, where possible.

## Testing

- Unit tests live inline in each source file, in a `#[cfg(test)] mod tests`
  module.
- Integration tests live in the `tests/` directory.

## Documentation

- **Minimal over maximal.** Say what something does and how to use it, not how
  it's built.
- **No implementation details.** Leave out file paths in the code, type,
  function and module names, log targets, and anything else that changes when
  the code does.
- **One source of truth.** Point to `qdp help` for command options, to the
  annotated example configuration for settings, and to rustdoc for APIs,
  rather than repeating them. The command-line programs' READMEs show their
  help in a Usage section, which their tests keep identical to it:
  `QDP_UPDATE_USAGE=1 cargo test` rewrites it after an option changes.
- **Uniform.** Give a new document the shape of the others of its kind: every
  crate README alike, every app README alike, every install page alike.
- **No filler.** Leave out sentences that only introduce or explain a heading,
  table or list; let the content speak for itself.
- **Dependencies.** Every app and crate README has a Dependencies table with
  the columns Dependency, Version, Platform and Needed for, one fact per cell.
  A crate's lists the tools and native libraries it needs beyond
  `Cargo.toml`, which already gives its Rust version; an app's lists what it
  builds with. A `+` marks a minimum version, and a bare number an exact one.
  A change to one of those versions updates the table in the same pull
  request.

## Releases

- Publish a release only when the maintainer asks for one, as
  [docs/releasing.md](docs/releasing.md) describes.
- The release's description is the version tag's annotation, written for the
  people who use QDP: a paragraph on what the release brings, its highlights,
  and anything existing installs must do. It follows the documentation rules
  above; the changelog GitHub adds below it lists every pull request.
- Tag with `git tag -a --cleanup=verbatim`, so that the description's
  Markdown headings survive.

## Changes, Commits, and Pull Requests

- Keep changes small and focused, with meaningful commit messages.
- Don't add features or refactor unrelated code as part of a change.
- Keep pull requests under 2000 lines of code when possible.
- Author and commit every change as the maintainer,
  `Ian Armour <ianmarmour@gmail.com>`, set with `git config user.name` and
  `git config user.email`. Agents must never commit under their own identity.
- Every pull request description must include an easy-to-read, bulleted list
  of the changes it makes.
- Delete a pull request's branch once it is merged.
