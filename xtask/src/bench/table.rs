//! The benchmarks `cargo xtask bench` knows, the sizes it runs them at, and
//! the groups they belong to.
//!
//! Every program, the Flux one and each port, takes its workload size as
//! its first argument. The smoke size keeps a VM run to a few seconds; the
//! full size (`--full`) gives a native run of 100 ms or more, so process
//! startup is a small part of what is timed, as nofib's fast and slow modes
//! do.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Core,
    Extended,
    /// Programs written with effect handlers, from effect-handlers-bench and
    /// Koka's suite.
    Effects,
}

pub struct Benchmark {
    pub name: &'static str,
    pub title: &'static str,
    /// The `<!-- <tag>-report:start -->` marker in `benchmarks/README.md`.
    pub tag: &'static str,
    pub group: Group,
    pub smoke: &'static str,
    pub full: &'static str,
    /// Whether the program has a direct-style twin,
    /// `benchmarks/flux/{name}_direct.flx`: the same computation without
    /// effect handlers, timed beside it for the effect overhead.
    pub twin: bool,
}

impl Benchmark {
    pub fn flux_program(&self) -> String {
        format!("benchmarks/flux/{}.flx", self.name)
    }

    pub fn flux_twin(&self) -> Option<String> {
        self.twin.then(|| format!("benchmarks/flux/{}_direct.flx", self.name))
    }

    /// The size every program of this benchmark is run with.
    pub fn size(&self, full: bool) -> &'static str {
        if full { self.full } else { self.smoke }
    }

    /// The Haskell sources name a benchmark with dashes.
    pub fn haskell_source(&self) -> String {
        format!("benchmarks/haskell/{}.hs", self.name.replace('_', "-"))
    }

    /// `rbtree_del` is the Elixir module `Bench.RbtreeDel`.
    pub fn elixir_module(&self) -> String {
        self.name
            .split('_')
            .map(|part| {
                let mut chars = part.chars();
                chars.next().map_or(String::new(), |c| c.to_ascii_uppercase().to_string())
                    + chars.as_str()
            })
            .collect()
    }
}

const fn bench(
    name: &'static str,
    title: &'static str,
    tag: &'static str,
    group: Group,
    smoke: &'static str,
    full: &'static str,
) -> Benchmark {
    Benchmark { name, title, tag, group, smoke, full, twin: false }
}

/// An effects benchmark; `twin` says it has a direct-style twin.
const fn effect_bench(
    name: &'static str,
    title: &'static str,
    tag: &'static str,
    twin: bool,
    smoke: &'static str,
    full: &'static str,
) -> Benchmark {
    Benchmark { name, title, tag, group: Group::Effects, smoke, full, twin }
}

