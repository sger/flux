use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::OnceLock,
};

use sha2::{Digest, Sha256};

/// Global cache epoch. Bump this single constant to invalidate ALL caches
/// (bytecode `.fxc`, module bytecode `.fxm`, module interfaces `.flxi`,
/// and native `.o` metadata) at once.
///
/// This replaces the need to coordinate 4 separate `FORMAT_VERSION` constants
/// across different cache modules. Each cache type embeds this epoch and
/// rejects entries written with a different value.
///
/// Epoch 1: initial unified epoch (replaces FXBC=11, FXMC=2, flxi=3, native=2).
/// Epoch 2: fix parse_int HM signature (String -> Int, was String -> Option<Int>).
/// Epoch 3: portable symbol table in .flxi (re-intern Symbols across sessions).
/// Epoch 4: relocatable module bytecode round-trips effect descriptors.
/// Epoch 5: generated class-dispatch functions are injected into module bodies
/// for cached VM assembly, preserving `Module.member` exports.
/// Epoch 6: cached module artifacts omit unreferenced imported globals, so
/// interface-only preloads do not become bogus linker dependencies.
/// Epoch 7: cached class dispatch splits module-member stubs from global
/// `__tc_*` instance functions, preserving both export conventions.
/// Epoch 8: module interfaces record exported member kind (`public fn` vs
/// `public let`) so native cached imports do not confuse zero-arg functions
/// with value getters.
/// Epoch 9: parameterized handlers and default-handler unit resumes update
/// cached module bytecode/global relocation shape.
/// Epoch 11: module interfaces record record-style constructor field order
/// (`ctor_field_names`) so importing modules can desugar named-field syntax;
/// the `TryReadFile` primop changes the primop table and cached lowering.
/// Epoch 13: the `FsListDir` and `FsMetadata` primops extend the primop table
/// and change cached lowering; recoverable-I/O runtime calls now take eleven
/// constructor tags rather than eight, changing the native calling convention.
/// Epoch 14: the `Sha256` and `Sha256File` primops extend the primop table.
/// Epoch 15: `Array.get` now returns `Some(x)` rather than the bare element,
/// and array equality is structural on both backends — both change results
/// baked into cached artifacts.
/// Epoch 16: the `Env` effect label and the `EnvVar` / `EnvArgs` / `EnvCwd` /
/// `EnvHomeDir` primops extend the primop table and the `IO` alias expansion.
/// Epoch 17: the `Process` effect label and the `ProcRun` primop extend the
/// primop table and the `IO` alias expansion.
/// Epoch 21: resolved package roots now depend on `flux.lock` as well as on the
/// manifests, so the roots cache records a lockfile fingerprint that older
/// entries do not carry.
/// Epoch 22: `Flow.List.map` / `filter` became effect-row polymorphic and
/// `Flow.Result` gained `map2` / `map3` / `apply` / `sequence` / `traverse`,
/// changing both stdlib interfaces and their global layout.
/// Epoch 23: a module compiled fresh now seeds inference with its dependencies'
/// constructor field types (KI-022), so a module whose imported-constructor
/// payloads previously inferred as unresolved variables compiles differently.
/// Epoch 24: native lowering marks any non-direct-path effect as suspending, so
/// callers of a cross-module `perform` gain yield checks they did not emit
/// before (KI-034). Cached native objects from epoch 23 lack them.
/// Epoch 27: package build profiles and default-eliding semantic config keys.
/// Epoch 28: structured typeclass predicates and kind metadata in interfaces.
/// Epoch 30: explicit instance-body clones receive fresh expression IDs, and
/// module-scoped generated typeclass methods retain qualified native symbols.
/// Epoch 31: typeclass constraints and generated dictionary/method symbols
/// carry the owning ClassId rather than a short class name.
/// Epoch 32: dictionaries lead with a slot per declared superclass, so every
/// method's slot index in a class that declares one has shifted.
/// Epoch 33: module interfaces carry class associated-type declarations and
/// their instance equations, so an importing module reduces an application an
/// epoch-32 interface left stuck.
/// Epoch 34: `Eq` has contextual instances over `List` and `Option`, so a
/// predicate an epoch-33 interface answered structurally — with no dictionary
/// behind it — now resolves to an instance and carries one.
/// Epoch 35: `Eq`, `Ord`, `Num`, `Show` and `Semigroup` moved from Rust
/// registration into the Flux prelude, so their instances, dictionaries and
/// mangled method names now carry the owning module.
/// Epoch 36: `Ord` declares `Eq` as a superclass, so every `Ord` dictionary
/// leads with an `Eq` evidence slot and each method sits one index later than
/// an epoch-35 artifact recorded.
/// Epoch 37: `Semigroup` gained instances over `List`, `Array` and `Option`,
/// and the new `Monoid` class sits on top of it, so a predicate an epoch-36
/// interface answered with no instance now resolves to one.
/// Epoch 38: the `Functor`, `Applicative` and `Monad` classes exist, and a
/// superclass constraint on a higher-kinded class parameter is kind-checked
/// against the owner's parameters rather than defaulting to `Type` — an
/// epoch-37 interface recorded the rejection.
/// Epoch 39: a class method with an effect-row parameter kept its declared
/// return type instead of silently becoming `Unit`, so every `Functor`
/// instance's runtime contract changes; `Either` also gained `Functor`,
/// `Applicative` and `Monad` instances.
/// Epoch 40: a constrained function's compiled arity is now its source arity
/// plus one leading dictionary per runtime-bearing constraint on the AST path
/// as well as the CFG path, and callers across a module boundary pass that
/// evidence (KI-060, KI-061). An epoch-39 `.fxc` was compiled to the old
/// convention, so a fresh caller would arrive one argument too many.
/// Epoch 41: context reduction drops a scheme constraint over a constructed
/// type, so a constrained function loses the dictionary parameter it used to
/// take (KI-077, KI-078), and `Flow.Eq`'s `List` / `Option` instances recurse
/// through `eq` instead of the removed `list_eq` / `option_eq` helpers. An
/// epoch-40 artifact passes the old evidence at the old arity.
/// Epoch 42: `+` desugars to `Flow.Add`'s `add`, not `Flow.Num`'s. `Num` loses
/// its `add` slot and gains a leading `Add` superclass slot, so every `Num`
/// dictionary has a different layout, and `String` now has an `Add` instance
/// where it had no arithmetic instance at all. An epoch-41 artifact indexes the
/// old slots. Proposal 0183 (R2 and R4 change dictionary shape too, and this
/// bump covers them).
/// Epoch 43: a bare call to a name the program itself defines is no longer
/// dispatched as a class method of that name (KI-085), so a call an epoch-42
/// artifact lowered to `__tc_<Class>_<Type>_<name>` now calls the program's own
/// function — different code, same source.
/// Epoch 44: the call arity check no longer accepts `parameters +
/// dictionaries` from a call that carries no dictionaries (KI-082), so a
/// program an epoch-43 artifact compiled and ran — passing a constrained
/// function that many extra arguments — is now an `E056` at compile time. The
/// stale artifact keeps running it, which is how the bug hid: with a warm
/// cache it surfaced as a run-time `E1000` rather than a diagnostic. Epoch 43
/// also predates the KI-083 fix, which changed where the dictionary globals
/// are stored and so changed emitted bytecode.
/// Epoch 45: proposal 0186 rebuilds generics on binding groups. Statements are
/// now grouped by reference rather than by adjacency (KI-087), so a mutually
/// recursive pair separated by an intervening statement lowers to one recursive
/// group instead of two independent binders — an epoch-44 artifact holds the
/// miscompiled form, which ran as `E1001 ... (got Uninit)`. Bumped ahead of the
/// quantification and evidence stages so no artifact written during them is
/// epoch-compatible with the finished compiler.
/// Epoch 46: a field-access predicate no longer contributes its receiver's
/// variables to the quantified set (proposal 0186 stage 4). A binding that
/// quantified such a variable under epoch 45 got a scheme one variable wider,
/// and the predicate that should have been reported at the access was
/// generalized away instead — so an epoch-45 artifact can hold a program that
/// is now an `E490`.
/// Epoch 47: binding groups are emitted in dependency order rather than at
/// their source position. Epoch 45 fixed adjacency-based *grouping* but kept
/// source-order *placement*, so a nested `fn a()` calling a later `fn b()`
/// lowered to `a` outside `b` and captured b's uninitialised slot — the same
/// `E1001 ... (got Uninit)` KI-087 was about, for a plainer program. An
/// epoch-46 artifact holds that miscompiled nesting.
/// Epoch 48: a match whose arms share a pattern family now propagates that
/// family to an unresolved scrutinee even when a catch-all arm is present, and
/// decides arm isolation against the result (KI-095). A binding whose receiver
/// is bound by a match arm therefore infers a type where an epoch-47 compiler
/// either reported `E490` or inferred a narrower one, so cached interfaces and
/// bytecode from epoch 47 disagree with what this compiler would produce.
/// Epoch 49: a recursive group's predeclared placeholder is now unified with
/// what the member infers, and a `let` reads the environment's free variables
/// through the substitution before generalizing (KI-096). A binding that an
/// epoch-48 compiler either rejected with `E490` or generalized too widely
/// infers a different — narrower, and correct — type, so cached interfaces
/// disagree with what this compiler would produce.
/// Epoch 50: tuple projection is a solver predicate rather than a guessed tuple
/// shape (proposal 0185 stage 5). An unresolved receiver used to be unified with
/// a tuple of `max(index + 1, 2)` fresh variables, so a program could be typed
/// against a width no source line asked for; it now carries a predicate
/// discharged after inference. Types an epoch-49 artifact recorded for such a
/// binding can differ, and the projection is rejected where it used to be
/// accepted silently.
/// Epoch 51: an unannotated definition that takes parameters and raises no
/// class constraint is now generalized (the unconstrained half of
/// generalize-by-arity). `fn identity(x) { x }` has a scheme where an epoch-50
/// compiler gave it a monotype, so a cached `.flxi` records the narrower type
/// and a fresh caller would be rejected at a second argument type.
pub const CACHE_EPOCH: u16 = 51;

