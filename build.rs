// build.rs — prune stale libflux-<hash>.a archives from deps/ (keep 2 newest)
// so the linker's nm fallback scan stays fast and bounded.
//
// We do NOT write the flux-staticlib.path sidecar here: build.rs cannot know
// whether the archive was built with --features llvm (which is required for
// flux_async_* symbols). The linker writes the sidecar after its own verified
// scan, and validates the symbol on every read.
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

fn main() {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    // OUT_DIR = target/<profile>/build/flux-<hash>/out  (three parents up = profile dir)
    let profile_dir = out_dir
        .parent() // build/flux-HASH
        .and_then(|p| p.parent()) // build/
        .and_then(|p| p.parent()); // target/<profile>/

    if let Some(profile_dir) = profile_dir {
        let deps_dir = profile_dir.join("deps");

        // Collect all libflux-<hash>.a / flux-<hash>.lib archives.
        let mut archives: Vec<PathBuf> = fs::read_dir(&deps_dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let name = p.file_name().and_then(|f| f.to_str()).unwrap_or("");
                (name.starts_with("libflux-") && name.ends_with(".a"))
                    || (name.starts_with("flux-") && name.ends_with(".lib"))
            })
            .collect();

        // Sort newest first.
        archives.sort_by_key(|p| {
            fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH)
        });
        archives.reverse();

        // Prune stale archives — keep only the 2 newest to bound future scan cost.
        // If the sidecar pointed at a pruned archive, remove it so the linker
        // falls back to a fresh scan rather than reading a deleted path.
        let sidecar = profile_dir.join("flux-staticlib.path");
        let sidecar_target = fs::read_to_string(&sidecar)
            .map(|s| PathBuf::from(s.trim().to_owned()))
            .ok();
        for stale in archives.iter().skip(2) {
            if sidecar_target.as_deref() == Some(stale.as_path()) {
                let _ = fs::remove_file(&sidecar);
            }
            let _ = fs::remove_file(stale);
        }
    }

    println!("cargo:rerun-if-changed=build.rs");
}
