use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct ParityFixtureCase {
    pub path: &'static str,
    pub roots: &'static [&'static str],
    pub strict: bool,
    pub expect_compile_error: bool,
    pub category: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticTuple {
    pub code: String,
    pub title: String,
    pub primary_label: String,
}

#[derive(Debug, Clone)]
pub struct CommandOutcome {
    pub command: String,
    pub exit_code: i32,
    pub normalized_output: String,
    pub tuples: Vec<DiagnosticTuple>,
}

pub fn curated_cases() -> Vec<ParityFixtureCase> {
    vec![
        // A: direct effects / propagation
        ParityFixtureCase {
            path: "examples/type_system/failing/35_pure_context_typed_pure_rejects_io.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "A",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/36_pure_context_time_only_rejects_io.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "A",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/37_pure_context_unannotated_infers_io_then_rejects_time_caller.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "A",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/33_module_qualified_effect_propagation_missing.flx",
            roots: &["examples/type_system"],
            strict: false,
            expect_compile_error: true,
            category: "A",
        },
        // B: handle/perform
        ParityFixtureCase {
            path: "examples/type_system/failing/17_handle_unknown_operation.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "B",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/18_handle_incomplete_operation_set.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "B",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/42_handle_unknown_effect.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "B",
        },
        ParityFixtureCase {
            path: "examples/type_system/22_handle_discharges_effect.flx",
            roots: &[],
            strict: false,
            expect_compile_error: false,
            category: "B",
        },
        // C: effect polymorphism
        ParityFixtureCase {
            path: "examples/type_system/30_effect_poly_hof_nested_ok.flx",
            roots: &[],
            strict: false,
            expect_compile_error: false,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/44_effect_poly_hof_nested_missing_effect.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/45_effect_row_subtract_missing_io.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/100_effect_row_order_equivalence_ok.flx",
            roots: &[],
            strict: false,
            expect_compile_error: false,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/101_effect_row_subtract_concrete_ok.flx",
            roots: &[],
            strict: false,
            expect_compile_error: false,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/102_effect_row_subtract_var_satisfied_ok.flx",
            roots: &[],
            strict: false,
            expect_compile_error: false,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/103_effect_row_multivar_disambiguated_ok.flx",
            roots: &[],
            strict: false,
            expect_compile_error: false,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/104_effect_row_absent_ordering_linked_ok.flx",
            roots: &[],
            strict: false,
            expect_compile_error: false,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/194_effect_row_multi_missing_deterministic_e400.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/195_effect_row_invalid_subtract_e421.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/196_effect_row_subtract_unresolved_single_e419.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/197_effect_row_subtract_unresolved_multi_e420.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/198_effect_row_subset_unsatisfied_e422.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/199_effect_row_subset_ordered_missing_e422.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "C",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/200_effect_row_absent_ordering_linked_violation_e421.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "C",
        },
        // D: entry policy
        ParityFixtureCase {
            path: "examples/type_system/failing/38_top_level_effect_rejected.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "D",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/43_main_unhandled_custom_effect.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "D",
        },
        ParityFixtureCase {
            path: "examples/type_system/29_main_handles_custom_effect.flx",
            roots: &[],
            strict: false,
            expect_compile_error: false,
            category: "D",
        },
        // E/F: strict + public boundary
        ParityFixtureCase {
            path: "examples/type_system/failing/29_strict_missing_main.flx",
            roots: &[],
            strict: true,
            expect_compile_error: true,
            category: "E",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/57_strict_entry_path_parity.flx",
            roots: &[],
            strict: true,
            expect_compile_error: true,
            category: "E",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/58_strict_public_underscore_missing_annotation.flx",
            roots: &[],
            strict: true,
            expect_compile_error: true,
            category: "F",
        },
        ParityFixtureCase {
            path: "examples/type_system/61_strict_module_private_unannotated_allowed.flx",
            roots: &["examples/type_system"],
            strict: true,
            expect_compile_error: false,
            category: "F",
        },
        // H: HM + ADT hardening
        ParityFixtureCase {
            path: "examples/type_system/failing/64_hm_inferred_call_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/65_adt_nested_constructor_non_exhaustive.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/66_module_constructor_not_public_api.flx",
            roots: &["examples/type_system"],
            strict: true,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/67_adt_multi_arity_nested_non_exhaustive.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/68_adt_nested_list_non_exhaustive.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/69_hm_typed_let_infix_compile_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/70_hm_prefix_non_numeric_compile_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/71_hm_if_known_type_compile_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/72_hm_match_known_type_compile_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/73_hm_index_non_int_compile_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/74_hm_index_non_indexable_compile_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/75_hm_if_non_bool_condition_compile_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/76_hm_match_guard_non_bool_compile_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/77_hm_logical_non_bool_compile_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/79_hm_module_generic_call_mismatch.flx",
            roots: &["examples/type_system"],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/85_hm_function_effect_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/86_hm_mixed_numeric_typed_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
        ParityFixtureCase {
            path: "examples/type_system/failing/87_hm_pattern_binding_constrained_mismatch.flx",
            roots: &[],
            strict: false,
            expect_compile_error: true,
            category: "H",
        },
    ]
}

pub fn snapshot_name(case: &ParityFixtureCase) -> String {
    let basename = Path::new(case.path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown");
    format!("purity_parity__{}__{}", case.category, basename)
}

pub fn run_case(
    workspace_root: &Path,
    flux_bin: &Path,
    case: &ParityFixtureCase,
    jit: bool,
) -> Result<CommandOutcome, String> {
    let mut args: Vec<String> = vec!["--no-cache".to_string()];
    if case.strict {
        args.push("--strict".to_string());
    }
    for root in case.roots {
        args.push("--root".to_string());
        args.push((*root).to_string());
    }
    args.push(case.path.to_string());
    if jit {
        args.push("--jit".to_string());
    }

    let output = Command::new(flux_bin)
        .args(&args)
        .env("NO_COLOR", "1")
        .output()
        .map_err(|e| format!("failed to run flux for `{}`: {e}", case.path))?;

    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    combined.push_str(&String::from_utf8_lossy(&output.stderr));

    let normalized = normalize_output(&combined, workspace_root);
    let tuples = parse_diagnostic_tuples(&normalized);

    let command = format!(
        "$ flux {}",
        args.iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("\"{}\"", a)
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    );

    Ok(CommandOutcome {
        command,
        exit_code: output.status.code().unwrap_or(-1),
        normalized_output: normalized,
        tuples,
    })
}

pub fn parity_transcript(
    case: &ParityFixtureCase,
    vm: &CommandOutcome,
    jit: &CommandOutcome,
) -> String {
    let vm_tuples = format_tuples(&vm.tuples);
    let jit_tuples = format_tuples(&jit.tuples);

    let mismatch = if vm.tuples == jit.tuples {
        String::from("<none>")
    } else {
        format_mismatch_debug(
            &vm.tuples,
            &jit.tuples,
            &vm.normalized_output,
            &jit.normalized_output,
        )
    };

    format!(
        "Fixture: {}\nCategory: {}\nStrict: {}\nExpect compile error: {}\n\n== vm command ==\n{}\nexit_code: {}\n\n== jit command ==\n{}\nexit_code: {}\n\n== vm tuples ==\n{}\n\n== jit tuples ==\n{}\n\n== parity ==\n{}\n\n== mismatch_debug ==\n{}\n",
        case.path,
        case.category,
        case.strict,
        case.expect_compile_error,
        vm.command,
        vm.exit_code,
        jit.command,
        jit.exit_code,
        vm_tuples,
        jit_tuples,
        if vm.tuples == jit.tuples {
            "match"
        } else {
            "mismatch"
        },
        mismatch,
    )
}

fn format_tuples(tuples: &[DiagnosticTuple]) -> String {
    if tuples.is_empty() {
        return String::from("<none>");
    }
    tuples
        .iter()
        .map(|t| format!("- {} | {} | {}", t.code, t.title, t.primary_label))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_mismatch_debug(
    vm: &[DiagnosticTuple],
    jit: &[DiagnosticTuple],
    vm_output: &str,
    jit_output: &str,
) -> String {
    let vm_only = vm
        .iter()
        .filter(|t| !jit.contains(t))
        .map(|t| format!("- {} | {} | {}", t.code, t.title, t.primary_label))
        .collect::<Vec<_>>();
    let jit_only = jit
        .iter()
        .filter(|t| !vm.contains(t))
        .map(|t| format!("- {} | {} | {}", t.code, t.title, t.primary_label))
        .collect::<Vec<_>>();

    format!(
        "vm_only:\n{}\n\njit_only:\n{}\n\nvm_output:\n{}\n\njit_output:\n{}",
        if vm_only.is_empty() {
            "<none>".to_string()
        } else {
            vm_only.join("\n")
        },
        if jit_only.is_empty() {
            "<none>".to_string()
        } else {
            jit_only.join("\n")
        },
        truncate(vm_output, 4000),
        truncate(jit_output, 4000),
    )
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}\n... <truncated>", &s[..max])
    }
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }

    out
}