/// Identity of the *build* that produced this compiler, not just its released
/// version number.
///
/// Every cache key and every cached-artifact metadata record embeds this
/// string. `CARGO_PKG_VERSION` alone is not enough: two builds of the same
/// version — a rebuild after a source change, or the same source built with
/// and without `--features llvm` — produce different compilers that would
/// otherwise share cache entries, so a module compiled by an earlier build is
/// reused verbatim and the current compiler's diagnostics are never reported
/// (see `docs/known_issues.md#ki-079`).
///
/// The fingerprint is the executable's path, length and modification time,
/// which is stable for a given build and changes whenever Cargo relinks. The
/// file is deliberately not hashed: a debug binary is hundreds of megabytes and
/// this runs on every invocation. Set `FLUX_BUILD_ID` to pin the value when a
/// reproducible key is wanted across machines.
pub fn compiler_build_id() -> &'static str {
    static BUILD_ID: OnceLock<String> = OnceLock::new();
    BUILD_ID.get_or_init(|| {
        if let Ok(pinned) = std::env::var("FLUX_BUILD_ID")
            && !pinned.is_empty()
        {
            return pinned;
        }
        match current_exe_fingerprint() {
            Some(fingerprint) => format!("{}+{fingerprint}", env!("CARGO_PKG_VERSION")),
            // An unreadable executable is not a reason to refuse to run. The
            // result is the old behaviour: keyed by version alone.
            None => env!("CARGO_PKG_VERSION").to_string(),
        }
    })
}

