# Changelog Fragments

Flux uses changelog fragments to avoid merge conflicts and keep release notes accurate.

## Add a fragment in each feature/fix PR

Create a new file in `changes/`:

```text
changes/2026-02-21-short-topic.md
```

Use this format:

```markdown
### Added
- New user-visible capability.

### Changed
- Existing behavior that changed.

### Fixed
- Bug fix.

### Performance
- Performance improvement or regression note.

### Docs
- Documentation updates.
```

Sections are optional; only include what applies.

## Commands

Update `[Unreleased]` from fragments:

```bash
scripts/changelog_from_fragments.sh
```

Cut a release section from `[Unreleased]`:

```bash
scripts/release_cut.sh v0.0.4
```

This will:
- move current `[Unreleased]` entries into `## [v0.0.4] - YYYY-MM-DD`
- reset `[Unreleased]`
- update compare links at the bottom of `CHANGELOG.md`
