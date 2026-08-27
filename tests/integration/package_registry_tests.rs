//! Registry dependencies resolve through the lockfile and build.
//!
//! These exercise the whole registry path end to end: `flux.toml` declares a
//! semver requirement, the index says which versions exist, the resolver
//! picks one, `flux.lock` records it with its checksum, and the resolved
//! package becomes a scoped module root that the program can import.
//!
//! Every test points `$FLUX_HOME` at its own scratch directory, so nothing
//! here reads or writes the developer's real registry state.

use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "../support/scratch.rs"]
mod scratch;
use scratch::Scratch;

fn flux_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_flux"))
}

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create dir");
    }
    std::fs::write(path, contents).expect("write file");
}

/// A project laid out with its own `$FLUX_HOME`.
struct Fixture {
    _scratch: Scratch,
    app: PathBuf,
    home: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let app = scratch.path().join("app");
        let home = scratch.path().join("home");
        Self {
            _scratch: scratch,
            app,
            home,
        }
    }

    /// Declare the application, requiring `requirement` of package `json`.
    fn app_requiring(&self, requirement: &str) {
        write(
            &self.app.join("flux.toml"),
            &format!(
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n\
                 [dependencies]\njson = \"{requirement}\"\n"
            ),
        );
        write(
            &self.app.join("src").join("main.flx"),
            "import Json as Json\n\nfn main() with IO { print(Json.tag()) }\n",
        );
    }

    /// Publish a version of `json` to the index, and unpack its sources.
    ///
    /// The index line and the unpacked sources are written together because
    /// that is the state a real fetch leaves behind; a resolved version whose
    /// sources are missing is covered by its own test.
    fn publish(&self, version: &str, checksum: &str) {
        self.publish_index_only(version, checksum);
        write(
            &self
                .home
                .join("registry/src/json")
                .join(version)
                .join("src")
                .join("Json.flx"),
            &format!(
                "module Json {{\n    public fn tag() -> String {{ \"json {version}\" }}\n}}\n"
            ),
        );
    }

    /// Append a version to the index without unpacking its sources.
    fn publish_index_only(&self, version: &str, checksum: &str) {
        let index = self.home.join("registry/index/json");
        let mut text = std::fs::read_to_string(&index).unwrap_or_default();
        text.push_str(&format!(
            "{{\"name\":\"json\",\"version\":\"{version}\",\"checksum\":\"{checksum}\"}}\n"
        ));
        write(&index, &text);
    }

    /// Run `flux run` in the project, returning combined output and success.
    fn run(&self) -> (String, bool) {
        let output = Command::new(flux_bin())
            .current_dir(&self.app)
            .arg("run")
            .env("FLUX_HOME", &self.home)
            .env("NO_COLOR", "1")
            .output()
            .expect("run flux");
        let mut text = String::from_utf8_lossy(&output.stdout).replace("\r\n", "\n");
        text.push_str(&String::from_utf8_lossy(&output.stderr).replace("\r\n", "\n"));
        (text, output.status.success())
    }

    fn lock(&self) -> String {
        std::fs::read_to_string(self.app.join("flux.lock")).unwrap_or_default()
    }

    fn remove_lock(&self) {
        let _ = std::fs::remove_file(self.app.join("flux.lock"));
    }
}

#[test]
fn a_registry_dependency_resolves_and_builds() {
    let fixture = Fixture::new("pkg-registry-build");
    fixture.app_requiring("^1.0");
    fixture.publish("1.0.0", "sha256:aa");

    let (text, ok) = fixture.run();
    assert!(ok, "expected the build to succeed:\n{text}");
    assert!(text.contains("json 1.0.0"), "{text}");
}

/// Highest-version-first: a successful resolution should also be a good one.
#[test]
fn the_highest_matching_version_is_chosen() {
    let fixture = Fixture::new("pkg-registry-highest");
    fixture.app_requiring("^1.0");
    fixture.publish("1.0.0", "sha256:aa");
    fixture.publish("1.2.0", "sha256:bb");
    fixture.publish("2.0.0", "sha256:cc");

    let (text, ok) = fixture.run();
    assert!(ok, "expected the build to succeed:\n{text}");
    assert!(
        text.contains("json 1.2.0"),
        "expected the highest 1.x, got:\n{text}"
    );
}