fn current_exe_fingerprint() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let metadata = fs::metadata(&exe).ok()?;
    let modified = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let mut hasher = Sha256::new();
    hasher.update(exe.to_string_lossy().as_bytes());
    hasher.update([0]);
    hasher.update(metadata.len().to_le_bytes());
    hasher.update(modified.as_nanos().to_le_bytes());
    Some(crate::shared::hex::encode(&hasher.finalize())[..16].to_string())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheLayout {
    root: PathBuf,
}

impl CacheLayout {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn interfaces_dir(&self) -> PathBuf {
        self.root.join("interfaces")
    }

    pub fn vm_dir(&self) -> PathBuf {
        self.root.join("vm")
    }

    pub fn native_dir(&self) -> PathBuf {
        self.root.join("native")
    }
}

pub fn resolve_cache_layout(entry_file: &Path, cache_dir: Option<&Path>) -> CacheLayout {
    CacheLayout {
        root: resolve_cache_root(entry_file, cache_dir),
    }
}

pub fn resolve_cache_root(entry_file: &Path, cache_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = cache_dir {
        return absolutize(dir);
    }

    if let Some(project_root) = find_project_root(entry_file) {
        return project_root.join("target").join("flux");
    }

    entry_directory(entry_file).join(".flux").join("cache")
}

