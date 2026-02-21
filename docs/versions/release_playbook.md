# Flux Release Playbook

This is the operational checklist for cutting a Flux release (`vX.Y.Z`) on GitHub.

---

## 1) Milestone and Scope

1. Create GitHub milestone: `vX.Y.Z`.
2. Ensure all in-scope PRs target that milestone.
3. Freeze feature scope at least 48h before tag day.

Recommended labels:
- `feature`
- `bug`
- `perf`
- `docs`
- `breaking`
- `no-changelog` (exception only)

---

## 2) PR Hygiene (Before Freeze)

Every PR should:
1. Include a changelog fragment in `changes/*.md`.
2. Pass CI (`fmt`, `clippy`, tests).
3. Include VM/JIT coverage for backend-sensitive changes.

PRs without fragment should use `no-changelog` with justification.

---

## 3) Pre-Release Gates (Local)

Run:

```bash
scripts/release_check.sh
```

Then run optional extra checks from:
- `docs/versions/release_regression_v0.0.3.md`

---

## 4) Build Changelog and Cut Release Section

1. Rebuild `[Unreleased]` from fragments:

```bash
scripts/changelog_from_fragments.sh
```

2. Cut release section:

```bash
scripts/release_cut.sh vX.Y.Z
```

3. Review `CHANGELOG.md`:
- date is correct
- categories are sensible
- no duplicate/low-value entries

---

## 5) Release Notes Docs

Update/add:
- `docs/versions/whats_new_vX.Y.Z.md`
- `docs/versions/README.md` (add new version row)
- optional: root `README.md` version matrix updates

---

## 6) Release PR

Create `release/vX.Y.Z` PR (or a final release PR on `main`) containing:
- `CHANGELOG.md`
- `docs/versions/whats_new_vX.Y.Z.md`
- any final docs/version bumps

Merge only when:
- all checks green
- no open blocker issues

---

## 7) Tag and Publish

```bash
git checkout main
git pull --ff-only
git tag vX.Y.Z
git push origin main
git push origin vX.Y.Z
```

GitHub Actions release workflow:
- trigger: `v*` tag
- gate job runs fmt/clippy/tests/smoke
- if green, Linux artifact + checksum uploaded
- release notes body extracted from `CHANGELOG.md` section `## [vX.Y.Z]`

---

## 8) Post-Release Tasks

1. Verify GitHub Release assets downloaded/executable.
2. Create next milestone (`vX.Y.(Z+1)`).
3. Confirm `[Unreleased]` is clean and ready for next cycle.
4. Announce release with links:
- GitHub release page
- `docs/versions/whats_new_vX.Y.Z.md`

---

## 9) Rollback Plan

If tag/release is wrong:
1. Delete Git tag locally/remotely:
   - `git tag -d vX.Y.Z`
   - `git push origin :refs/tags/vX.Y.Z`
2. Fix `CHANGELOG.md`/docs.
3. Re-run checks.
4. Re-tag and push.
