# Flume package manager

Flume is Flux's package manager. Users run it through the flux command.
It uses flux.toml for package metadata and dependencies, and flux.lock for
the resolved dependency graph.

This guide describes the package-manager commands currently implemented in
Flux.

## Requirements

Build or install the Flux compiler first:

~~~sh
cargo build
export PATH="$PWD/target/debug:$PATH"
~~~

Verify the package commands:

~~~sh
flux --help
~~~

When working from the Flux repository, use cargo run -- instead of flux:

~~~sh
cargo run -- new hello
~~~

## Create a package

Create a binary package in a new directory:

~~~sh
flux new hello
cd hello
~~~

This creates:

~~~text
hello/
├── flux.toml
└── src/
    └── main.flx
~~~

Create a library package:

~~~sh
flux new greeter --lib
~~~

For greeter, the library entry module is src/Greeter.flx.

Initialize the current directory with flux init:

~~~sh
flux init
flux init --lib
~~~

Without a name, flux init uses the current directory name. It does not
overwrite an existing flux.toml.

## Package layout

A binary package normally uses src/main.flx. A library package normally
uses a namespace root:

~~~text
greeter/
├── flux.toml
└── src/
    ├── Greeter.flx
    └── Greeter/
        └── Format.flx
~~~

Explicit library and binary targets are also supported:

~~~toml
[lib]
path = "src/Greeter.flx"

[[bin]]
name = "cli"
path = "src/cli.flx"
~~~

For a bare package command, Flux selects src/main.flx, then the first
declared binary, then [lib], and finally the conventional namespace module.
Select a named binary with:

~~~sh
flux run --bin cli
flux build --bin cli
~~~

## The manifest

The smallest binary manifest is:

~~~toml
[package]
name = "hello"
version = "0.1.0"
edition = "2026"
~~~

| Field | Required | Description |
| --- | --- | --- |
| name | Yes | Package name. |
| version | Yes | Three-part semantic version, such as 0.1.0. |
| edition | No | Language edition. Defaults to 2026. |
| namespace | No | Module namespace. Derived from name when omitted. |

flux-json derives the namespace FluxJson. Override it when needed:

~~~toml
[package]
name = "http-client"
version = "0.1.0"
namespace = "HttpClient"
~~~

## Dependencies

### Path dependencies

Use a path dependency for another local package:

~~~toml
[dependencies]
shared = { path = "../shared" }
~~~

The path must contain its own flux.toml.

### Git dependencies

Use a Git repository:

~~~toml
[dependencies]
flux-greeter = { git = "https://github.com/sger/flux-greeter" }
~~~

Pin it with one of these forms:

~~~toml
flux-greeter = { git = "https://github.com/sger/flux-greeter", branch = "main" }
flux-greeter = { git = "https://github.com/sger/flux-greeter", tag = "v1.0.0" }
flux-greeter = { git = "https://github.com/sger/flux-greeter", rev = "50543e1" }
~~~

Only one of branch, tag, or rev may be used. Without a pin, the repository's
default branch is used. Branches and tags resolve to a commit, which is
recorded in flux.lock.

Git dependencies are fetched by the system git executable.

### Registry dependencies

A registry requirement is a version string:

~~~toml
[dependencies]
json = "^1.2"
~~~

The inline form is also accepted:

~~~toml
json = { version = "~1.2" }
~~~

Supported requirements include:

~~~text
1.2       # equivalent to ^1.2.0
^1.2.3
~1.2.3
>=1.2.0
<2.0.0
=1.2.3
>=1.2.0, <2.0.0
~~~

The resolver selects one version per package name and prefers the highest
matching version.

Registry data is currently read from the local Flux home directory. Automatic
registry download and HTTPS upload are not implemented.

The local registry layout is:

~~~text
$FLUX_HOME/
└── registry/
    ├── index/<package>                # one JSON entry per version
    └── src/<package>/<version>/       # unpacked package sources
~~~

FLUX_HOME defaults to ~/.flux. An index entry looks like:

~~~json
{"name":"json","version":"1.2.0","checksum":"sha256:...","deps":[]}
~~~

### Development dependencies

Development-only dependencies belong in [dev-dependencies]:

~~~toml
[dev-dependencies]
test-support = { path = "../test-support" }
~~~

They are available to package tests and excluded from the normal build tree.

## Module namespaces

Every package owns a namespace. A package named flux-greeter normally owns
FluxGreeter:

~~~text
flux-greeter/
└── src/
    ├── FluxGreeter.flx
    └── FluxGreeter/
        └── Style.flx
~~~

Import it from another package with:

~~~flux
import FluxGreeter
import FluxGreeter.Style
~~~

The module name and source path must match the package namespace. For example,
module FluxGreeter.Style belongs at src/FluxGreeter/Style.flx.

## Build and run

Run package commands from a directory containing flux.toml:

~~~sh
flux check       # type-check without running
flux build       # build the package and dependencies
flux run         # build and run the package
flux test        # run test_* functions
flux tree        # print the resolved dependency graph
~~~

flux test discovers test_* functions in package modules. For a standalone
source file, use:

~~~sh
flux --test path/to/tests.flx
~~~

Pass program arguments after --:

~~~sh
flux run -- --name Flux
~~~

## Build profiles

Profiles choose the compilation backend and its optimization setting. With no
profile table, package commands use `dev` by default:

~~~toml
[profile.dev]
backend = "vm"
optimize = false

[profile.release]
backend = "native"
optimize = true
~~~

