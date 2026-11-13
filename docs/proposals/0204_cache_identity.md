- Feature Name: `cache_identity`
- Start Date: 2026-11-12
- Proposal PR:
- Flux Issue:
- Status: Partially Implemented (S1 and S2; S3 open)
- Relates to: [0139](implemented/0139_incremental_module_caching.md) (the module cache this keys), [0177](implemented/0177_package_manager.md) (the shared store and the roots cache), KI-079 (the build id)

# Proposal 0204: Cache identity without a hand-bumped epoch

## Summary
[summary]: #summary

Every cached artifact is keyed by two values: `CACHE_EPOCH`, a number bumped
by hand, and `compiler_build_id()`, a fingerprint of the compiler executable.
Since KI-079 the build id changes on every rebuild, so it already covers what
the epoch was bumped for. This proposal does three things:
- It renames the epoch to `CACHE_FORMAT` and bumps it only when a reader can no
  longer reach the build id. This is rustc's rule for `METADATA_VERSION`.
- It adds the git commit to the build id. A release build, and only a release
  build, drops the executable fingerprint, so its id is the same on every
  machine. This follows GHC's `RELEASE=YES` split.
- It fixes the two caches the build id does not reach: the roots cache and the
  native support object.

## Motivation
[motivation]: #motivation

### The epoch is bumped for semantic changes

`CACHE_EPOCH` (`src/shared/cache_paths.rs`) is at 145. Its value changed in 132
commits. Its doc comment is a changelog of about 425 lines, one note per bump,
saying what an older artifact holds that the current compiler would not
produce. Two more keys follow the same practice:

| Key | Where | Value | Commits that changed it |
|---|---|---|---|
| `CACHE_EPOCH` | `src/shared/cache_paths.rs` | 145 | 132 |
| `NATIVE_CACHE_ABI_TAG` | `src/llvm/module_cache.rs` | `native-cache-abi-v37:…` | 31 |
| `COMPILER_ABI` | `src/driver/artifact_store.rs` | `flux-fxmc-26` | 1 |

Each bump is a line to write and a merge conflict when two branches bump at
once. Forgetting one is a silent stale cache.

### The build id already does the job

KI-079 (fixed 2026-09-09) made every cache record and check
`compiler_build_id()`: the crate version plus a hash of the executable's path,
length and modification time. A rebuild relinks the binary, so the id changes
and every artifact written by the previous build is skipped:
- module interfaces (`.flxi`), in `load_cached_interface`
  (`src/compiler/module_interface.rs`);
- program and module bytecode (`.fxc`, `.fxm`), read right after the magic and
  the format number (`src/bytecode/bytecode_cache/`);
- native object metadata (`.fno`), in `NativeModuleCache::validate`
  (`src/llvm/module_cache.rs`);
- store units, hashed into `unit_hash` (`src/driver/artifact_store.rs`).

95 of the 132 epoch bumps came after that fix. In normal use none of them
changed what was read, because the build id had changed first. A bump still
matters when:
- `FLUX_BUILD_ID` pins the id, which only
  `a_different_compiler_build_does_not_reuse_cached_modules` does;
- the executable can't be read, and the id falls back to the crate version;
- the header before the build id changes. It has been magic, `u16`, build-id
  string since the bytecode caches were added (`43fac802`, `7baee595`).

### Two caches the build id doesn't reach

**The roots cache** (`src/driver/manifest_roots.rs`) stores resolved package
roots. Its first line is `epoch <CACHE_EPOCH>`, and it has no build id. Today a
bump protects it. Once semantic bumps stop, a change to how roots are resolved
would read roots resolved by the old compiler.

**The native support object** (`flux_support_O0.o`, `flux_support_O2.o`) is
emitted by the compiler from an empty LIR program and cached under a fixed name.
`compile_native_support_object` (`src/driver/pipeline/native/mod.rs`) reuses it
whenever the file exists. Neither key reaches it. A native build under
`FLUX_BUILD_ID=build-a` and then under `build-b` linked the same file (same
inode, same modification time). A compiler change to the code it holds is
never picked up until `flux clean`.

## Guide-level explanation
[guide-level-explanation]: #guide-level-explanation

For contributors:
- **A change to what the compiler emits needs no bump.** New primops,
  dictionary layout, ownership plans, a runtime calling convention: rebuilding
  the compiler changes its build id, and every cache recompiles.
- **Bump `CACHE_FORMAT` only for a header change.** That is a change that stops
  a reader from finding the build id: a field before it in the bytecode
  header, or `compiler_version` renamed or moved in the JSON metadata.
- **A release build sets `FLUX_RELEASE=1`.** Its build id is then
  `<version>+<commit>`, the same on every machine, so store units can be shared.
  Every other build keeps the executable fingerprint.
- **`--no-cache` and `flux clean` stay** for investigating. Some known issues
  still show only with a cache or only without one.

## Reference-level explanation
[reference-level-explanation]: #reference-level-explanation

### §1 `CACHE_FORMAT`

`CACHE_EPOCH` becomes `CACHE_FORMAT` and keeps the value 145, so no key moves.
The changelog is deleted, since git history keeps it. The doc comment states the
rule. The five aliases and use sites move to the new name:
- `FORMAT_VERSION` in `bytecode_cache/mod.rs` and `bytecode_cache/module_cache.rs`;
- `NATIVE_MODULE_CACHE_FORMAT_VERSION`;
- `MODULE_INTERFACE_FORMAT_VERSION`;
- the `"format"` field of `unit_hash`. Its label stays, so store digests are
  unchanged.