/// The lockfile records the version *and* the checksum the index published,
/// which is what makes tampering detectable rather than executable.
#[test]
fn the_resolution_is_written_to_the_lockfile() {
    let fixture = Fixture::new("pkg-registry-lockfile");
    fixture.app_requiring("^1.0");
    fixture.publish("1.2.0", "sha256:bb");

    let (text, ok) = fixture.run();
    assert!(ok, "expected the build to succeed:\n{text}");

    let lock = fixture.lock();
    assert!(lock.contains("name = \"json\""), "{lock}");
    assert!(lock.contains("version = \"1.2.0\""), "{lock}");
    assert!(lock.contains("checksum = \"sha256:bb\""), "{lock}");
    assert!(lock.contains("source = \"registry+"), "{lock}");
}

/// The property the lockfile exists for: publishing a newer version does not
/// change an existing build.
#[test]
fn a_locked_version_survives_a_newer_publication() {
    let fixture = Fixture::new("pkg-registry-pinned");
    fixture.app_requiring("^1.0");
    fixture.publish("1.2.0", "sha256:bb");

    let (text, ok) = fixture.run();
    assert!(ok, "expected the first build to succeed:\n{text}");
    assert!(text.contains("json 1.2.0"), "{text}");

    fixture.publish("1.5.0", "sha256:cc");

    let (text, ok) = fixture.run();
    assert!(ok, "expected the rebuild to succeed:\n{text}");
    assert!(
        text.contains("json 1.2.0"),
        "a published version must not change a locked build:\n{text}"
    );
}

/// Deleting the lockfile re-resolves. This is the direction the roots cache
/// originally got wrong — see docs/known_issues.md#ki-024.
#[test]
fn deleting_the_lockfile_re_resolves() {
    let fixture = Fixture::new("pkg-registry-relock");
    fixture.app_requiring("^1.0");
    fixture.publish("1.2.0", "sha256:bb");

    let (text, ok) = fixture.run();
    assert!(ok, "expected the first build to succeed:\n{text}");

    fixture.publish("1.5.0", "sha256:cc");
    fixture.remove_lock();

    let (text, ok) = fixture.run();
    assert!(ok, "expected the rebuild to succeed:\n{text}");
    assert!(
        text.contains("json 1.5.0"),
        "deleting the lockfile must re-resolve:\n{text}"
    );
    assert!(fixture.lock().contains("1.5.0"), "{}", fixture.lock());
}

/// A lock is a preference, not a pin: a manifest the locked version no longer
/// satisfies re-resolves instead of failing.
#[test]
fn a_manifest_edit_overrides_the_lock() {
    let fixture = Fixture::new("pkg-registry-manifest-edit");
    fixture.app_requiring("^1.0");
    fixture.publish("1.2.0", "sha256:bb");
    fixture.publish("2.0.0", "sha256:cc");

    let (text, ok) = fixture.run();
    assert!(ok, "expected the first build to succeed:\n{text}");
    assert!(text.contains("json 1.2.0"), "{text}");

    fixture.app_requiring("^2.0");

    let (text, ok) = fixture.run();
    assert!(ok, "expected the rebuild to succeed:\n{text}");
    assert!(
        text.contains("json 2.0.0"),
        "an edited manifest must re-resolve rather than stay pinned:\n{text}"
    );
}

/// A rebuild that changes nothing must not rewrite the lockfile, or every
/// build would show up as a change in version control.
#[test]
fn an_unchanged_rebuild_leaves_the_lockfile_alone() {
    let fixture = Fixture::new("pkg-registry-stable");
    fixture.app_requiring("^1.0");
    fixture.publish("1.2.0", "sha256:bb");

    let (text, ok) = fixture.run();
    assert!(ok, "expected the first build to succeed:\n{text}");
    let first = fixture.lock();

    let (text, ok) = fixture.run();
    assert!(ok, "expected the rebuild to succeed:\n{text}");
    assert_eq!(
        first,
        fixture.lock(),
        "the lockfile churned across a rebuild"
    );
}

