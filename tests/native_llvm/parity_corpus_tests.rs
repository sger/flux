//! The `tests/parity` corpus: each fixture runs the ways its header names
//! (`vm, llvm` unless it says otherwise) and the results must agree with each
//! other and with any expected output the fixture pins, exactly as
//! `flux parity-check tests/parity` checks them.
//!
//! Most of these fixtures have no Rust test of their own, so before this file
//! only the release gate ran them.

#![cfg(feature = "llvm")]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use flux::parity::Verdict;
use flux::parity::cli::{check_fixture, fixtures_in};
use flux::parity::report::{DisplayFilter, print_result};

/// The corpus is split so that no one test carries all of it: each part is
/// every `PARTS`-th fixture, so a new fixture lands in some part on its own.
const PARTS: usize = 8;

fn corpus_part(part: usize) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let files: Vec<PathBuf> = fixtures_in(&root.join("tests/parity"))
        .into_iter()
        .enumerate()
        .filter(|(index, _)| index % PARTS == part)
        .map(|(_, file)| file)
        .collect();
    assert!(!files.is_empty(), "no parity fixtures in part {part}");

    // CI runners have three cores; more builds there slow the timed tests
    // running beside these.
    let workers = if std::env::var_os("CI").is_some() {
        1
    } else {
        2
    };
    let flux = Path::new(env!("CARGO_BIN_EXE_flux"));
    let next = AtomicUsize::new(0);
    let passed = AtomicUsize::new(0);
    let mut failures: Vec<String> = std::thread::scope(|threads| {
        let handles: Vec<_> = (0..workers.min(files.len()))
            .map(|_| {
                threads.spawn(|| {
                    let mut failed = Vec::new();
                    while let Some(file) = files.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let result = check_fixture(file, flux);
                        if matches!(result.verdict, Verdict::Pass) {
                            passed.fetch_add(1, Ordering::Relaxed);
                        }
                        if matches!(
                            result.verdict,
                            Verdict::Mismatch { .. } | Verdict::ExpectedOutputMismatch { .. }
                        ) {
                            print_result(&result, DisplayFilter::FailOnly, false);
                            let rel = file.strip_prefix(root).unwrap_or(file);
                            failed.push(rel.display().to_string());
                        }
                    }
                    failed
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| handle.join().expect("a fixture worker"))
            .collect()
    });
    failures.sort();
    // As `parity-check` does: a part where every fixture was skipped has
    // checked nothing.
    assert!(
        passed.into_inner() > 0,
        "no fixture in part {part} ran: all were skipped"
    );
    assert!(
        failures.is_empty(),
        "these fixtures disagree (details above); rerun one with\n  \
         cargo run --features llvm -- parity-check <file>\n{}",
        failures.join("\n")
    );
}

#[test]
fn parity_fixtures_run_the_same_natively_part_1() {
    corpus_part(0)
}

#[test]
fn parity_fixtures_run_the_same_natively_part_2() {
    corpus_part(1)
}

#[test]
fn parity_fixtures_run_the_same_natively_part_3() {
    corpus_part(2)
}

#[test]
fn parity_fixtures_run_the_same_natively_part_4() {
    corpus_part(3)
}

#[test]
fn parity_fixtures_run_the_same_natively_part_5() {
    corpus_part(4)
}

#[test]
fn parity_fixtures_run_the_same_natively_part_6() {
    corpus_part(5)
}

#[test]
fn parity_fixtures_run_the_same_natively_part_7() {
    corpus_part(6)
}

#[test]
fn parity_fixtures_run_the_same_natively_part_8() {
    corpus_part(7)
}
