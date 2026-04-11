//! Output normalization for parity comparison.
//!
//! Strips known non-semantic noise (backend banners, cargo progress, temp paths)
//! while preserving all user-visible output.

/// Normalize raw process output for parity comparison.
///
/// Removes:
/// - Backend banner lines (`[cfg→vm] ...`, `[lir→llvm] ...`)
/// - Cargo/toolchain progress lines (`Compiling ...`, `Finished ...`)
/// - Absolute temp paths (`/tmp/flux_native_XXXX/...` → `<TMPDIR>`)
///
/// Preserves:
/// - User stdout/stderr
/// - Rendered diagnostics (beyond path normalization)
/// - Runtime error messages
pub fn normalize(raw: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim_start();

        if should_strip_build_progress_line(trimmed) || trimmed.starts_with("Downloading ") {
            continue;
        }

        lines.push(line.trim_end());
    }

    let joined = lines.join("\n");
    normalize_temp_paths(&joined).trim().to_string()
}

/// Replace absolute temp paths from native compilation with a placeholder.
fn normalize_temp_paths(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut rest = s;

    while let Some(idx) = rest.find("/tmp/flux_native_") {
        result.push_str(&rest[..idx]);
        result.push_str("<TMPDIR>");
        // Skip past the temp path segment (until whitespace, quote, or end)
        let after = &rest[idx..];
        let end = after
            .find(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == ')' || c == ':')
            .unwrap_or(after.len());
        rest = &after[end..];
    }

    result.push_str(rest);
    result
}

/// Normalize `--dump-core` output for cross-way comparison.
///
/// Core IR dumps are pre-backend, so they should be identical across ways.
/// This normalizer strips:
/// - Aether stats summary lines (counts may vary between binary builds)
/// - Absolute file paths (replace with `<FILE>`)
/// - Empty lines (insignificant whitespace differences in dump ordering)
/// - Trailing whitespace per line
pub fn normalize_core_dump(raw: &str) -> String {
    let mut lines: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed_start = line.trim_start();

        if should_strip_build_progress_line(trimmed_start) {
            continue;
        }

        // Strip Aether stats lines (e.g., "Aether: 3 dups, 2 drops, 1 reuse")
        if trimmed_start.starts_with("Aether:") || trimmed_start.starts_with("── Aether") {
            continue;
        }

        // Strip timing/stats lines
        if trimmed_start.starts_with("Core IR lowered in")
            || trimmed_start.starts_with("Passes completed in")
        {
            continue;
        }

        // Skip empty/whitespace-only lines to ignore trivial ordering diffs
        if trimmed_start.is_empty() {
            continue;
        }

        let normalized = normalize_debug_ids(line);
        // Trim trailing whitespace but preserve leading indentation
        lines.push(normalized.trim_end().to_string());
    }

    // Sort consecutive drop-only lines to normalize Aether insertion order.
    // Adjacent `drop x` lines within the same block are semantically
    // equivalent regardless of order.
    let lines = sort_consecutive_drops(lines);

    let joined = lines.join("\n");
    normalize_file_paths(&joined)
}

/// Sort runs of consecutive drop lines alphabetically.
///
/// Aether may emit drops in different orders depending on compilation flags.
/// Sorting makes the comparison stable. Handles both Core IR format
/// (`drop x`) and Aether report format (`- line N: drop x`).
fn sort_consecutive_drops(lines: Vec<String>) -> Vec<String> {
    let mut result: Vec<String> = Vec::with_capacity(lines.len());
    let mut drop_run: Vec<String> = Vec::new();

    for line in lines {
        if is_drop_line(&line) {
            drop_run.push(line);
        } else {
            if !drop_run.is_empty() {
                drop_run.sort();
                result.append(&mut drop_run);
            }
            result.push(line);
        }
    }

    if !drop_run.is_empty() {
        drop_run.sort();
        result.append(&mut drop_run);
    }

    result
}

/// Check if a line is a drop statement in either Core IR or Aether format.
fn is_drop_line(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("drop ") || (trimmed.starts_with("- ") && trimmed.contains("drop "))
}

