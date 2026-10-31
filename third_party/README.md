# Third-party code

## rav1e 0.8.1, patched

`rav1e/` is the [rav1e](https://github.com/xiph/rav1e) AV1 encoder, version
0.8.1, which QDP uses as its software fallback encoder. The workspace uses it
in place of the crates.io release through `[patch.crates-io]` in the root
`Cargo.toml`.

rav1e is licensed under the BSD 2-Clause License (`rav1e/LICENSE`), with the
patent grant in `rav1e/PATENTS`.

### Provenance

The directory holds the crates.io package exactly as published, without the
`.cargo-ok` marker Cargo writes when unpacking it, plus the patch below. The
package's SHA-256 is
`43b6dd56e85d9483277cde964fd1bdb0428de4fec5ebba7540995639a21cb32b`, the
checksum crates.io and Cargo record for rav1e 0.8.1. To check that the only
differences are the patch, download the package and compare:

```bash
curl -L https://crates.io/api/v1/crates/rav1e/0.8.1/download | tar -xz -C /tmp
```

```bash
diff -r /tmp/rav1e-0.8.1 third_party/rav1e
```

The commit "Vendor rav1e 0.8.1 unmodified" adds the package as published; the
next commit adds the patch.

### The patch

Commit "Patch rav1e to allow encoding without lookahead" changes
`src/api/internal.rs` and `src/api/config/mod.rs`, and adds two tests to
`src/api/test.rs`.

Without it, rav1e returns each frame four frames after receiving it, even in
low-latency mode with the shortest lookahead it accepts, because its keyframe
and RDO lookahead queues must fill first. That adds about 67 ms at 60 frames
per second.

With it, an RDO lookahead of zero frames is accepted in low-latency mode.
Combined with disabled scene detection, the keyframe lookahead is skipped too,
so each frame is encoded as soon as it is received. Configurations that worked
before are unaffected, since an RDO lookahead of zero was rejected, and
rav1e's existing tests pass unchanged.

### Removing it

Once rav1e accepts an equivalent change, delete `rav1e/`, the
`[patch.crates-io]` entry and the `exclude` entry in the root `Cargo.toml`, and
raise the `rav1e` version in `[workspace.dependencies]`.