fn normalize_output(output: &str, workspace_root: &Path) -> String {
    let mut normalized = output.replace("\r\n", "\n").replace('\\', "/");
    normalized = strip_ansi(&normalized);

    let mut prefixes = vec![workspace_root.to_string_lossy().replace('\\', "/")];
    if let Ok(canonical) = workspace_root.canonicalize() {
        prefixes.push(canonical.to_string_lossy().replace('\\', "/"));
    }

    for prefix in prefixes {
        if prefix.is_empty() {
            continue;
        }
        let with_slash = format!("{prefix}/");
        normalized = normalized.replace(&with_slash, "");
        normalized = normalized.replace(&prefix, "");
    }

    let mut cleaned = String::new();
    for line in normalized.lines() {
        if line.starts_with("Finished `") || line.starts_with("Running `") {
            continue;
        }
        cleaned.push_str(line);
        cleaned.push('\n');
    }

    cleaned
}

fn parse_diagnostic_tuples(output: &str) -> Vec<DiagnosticTuple> {
    let lines: Vec<&str> = output.lines().collect();
    let mut i = 0;
    let mut tuples = Vec::new();

    while i < lines.len() {
        let line = lines[i].trim();
        if let Some((code, title)) = parse_diagnostic_header(line) {
            let mut primary_label = String::new();
            let mut j = i + 1;
            while j < lines.len() {
                let block_line = lines[j].trim();
                if parse_diagnostic_header(block_line).is_some() {
                    break;
                }
                if primary_label.is_empty()
                    && let Some(label) = parse_primary_label(lines[j])
                {
                    primary_label = label;
                }
                j += 1;
            }

            tuples.push(DiagnosticTuple {
                code,
                title,
                primary_label,
            });
            i = j;
            continue;
        }
        i += 1;
    }

    tuples
}

fn parse_diagnostic_header(line: &str) -> Option<(String, String)> {
    for marker in [
        "error[",
        "error[",
        "Warning[",
        "Note[",
        "Help[",
        "compiler error[",
        "compiler warning[",
        "warning[",
    ] {
        if let Some(marker_idx) = line.find(marker) {
            let code_start = marker_idx + marker.len();
            let code_end_rel = line[code_start..].find(']')?;
            let code_end = code_start + code_end_rel;
            let code = line[code_start..code_end].trim().to_string();

            let title_marker = "]:";
            let title_start = line[code_end..].find(title_marker)? + code_end + title_marker.len();
            let title = line[title_start..].trim().to_string();
            return Some((code, title));
        }
    }
    None
}

fn parse_primary_label(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('|') {
        return None;
    }

    let after_bar = trimmed.trim_start_matches('|').trim();
    if after_bar.is_empty() {
        return None;
    }

    // skip pure pointers like "^^^^" or "-----"
    if after_bar
        .chars()
        .all(|c| c == '^' || c == '-' || c == '~' || c == ' ')
    {
        return None;
    }

    let cleaned = after_bar
        .trim_start_matches(['^', '-', '~', ' '])
        .trim()
        .to_string();

    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}