/// Replace absolute file paths with a placeholder for stable comparison.
fn normalize_file_paths(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut rest = s;

    // Normalize paths like /home/user/.../file.flx or /tmp/.../file.flx
    while let Some(idx) = rest.find(".flx") {
        // Walk backwards from ".flx" to find the start of the path
        let before = &rest[..idx];
        if let Some(path_start) = before.rfind('/') {
            // Find the real start of the absolute path (first / in sequence)
            let mut start = path_start;
            for (i, c) in before[..path_start].char_indices().rev() {
                if c == '/' || c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
                    start = i;
                } else {
                    break;
                }
            }
            // Only normalize if it looks like an absolute path
            if before.as_bytes().get(start) == Some(&b'/') {
                result.push_str(&rest[..start]);
                result.push_str("<FILE>");
                rest = &rest[idx + 4..]; // skip past ".flx"
                continue;
            }
        }
        // Not a path, keep as-is up through ".flx"
        result.push_str(&rest[..idx + 4]);
        rest = &rest[idx + 4..];
    }

    result.push_str(rest);
    result
}

/// Normalize `--dump-aether=debug` output for cross-way comparison.
///
/// The Aether debug report includes per-function ownership metadata:
/// borrow signatures, call modes, dup/drop/reuse details. This normalizer
/// preserves the semantically important ownership structure while stripping:
/// - Line numbers in drop/dup references (e.g., `line 8:` → `line N:`)
/// - Symbol IDs that may differ between binaries (e.g., `<sym:2000002>#5`)
/// - Empty lines and trailing whitespace
/// - Title/separator lines
/// - Consecutive drop lines are sorted for order-independence
pub fn normalize_aether_dump(raw: &str) -> String {
    let mut lines: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim_start();

        if should_strip_build_progress_line(trimmed) {
            continue;
        }

        // Skip title and separator lines
        if trimmed == "Aether Memory Model Report" || trimmed == "Aether Ownership Report"
            || trimmed.chars().all(|c| c == '=')
            || trimmed.is_empty()
        {
            continue;
        }

        // Normalize line numbers: "line 8:" → "line N:"
        let normalized = normalize_line_numbers(line);
        // Normalize symbol IDs: "<sym:2000002>#5" → "<sym>#N"
        let normalized = normalize_sym_ids(&normalized);
        // Normalize debug-only temp and binder numbering.
        let normalized = normalize_debug_ids(&normalized);

        lines.push(normalized.trim_end().to_string());
    }

    let lines = sort_consecutive_drops(lines);
    let joined = lines.join("\n");
    normalize_file_paths(&joined)
}

/// Normalize `--dump-repr` output for cross-way comparison.
///
/// The representation contract dump is intentionally backend-agnostic, so the
/// normalizer only strips empty lines, trailing whitespace, and title lines.
pub fn normalize_repr_dump(raw: &str) -> String {
    raw.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed != "Flux Backend Representation Contract" && !trimmed.chars().all(|c| c == '=')
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Normalize `--dump-cfg` output for parity/debug inspection.
pub fn normalize_cfg_dump(raw: &str) -> String {
    normalize_backend_ir_dump(raw)
}

/// Normalize `--dump-lir` output for parity/debug inspection.
pub fn normalize_lir_dump(raw: &str) -> String {
    normalize_backend_ir_dump(raw)
}

fn normalize_backend_ir_dump(raw: &str) -> String {
    raw.lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty())
        .filter(|line| !should_strip_build_progress_line(line.trim_start()))
        .map(normalize_cfg_like_ids)
        .collect::<Vec<_>>()
        .join("\n")
}

fn should_strip_build_progress_line(trimmed: &str) -> bool {
    trimmed.starts_with("Finished ")
        || trimmed.starts_with("Running ")
        || trimmed.starts_with("Blocking waiting for file lock")
        || trimmed.starts_with("Compiling ")
        || trimmed.starts_with("[cfg")
        || trimmed.starts_with("[lir")
        || trimmed.starts_with('[')
            && trimmed.contains(" of ")
            && (trimmed.contains("Compiling")
                || trimmed.contains("Cached")
                || trimmed.contains("Linking")
                || trimmed.contains("Checking"))
}