/// Locate the project root by walking up from `entry_file`.
///
/// A Flux package normally roots at its nearest `flux.toml`. When that
/// package is named by an ancestor workspace, the workspace manifest wins so
/// all members share cache and lockfile state.
///
/// `Cargo.toml` remains a fallback purely for the compiler's own test corpus,
/// which runs `.flx` fixtures from inside this Rust checkout and has no
/// `flux.toml` of its own. It is consulted only after the whole ancestor chain
/// has been searched for `flux.toml`, so a Flux project nested in a Rust
/// workspace still roots at its own manifest.
pub fn find_project_root(entry_file: &Path) -> Option<PathBuf> {
    let nearest = find_marker_upwards(entry_file, "flux.toml");
    nearest
        .as_ref()
        .and_then(|root| workspace_root_for(root))
        .or(nearest)
        .or_else(|| find_marker_upwards(entry_file, "Cargo.toml"))
}

/// Locate the nearest package manifest, even when it belongs to a workspace.
/// This is used for selecting the member's entry target; [`find_project_root`]
/// remains the workspace-wide cache and lockfile root.
pub fn find_package_root(entry_file: &Path) -> Option<PathBuf> {
    find_marker_upwards(entry_file, "flux.toml")
}

fn workspace_root_for(package_root: &Path) -> Option<PathBuf> {
    let mut current = package_root.to_path_buf();
    loop {
        if current.join("flux.toml").is_file()
            && current != package_root
            && workspace_contains(&current, package_root)
        {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Small, deliberately conservative reader used only for root discovery. The
/// authoritative TOML parser remains Flume; accepting only a normal
/// `members = ["..."]` declaration here avoids making cache-root discovery a
/// second manifest parser.
fn workspace_contains(workspace_root: &Path, package_root: &Path) -> bool {
    let Ok(text) = fs::read_to_string(workspace_root.join("flux.toml")) else {
        return false;
    };
    let relative = package_root
        .strip_prefix(workspace_root)
        .ok()
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let mut in_workspace = false;
    let mut members = String::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_workspace = trimmed == "[workspace]";
        } else if in_workspace
            && (trimmed.starts_with("members")
                || (!members.is_empty() && !trimmed.starts_with(']')))
        {
            members.push_str(trimmed);
        }
    }
    members.split('"').enumerate().any(|(index, value)| {
        index % 2 == 1 && (value == relative || (relative.is_empty() && value == "."))
    })
}

fn find_marker_upwards(entry_file: &Path, marker: &str) -> Option<PathBuf> {
    let mut current = absolutize(entry_file);
    if current.is_file() {
        current.pop();
    }

    loop {
        if current.join(marker).exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

pub fn interface_cache_path(cache_root: &Path, source_path: &Path) -> PathBuf {
    cache_root
        .join("interfaces")
        .join(format!("{}.flxi", artifact_stem(source_path)))
}

pub fn cache_key_filename(source_path: &Path, cache_key: &[u8; 32], ext: &str) -> String {
    format!(
        "{}-{}.{}",
        artifact_stem(source_path),
        hex_prefix(cache_key, 16),
        ext
    )
}

pub fn artifact_stem(source_path: &Path) -> String {
    let readable = source_path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(sanitize_component)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "module".to_string());
    let path_hash = short_path_hash(source_path);
    format!("{readable}-{path_hash}")
}

fn entry_directory(entry_file: &Path) -> PathBuf {
    let absolute = absolutize(entry_file);
    if absolute.is_dir() {
        absolute
    } else {
        absolute
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."))
    }
}

fn absolutize(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }

    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

fn short_path_hash(path: &Path) -> String {
    let canonicalish = fs::canonicalize(path).unwrap_or_else(|_| absolutize(path));
    let mut hasher = Sha256::new();
    hasher.update(normalize_for_hash(&canonicalish));
    let digest = hasher.finalize();
    hex_prefix(digest.as_slice(), 12)
}

fn normalize_for_hash(path: &Path) -> String {
    let mut normalized = String::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push_str(&prefix.as_os_str().to_string_lossy()),
            Component::RootDir => normalized.push('/'),
            Component::CurDir => normalized.push('.'),
            Component::ParentDir => normalized.push_str(".."),
            Component::Normal(part) => normalized.push_str(&part.to_string_lossy()),
        }
        normalized.push('/');
    }
    normalized
}