`NATIVE_CACHE_ABI_TAG` and `COMPILER_ABI` keep their values and stop being
bumped. The native metadata compares `compiler_version`, and `unit_hash` feeds
it, so the build id covers what the tags were bumped for. Removing them would
change every key once for no gain.

### §2 The roots cache

The first line becomes `format <CACHE_FORMAT> build <compiler_build_id()>`.
Each rebuild of the compiler then re-resolves roots once, which reads the
manifests and the lockfile and costs little.

### §3 The native support object

The object's file name carries a short hash of `compiler_build_id()`, in both
`compile_native_support_object` and `support_object_path`, which `flux cache`
uses. This was KI-197, fixed test-first: a native test builds under two
`FLUX_BUILD_ID`s and checks that the second doesn't link the first's object.

### §4 The build id

`build.rs` sets two values at build time:
- `FLUX_GIT_COMMIT`, from `git rev-parse HEAD`, with `rerun-if-changed` on
  `.git/HEAD` and the ref it names. In a worktree `.git` is a file that names
  the git directory, so read that file first. Without git, the commit part is
  left out.
- `FLUX_RELEASE`, with `rerun-if-env-changed=FLUX_RELEASE`.

`compiler_build_id()` becomes:

| Build | Build id |
|---|---|
| `FLUX_BUILD_ID` set at run time | the given value |
| Built with `FLUX_RELEASE=1` | `<version>+<commit12>` |
| Any other build | `<version>+<commit12>+<exe fingerprint>` |

The release id can't depend on whether the working tree is clean. Cargo reruns
`build.rs` only when its listed inputs change, not when a source file is
edited. So a binary built after editing a clean commit would still report that
commit as clean, and its id would equal the real build's. That is KI-079 again.

## Drawbacks
[drawbacks]: #drawbacks

- Two dev builds used alternately recompile each other's artifacts. This is
  already true since KI-079.
- A release id trusts the release process: two different binaries built with
  `FLUX_RELEASE=1` from the same commit would share a cache.

## Rationale and alternatives
[rationale-and-alternatives]: #rationale-and-alternatives

- **Keep bumping.** It works, but most bumps are redundant, and a missed one
  fails silently.
- **Drop the format number.** A header change would then be read as garbage
  rather than skipped. The number costs nothing while it doesn't move.
- **Hash the executable's contents.** This is Go's devel approach, and it
  doesn't depend on mtimes. KI-079 rejected it: a debug binary is hundreds of
  megabytes, and the id is computed on every run.

## Prior art
[prior-art]: #prior-art

All three split the job in two: a format number bumped by hand only when a
reader can't reach the identity field, and a compiler identity that changes on
its own.

| | Format check | Compiler identity | Dev builds |
|---|---|---|---|
| GHC | `.hi` magic number. It "does not change when we change interface file format" (`compiler/GHC/Iface/Binary.hs`, `readBinIfaceHeader`) | `hiVersion = cProjectVersionInt ++ cProjectPatchLevel` (`compiler/GHC/Settings/Constants.hs`). With `RELEASE=NO` the patch level is the date of the latest commit (`configure.ac`). A mismatch is "mismatched interface file versions" | Hadrian makes the stage compiler a build dependency (`needBuilders`, `hadrian/src/Builder.hs`), so a rebuilt GHC rebuilds every library. `GHC.Iface.Recomp` has no "compiler changed" reason |
| rustc | `METADATA_VERSION` (12), incremented only "if you change the format of metadata such that the rustc version can't be found to compare with `rustc_version()`" (`compiler/rustc_metadata/src/rmeta/mod.rs`) | The `rustc` version string with its commit hash, stored after the header. A mismatch is E0514 | Bootstrap rebuilds the standard library when the stage compiler changes. This detail is from memory, not checked against the source |
| Go | Export-data format version | The action ID hashes every input, including the tool ID from `compile -V=full` (`cmd/go/internal/work/buildid.go`) | A devel toolchain's tool ID is the content ID of the compiler binary. A release uses the version string |

The GHC references are from a local checkout at `aefa0c1b90` (2026-08-16).

## Unresolved questions
[unresolved-questions]: #unresolved-questions

- Whether to remove the frozen tags later, at a release that invalidates every
  cache anyway.

## Future possibilities
[future-possibilities]: #future-possibilities

- `flux cache` could print the commit each artifact was built by.
- A release id that is the same on every machine lets CI publish store units
  for the standard library.

## Stages
[stages]: #stages

- **S1, done 2026-11-12:** §1 and §2. The roots cache test came first
  (`f15fa6c0`), then the fix (`1fd4e92a`), then the rename and the frozen tags
  (`0f5e4391`).
- **S2, done 2026-11-12:** §3, as KI-197 (`c31fcada`). Its native test needed a
  working native link, and that turned out to be KI-196: Cargo 1.100 puts its
  build artifacts in `target/<profile>/build/<package>/<hash>/out/` and has no
  `deps/`, where the compiler looked for the Rust staticlib (`e443125c`).
- **S3, open:** §4.

Follow-ups from S2, outside this proposal:
- `build.rs` prunes old staticlib archives only in `deps/`, so under Cargo
  1.100 the archives under `target/<profile>/build/flux/` are never removed.
- `ci.yml` pins Rust 1.93.0 and `release.yml` uses `@stable`, so a toolchain
  change such as KI-196 reaches a release before CI sees it.
