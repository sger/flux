### Added
- `scripts/check_changelog_fragment.sh` to enforce changelog fragments in PR CI.
- `scripts/changelog_from_fragments.sh` to rebuild `CHANGELOG.md` `[Unreleased]` from `changes/*.md`.
- `scripts/release_cut.sh` to cut a new version section from `[Unreleased]` and update compare links.
- `scripts/release_check.sh` local preflight command documented in `README.md`.

### Changed
- CI now runs changelog fragment validation on pull requests.
- Release docs now use a fragment-first changelog workflow.

### Docs
- Added `changes/README.md` and `changes/_template.md` for contributor guidance.