// Sizes measured on an Apple M5 Pro: smoke runs take 0.5–2.5 s on the VM,
// full runs 0.1–2 s natively. The programs' own defaults, used when no size
// is given, are the sizes the suite was first written with.
pub const BENCHMARKS: &[Benchmark] = &[
    bench("binarytrees", "Binary Trees", "binarytrees", Group::Core, "12", "21"),
    bench("cfold", "Constant Folding", "cfold", Group::Core, "16", "20"),
    bench("deriv", "Symbolic Differentiation", "deriv", Group::Core, "8", "10"),
    bench("nqueens", "N-Queens", "nqueens", Group::Core, "11", "13"),
    bench("qsort", "Quicksort", "qsort", Group::Core, "48", "96"),
    bench("rbtree_del", "Red-Black Tree Delete", "rbtree-del", Group::Core, "50000", "1000000"),
    bench("nbody", "N-Body", "nbody", Group::Core, "20000", "250000"),
    bench("tak", "Takeuchi", "tak", Group::Core, "27", "33"),
    bench("rbtree_ck", "Red-Black Tree", "rbtree-ck", Group::Extended, "50000", "1000000"),
    bench("rbtree", "Red-Black Tree Insert", "rbtree", Group::Extended, "50000", "1000000"),
    bench(
        "rbtree2",
        "Red-Black Tree Insert BigInt",
        "rbtree2",
        Group::Extended,
        "50000",
        "1000000",
    ),
    bench("sfib", "Wrapping Fibonacci", "sfib", Group::Extended, "5000000", "200000000"),
    // The suite's 200000000 takes about 15 s natively, past the full size's
    // 0.1-2 s.
    effect_bench("countdown", "Countdown", "countdown", true, "2000000", "20000000"),
    effect_bench("iterator", "Iterator", "iterator", true, "2000000", "40000000"),
    // Koka's 100100100 takes about 8 s natively, past the full size's 0.1-2 s.
    effect_bench("counter", "Counter", "counter", true, "2000000", "20000000"),
    // The suite's height of 25 takes about 4.4 s natively, past the full size's
    // 0.1-2 s.
    effect_bench("generator", "Generator", "generator", true, "17", "23"),
    effect_bench("triples", "Triples", "triples", true, "200", "300"),
    effect_bench("tree_explore", "Tree Explore", "tree-explore", true, "11", "16"),
    // No effects: the group's baseline for the cost of a call.
    effect_bench(
        "fibonacci_recursive",
        "Recursive Fibonacci",
        "fibonacci-recursive",
        false,
        "33",
        "42",
    ),
    effect_bench(
        "product_early",
        "Product with Early Exit",
        "product-early",
        true,
        "2000",
        "100000",
    ),
    effect_bench("resume_nontail", "Non-Tail Resume", "resume-nontail", true, "400", "10000"),
    effect_bench("nqueens_choice", "N-Queens by Choice", "nqueens-choice", true, "10", "12"),
];

/// A benchmark's name, or `core`, `extended`, `effects` or `all`.
pub fn select(target: &str) -> Result<Vec<&'static Benchmark>, String> {
    let in_group = |group: Option<Group>| -> Vec<&'static Benchmark> {
        BENCHMARKS.iter().filter(|b| group.is_none_or(|g| b.group == g)).collect()
    };
    match target {
        "core" => Ok(in_group(Some(Group::Core))),
        "extended" => Ok(in_group(Some(Group::Extended))),
        "effects" => Ok(in_group(Some(Group::Effects))),
        "all" => Ok(in_group(None)),
        name => BENCHMARKS.iter().find(|b| b.name == name).map(|b| vec![b]).ok_or_else(|| {
            let names: Vec<&str> = BENCHMARKS.iter().map(|b| b.name).collect();
            format!(
                "unknown benchmark {name}; one of {}, or core, extended, effects, all",
                names.join(", ")
            )
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_hold_every_benchmark_once() {
        let core = select("core").unwrap().len();
        let extended = select("extended").unwrap().len();
        let effects = select("effects").unwrap().len();
        assert_eq!(core + extended + effects, select("all").unwrap().len());
        assert_eq!(core, 8);
        assert!(select("effects").unwrap().iter().all(|b| b.group == Group::Effects));
    }

    #[test]
    fn each_benchmark_has_its_flux_source_and_two_sizes() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for b in BENCHMARKS {
            assert!(root.join(b.flux_program()).is_file(), "{}", b.flux_program());
            if let Some(twin) = b.flux_twin() {
                assert!(root.join(&twin).is_file(), "{twin}");
            }
            let (smoke, full): (u64, u64) = (b.smoke.parse().unwrap(), b.full.parse().unwrap());
            assert!(smoke < full, "{}: the full size is the larger", b.name);
        }
    }

    #[test]
    fn port_names_follow_each_language() {
        let b = select("rbtree_del").unwrap()[0];
        assert_eq!(b.elixir_module(), "RbtreeDel");
        assert_eq!(b.haskell_source(), "benchmarks/haskell/rbtree-del.hs");
        assert_eq!(b.size(false), "50000");
        assert_eq!(select("binarytrees").unwrap()[0].size(true), "21");
    }
}