fn sanitize_component(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn hex_prefix(bytes: &[u8], len: usize) -> String {
    let mut out = String::with_capacity(len * 2);
    for b in bytes.iter().take(len) {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{
        artifact_stem, cache_key_filename, find_project_root, interface_cache_path,
        resolve_cache_root,
    };
    use std::path::{Path, PathBuf};

    #[test]
    fn finds_repo_root_from_nested_entry() {
        let entry = Path::new("examples/aoc/2024/day06.flx");
        let root = find_project_root(entry).expect("expected Cargo project root");
        assert!(root.ends_with("flux"));
    }

    #[test]
    fn resolves_repo_cache_root_to_target_flux() {
        let entry = Path::new("examples/aoc/2024/day06.flx");
        let root = resolve_cache_root(entry, None);
        assert!(root.ends_with(Path::new("flux/target/flux")));
    }

    #[test]
    fn resolves_non_cargo_cache_root_to_local_flux_cache() {
        let entry = PathBuf::from("/tmp/flux-standalone/example.flx");
        let root = resolve_cache_root(&entry, None);
        let suffix = Path::new("flux-standalone").join(".flux").join("cache");
        assert!(
            root.ends_with(&suffix),
            "expected root to end with {}, got {}",
            suffix.display(),
            root.display()
        );
    }

    #[test]
    fn explicit_cache_dir_wins() {
        let entry = Path::new("examples/aoc/2024/day06.flx");
        let root = resolve_cache_root(entry, Some(Path::new("tmp/cache")));
        assert!(root.ends_with(Path::new("tmp/cache")));
    }

    #[test]
    fn interface_paths_live_under_interfaces_dir() {
        let root = Path::new("/tmp/flux-cache");
        let path = interface_cache_path(root, Path::new("examples/aoc/2024/day06.flx"));
        assert_eq!(
            path.parent().unwrap(),
            Path::new("/tmp/flux-cache/interfaces")
        );
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".flxi")
        );
    }

    #[test]
    fn cache_filenames_include_path_hash_and_cache_key() {
        let filename =
            cache_key_filename(Path::new("examples/shared/Main.flx"), &[0xabu8; 32], "fxm");
        assert!(filename.starts_with("Main-"));
        assert!(filename.ends_with(".fxm"));
        assert!(filename.contains("-abababababababababababababababab."));
    }

    #[test]
    fn artifact_stem_changes_for_same_basename_in_different_dirs() {
        let a = artifact_stem(Path::new("examples/alpha/Main.flx"));
        let b = artifact_stem(Path::new("examples/beta/Main.flx"));
        assert_ne!(a, b);
    }
}