#[test]
fn an_unsatisfiable_requirement_is_reported() {
    let fixture = Fixture::new("pkg-registry-unsatisfiable");
    fixture.app_requiring("^2.0");
    fixture.publish("1.0.0", "sha256:aa");

    let (text, ok) = fixture.run();
    assert!(!ok, "expected the build to fail:\n{text}");
    assert!(
        text.contains("json"),
        "the failure must name the package:\n{text}"
    );
}

#[test]
fn a_package_missing_from_the_index_is_reported() {
    let fixture = Fixture::new("pkg-registry-missing");
    fixture.app_requiring("^1.0");

    let (text, ok) = fixture.run();
    assert!(!ok, "expected the build to fail:\n{text}");
    assert!(
        text.contains("json"),
        "the failure must name the package:\n{text}"
    );
}

/// A version that resolves but was never unpacked is a different failure from
/// one that does not exist, and says so.
#[test]
fn a_resolved_but_unpacked_version_is_reported() {
    let fixture = Fixture::new("pkg-registry-unpacked");
    fixture.app_requiring("^1.0");
    fixture.publish_index_only("1.0.0", "sha256:aa");

    let (text, ok) = fixture.run();
    assert!(!ok, "expected the build to fail:\n{text}");
    assert!(
        text.contains("sources are not in"),
        "expected a sources-missing message:\n{text}"
    );
}

/// A malformed index line fails the read rather than being skipped: an index
/// that silently drops entries resolves differently depending on which lines
/// happened to parse.
#[test]
fn a_malformed_index_line_is_reported() {
    let fixture = Fixture::new("pkg-registry-malformed");
    fixture.app_requiring("^1.0");
    fixture.publish("1.0.0", "sha256:aa");
    let index = fixture.home.join("registry/index/json");
    let mut text = std::fs::read_to_string(&index).expect("read index");
    text.push_str("this is not json\n");
    write(&index, &text);

    let (text, ok) = fixture.run();
    assert!(!ok, "expected the build to fail:\n{text}");
    assert!(
        text.contains("unreadable") || text.contains("not valid JSON"),
        "expected an index-parse failure:\n{text}"
    );
}

/// A registry package with no checksum cannot be verified, so a lockfile
/// carrying one is rejected rather than trusted.
#[test]
fn a_lockfile_entry_without_a_checksum_is_rejected() {
    let fixture = Fixture::new("pkg-registry-nochecksum");
    fixture.app_requiring("^1.0");
    fixture.publish("1.0.0", "sha256:aa");
    write(
        &fixture.app.join("flux.lock"),
        "version = 1\n\n[[package]]\nname = \"json\"\nversion = \"1.0.0\"\n\
         source = \"registry+https://packages.flux-lang.org\"\n",
    );

    let (text, ok) = fixture.run();
    assert!(!ok, "expected the build to fail:\n{text}");
    assert!(
        text.contains("checksum"),
        "expected a checksum failure:\n{text}"
    );
}

/// A lockfile from a newer toolchain is refused rather than partially read.
#[test]
fn a_future_lockfile_format_is_rejected() {
    let fixture = Fixture::new("pkg-registry-future");
    fixture.app_requiring("^1.0");
    fixture.publish("1.0.0", "sha256:aa");
    write(&fixture.app.join("flux.lock"), "version = 99\n");

    let (text, ok) = fixture.run();
    assert!(!ok, "expected the build to fail:\n{text}");
    assert!(
        text.contains("does not understand"),
        "expected a format-version failure:\n{text}"
    );
}

/// A project with no registry dependencies never reads the index and never
/// writes a lockfile, so the common case costs nothing.
#[test]
fn a_project_without_registry_dependencies_writes_no_lockfile() {
    let fixture = Fixture::new("pkg-registry-none");
    write(
        &fixture.app.join("flux.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\n",
    );
    write(
        &fixture.app.join("src").join("main.flx"),
        "fn main() with IO { print(\"standalone\") }\n",
    );

    let (text, ok) = fixture.run();
    assert!(ok, "expected the build to succeed:\n{text}");
    assert!(text.contains("standalone"), "{text}");
    assert!(
        !fixture.app.join("flux.lock").exists(),
        "a project with no registry dependencies must not get a lockfile"
    );
}