`dev` uses the bytecode VM without optimization. `release` selects the native
LLVM pipeline and enables LLVM optimization. Native optimization is a large
code-generation tier change; optimization on the VM is a smaller improvement.
Profiles affect compilation and optimization, not Flux language semantics.

Choose a profile for one package command with `--profile`:

~~~sh
flux build --profile dev
flux run --profile release
flux test --profile release
~~~

The native release profile requires a compiler built with LLVM support:

~~~sh
cargo build --features llvm
~~~

`--native` and `--vm` override the profile backend. Likewise, `--optimize`
(`-O`) and `--no-optimize` override the profile optimization setting. These
overrides apply only to the current command. The Flume resolver always runs on
the VM, regardless of the package profile.

Useful flags include:

~~~sh
flux build --verbose
flux build --explain-rebuild
flux build --no-cache
flux build --native
flux test --test-filter arithmetic
~~~

The native backend requires LLVM:

~~~sh
cargo run --features llvm -- --native path/to/file.flx
~~~

## Add and remove dependencies

flux add edits flux.toml while preserving its formatting:

~~~sh
flux add shared --path ../shared
flux add flux-greeter --git https://github.com/sger/flux-greeter
flux add flux-greeter --git https://github.com/sger/flux-greeter --rev 50543e1
flux add json --version '^1.2'
flux add test-support --path ../test-support --dev
~~~

Remove a dependency with:

~~~sh
flux remove shared
flux remove test-support --dev
~~~

Each dependency entry may have only one source: path, version, or git. Git
dependencies may have at most one pin: rev, tag, or branch.

## Lockfiles and reproducible builds

The first build resolves dependencies and writes flux.lock. Commit it to
version control. It records exact package versions, Git commits, registry
checksums, and resolved dependencies.

Update dependencies explicitly:

~~~sh
flux update
flux update -p flux-greeter
flux update -p one -p two
~~~

Use strict modes in CI:

~~~sh
flux build --locked    # fail if flux.lock would change
flux build --offline   # do not access the network
flux build --frozen    # both --locked and --offline
~~~

--offline can use existing Git checkouts and local registry data. It cannot
resolve a missing dependency or fetch a new commit.

## Workspaces

A workspace has one root manifest and multiple member packages:

~~~text
workspace/
├── flux.toml
├── flux.lock
├── app/
│   ├── flux.toml
│   └── src/main.flx
└── shared/
    ├── flux.toml
    └── src/Shared.flx
~~~

Declare the members in the root manifest:

~~~toml
[workspace]
members = ["app", "shared"]
~~~

Run commands from the root or any member directory. Flux finds the workspace
root and uses its resolution and lockfile:

~~~sh
cd workspace/app
flux build
flux metadata --format json
~~~

Members can refer to each other with path dependencies:

~~~toml
[dependencies]
shared = { path = "../shared" }
~~~

## Metadata and build plans

Print the resolved graph as versioned JSON:

~~~sh
flux metadata --format json
~~~

The output has format_version: 1 and includes the workspace root, members,
resolved package roots, and target/cache locations.

Print the build-unit plan:

~~~sh
flux build --plan
~~~

The plan includes the workspace root and units with package, target, backend,
source, and unit-hash fields.

## Store and cache

Flux uses two artifact layers:

- local package outputs are written under target/flux/;
- reusable dependency artifacts are stored under
  $FLUX_HOME/store/flux-fxmc-26/<unit-hash>/<backend>/.

The unit hash includes source content, package-relative identity, compiler/ABI,
backend, semantic settings, and dependency interface fingerprints. VM and
native artifacts use separate backend directories.

Inspect or clear cache data with:

~~~sh
flux cache-info src/main.flx
flux native-cache-info src/main.flx
flux clean
flux clean --deps
flux clean --store
~~~

flux clean removes the project cache. --deps also removes downloaded Git
checkouts. --store removes the global artifact store.

For an isolated local or CI run:

~~~sh
flux build --cache-dir /tmp/flux-cache
~~~

## Publishing

Run the local publishing check with:

~~~sh
flux publish --dry-run
~~~

It creates target/flux/publish/<name>-<version>.tar, excludes build and VCS
files, extracts the archive into a fresh temporary directory, builds it with
--no-cache, and prints its SHA-256 checksum.

Example output:

~~~text
created target/flux/publish/hello-0.1.0.tar
sha256:...
verified clean checkout
~~~

The archive contains source and package metadata, not build artifacts. Registry
upload is not available yet, so use --dry-run for successful local
verification.

## Troubleshooting

### No package manifest

Package commands require flux.toml in the current directory or an ancestor.
Run the command from the package directory or use flux init.

### No entry point

Add src/main.flx for a binary, src/<Namespace>.flx for a library, or declare
[lib] or [[bin]].

### Dependency changes are not visible

Use flux update after changing a Git branch or tag:

~~~sh
flux tree
sed -n '1,200p' flux.lock
~~~

For a fresh project build:

~~~sh
flux clean --deps
flux build --no-cache
~~~

### Registry dependency cannot be resolved

Check that the package has an index entry and unpacked source under
$FLUX_HOME/registry/. For CI, set FLUX_HOME to a prepared directory and use
--offline.

## Current limitations

- Registry download is not automatic.
- Registry upload is not implemented; flux publish requires --dry-run for
  successful local verification.
- Package features and conditional compilation are not supported.
- A package name can resolve to only one version in a build graph.

See [proposal 0177](../../docs/proposals/0177_package_manager.md) for design
and implementation details.