fn normalize_cfg_like_ids(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if matches!(ch, 'b' | 'v' | '%' | '@') {
            out.push(ch);
            let mut saw_digit = false;
            while let Some(next) = chars.peek().copied() {
                if next.is_ascii_digit() {
                    saw_digit = true;
                    chars.next();
                } else {
                    break;
                }
            }
            if saw_digit {
                out.push('N');
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn normalize_debug_ids(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '%' && i + 2 < chars.len() && chars[i + 1] == 't' && chars[i + 2].is_ascii_digit() {
            out.push('%');
            out.push('t');
            out.push('N');
            i += 3;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            continue;
        }

        if chars[i] == '#' && i + 1 < chars.len() && chars[i + 1].is_ascii_digit() {
            out.push('#');
            out.push('N');
            i += 2;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }

    out
}

/// Replace `line <num>:` with `line N:` in Aether drop/dup references.
fn normalize_line_numbers(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut rest = s;

    while let Some(idx) = rest.find("line ") {
        result.push_str(&rest[..idx]);
        result.push_str("line ");
        let after = &rest[idx + 5..];
        // Skip digits
        let digit_end = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        if digit_end > 0 {
            result.push('N');
            rest = &after[digit_end..];
        } else {
            rest = after;
        }
    }

    result.push_str(rest);
    result
}

/// Replace `<sym:NNNNN>#N` with `<sym>#N` in Aether reports.
fn normalize_sym_ids(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut rest = s;

    while let Some(idx) = rest.find("<sym:") {
        result.push_str(&rest[..idx]);
        result.push_str("<sym>");
        let after = &rest[idx + 5..];
        // Skip past the closing >
        if let Some(close) = after.find('>') {
            rest = &after[close + 1..];
        } else {
            rest = after;
        }
    }

    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_repr_dump_strips_title_and_blank_lines() {
        let raw = "\
Flux Backend Representation Contract
===================================

family.array = boxed:array
rule.match_ctor = decode_ctor_only_after_family_proof
";
        assert_eq!(
            normalize_repr_dump(raw),
            "family.array = boxed:array\nrule.match_ctor = decode_ctor_only_after_family_proof"
        );
    }

    #[test]
    fn normalize_cfg_dump_strips_banner_and_ids() {
        let raw = "\
Finished `dev` profile
[cfg→vm] Running via CFG → bytecode VM backend...
fn main [7]
b12(v3):
  %4 = Call @12(v9)
";
        assert_eq!(
            normalize_cfg_dump(raw),
            "fn main [7]\nbN(vN):\n  %N = Call @N(vN)"
        );
    }

    #[test]
    fn strips_vm_banner() {
        let input = "[cfg\u{2192}vm] Running via CFG \u{2192} bytecode VM backend...\n42\n";
        assert_eq!(normalize(input), "42");
    }

    #[test]
    fn strips_llvm_banner() {
        let input = "[lir\u{2192}llvm] Compiling via LIR \u{2192} LLVM native backend...\n42\n";
        assert_eq!(normalize(input), "42");
    }

    #[test]
    fn strips_ascii_banners() {
        let input = "[cfg->vm] Running...\nhello\n";
        assert_eq!(normalize(input), "hello");
    }

    #[test]
    fn strips_cargo_progress() {
        let input = "Compiling flux v0.0.4\nFinished dev\nhello\n";
        assert_eq!(normalize(input), "hello");
    }

    #[test]
    fn normalizes_temp_paths() {
        let input = "error at /tmp/flux_native_abc123/main.ll: bad\n";
        assert_eq!(normalize(input), "error at <TMPDIR>: bad");
    }

    #[test]
    fn preserves_user_output() {
        let input = "hello world\n42\nfoo bar\n";
        assert_eq!(normalize(input), "hello world\n42\nfoo bar");
    }

    #[test]
    fn empty_input() {
        assert_eq!(normalize(""), "");
    }

    #[test]
    fn core_dump_strips_aether_stats() {
        let input = "let x = 42\nAether: 3 dups, 2 drops\nlet y = x\n";
        assert_eq!(normalize_core_dump(input), "let x = 42\nlet y = x");
    }

    #[test]
    fn core_dump_strips_timing() {
        let input = "let x = 42\nCore IR lowered in 0.5ms\n";
        assert_eq!(normalize_core_dump(input), "let x = 42");
    }

    #[test]
    fn core_dump_normalizes_file_paths() {
        let input = "-- source: /home/user/Code/project/test.flx\nlet x = 1\n";
        assert_eq!(normalize_core_dump(input), "-- source: <FILE>\nlet x = 1");
    }

    #[test]
    fn core_dump_preserves_core_ir() {
        let input = "let main = \\() ->\n  PrimOp(Print, [42])\n";
        assert_eq!(
            normalize_core_dump(input),
            "let main = \\() ->\n  PrimOp(Print, [42])"
        );
    }

    #[test]
    fn core_dump_strips_build_progress_noise() {
        let input = "Finished `dev` profile\n[1 of 7] Compiling  Assert\n[2 of 7] Cached     Flow.IO\n[3 of 7] Linking    forward_reference\nlet x = 1\n";
        assert_eq!(normalize_core_dump(input), "let x = 1");
    }

    #[test]
    fn core_dump_normalizes_debug_ids() {
        let input = "letrec main#538 =\nλ.\n    let %t261 = foo#686(#4000931[synthetic]#931)\n";
        assert_eq!(
            normalize_core_dump(input),
            "letrec main#N =\nλ.\n    let %tN = foo#N(#N[synthetic]#N)"
        );
    }

    #[test]
    fn aether_strips_title() {
        let input =
            "Aether Ownership Report\n=======================\n── fn foo ──\n  Dups: 0\n";
        let result = normalize_aether_dump(input);
        assert!(result.starts_with("── fn foo ──"));
        assert!(!result.contains("Aether Memory Model Report"));
        assert!(!result.contains("Aether Ownership Report"));
    }

    #[test]
    fn aether_normalizes_line_numbers() {
        let input = "  - line 8: drop x\n  - line 42: dup y\n";
        let result = normalize_aether_dump(input);
        assert!(result.contains("line N:"));
        assert!(!result.contains("line 8:"));
        assert!(!result.contains("line 42:"));
    }

    #[test]
    fn aether_normalizes_sym_ids() {
        let input = "  - line 5: drop <sym:2000002>#5\n";
        let result = normalize_aether_dump(input);
        assert!(result.contains("<sym>#N"));
        assert!(!result.contains("<sym:2000002>"));
    }

    #[test]
    fn aether_preserves_borrow_signatures() {
        let input = "  borrow signature: [Borrowed, Owned] (Inferred)\n  call sites: none\n";
        let result = normalize_aether_dump(input);
        assert!(result.contains("[Borrowed, Owned] (Inferred)"));
        assert!(result.contains("call sites: none"));
    }

    #[test]
    fn aether_strips_build_progress_noise() {
        let input = "Finished `dev` profile\n[1 of 7] Compiling  Assert\n[2 of 7] Cached     Flow.IO\n[3 of 7] Linking    forward_reference\nAether Ownership Report\n=======================\n── fn foo ──\n";
        assert_eq!(normalize_aether_dump(input), "── fn foo ──");
    }

    #[test]
    fn aether_normalizes_debug_ids() {
        let input = "lam() ->\n  let <sym:2000008>#537 =\n    aether_call[] main#538(#4000790[synthetic]#790)\n";
        assert_eq!(
            normalize_aether_dump(input),
            "lam() ->\n  let <sym>#N =\n    aether_call[] main#N(#N[synthetic]#N)"
        );
    }
}
