# Proposal 015: Package/Module Workflow MVP

**Status:** Proposed  
**Priority:** High (Developer Experience)  
**Created:** 2026-02-07  
**Related:** Proposal 011 (Phase 2 Module System Enhancements), Proposal 012 (Phase 2 Module Split Plan)

## Overview

This proposal defines a simple, deterministic package/module workflow for Flux focused on reliability and fast adoption.

The goal is to make project setup, dependency resolution, and module imports predictable with clear diagnostics, while intentionally limiting initial scope to path/git dependencies.

---

## Goals

1. Deterministic builds across machines/CI.
2. Simple project/dependency UX (`init`, `add`, `build`, `test`, `run`).
3. Offline-capable workflow once dependencies are cached.
4. Stable module layout/import rules.
5. Actionable diagnostics for dependency/module resolution failures.

---

## Non-Goals (MVP)

1. Hosted public package registry.
2. Full semver solver complexity.
3. Workspace/monorepo orchestration beyond basic path dependencies.

---

## File Model

### `flux.toml`

Project manifest with metadata and dependency declarations.

```toml
[package]
name = "my_app"
version = "0.1.0"
edition = "2026"

[dependencies]
utils = { path = "../utils" }
jsonx = { git = "https://github.com/org/jsonx", rev = "abc123" }

[dev-dependencies]
testkit = { path = "../testkit" }
```

### `flux.lock`

Generated lock file containing:
- exact resolved package graph
- source details (`path`/`git`)
- pinned commit/revision
- checksums

`flux.lock` should be committed for reproducible CI and team builds.

### `.flux/`

Local cache/build metadata directory (git-ignored).

---

## Standard Project Layout

1. `src/main.flx` for binary entrypoint.
2. `src/lib.flx` for library entrypoint.
3. `tests/` for integration tests.
4. Module mapping: `foo::bar` -> `src/foo/bar.flx`.

---

## CLI Commands (MVP)

1. `flux init`  
   Create project scaffold (`flux.toml`, `src/main.flx`).
2. `flux add <name> --path <path>` or `flux add <name> --git <url> --rev <rev>`  
   Update manifest + lock.
3. `flux remove <name>`  
   Remove dependency + update lock.
4. `flux update`  
   Refresh lock according to manifest constraints.
5. `flux build`  
   Build with locked graph.
6. `flux test`  
   Test with locked graph.
7. `flux run`  
   Build and execute main target.
8. `flux check`  
   Fast syntax/semantic checks.
9. `flux tree`  
   Print dependency graph.
10. `flux clean`  
    Remove local build artifacts.

---

## Resolution Rules

1. `build/test/run/check` must not mutate `flux.lock`.
2. `update` is the only command allowed to refresh locked versions/revisions.
3. Same `flux.toml + flux.lock` must resolve to the same graph.
4. Detect and error on dependency cycles with cycle path in diagnostics.
5. Detect and error on duplicate/ambiguous package identity.

---

## Offline and Integrity

1. `--offline` uses local cache only.
2. Fail fast when required artifacts are missing in offline mode.
3. Verify checksums for fetched artifacts.
4. Fail on checksum mismatch with clear remediation steps.

---

## Import and Module Rules

1. Relative imports are package-local only.
2. Cross-package imports must use package namespace.
3. Error on ambiguous module resolution paths.
4. Error on duplicate module names mapping to same namespace.

---

## Diagnostics Requirements

All package/module failures should include:
1. failing package/module name
2. source path/revision where applicable
3. resolution chain context
4. suggested next step (`add`, `update`, fix path, fix cycle, etc.)

---

## Rollout Plan

### Phase 1 (MVP)

1. `flux.toml` + `flux.lock`.
2. path/git dependencies.
3. deterministic lock-based builds.
4. core commands (`init/add/remove/update/build/test/run/check`).
5. stable module layout and import mapping.

### Phase 2

1. registry support.
2. richer version constraints.
3. improved dependency UX and conflict resolution hints.

### Phase 3

1. workspace support.
2. plugin/hook extension points.

---

## Acceptance Criteria

1. New user can create a project, add path/git dep, build and test in < 5 minutes.
2. CI reproducibility is guaranteed via committed `flux.lock`.
3. Offline builds succeed when cache is present.
4. Dependency and module resolution errors are actionable and non-ambiguous.

---

## Implementation Checklist

1. Add manifest parser/validator for `flux.toml`.
2. Add lockfile schema + serializer/deserializer.
3. Implement deterministic resolver for path/git dependencies.
4. Implement command surface for `init/add/remove/update/build/test/run/check`.
5. Implement module path mapping and collision checks.
6. Add checksum verification and offline cache behavior.
7. Add end-to-end tests for reproducible resolution and cycle diagnostics.
