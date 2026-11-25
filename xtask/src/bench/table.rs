//! The benchmarks `cargo xtask bench` knows, and the groups they belong to.

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Core,
    Extended,
}

/// A smaller and a larger input, for a benchmark whose default run is short.
pub struct Workload {
    pub smoke_flux: &'static str,
    pub full_flux: &'static str,
    /// The argument every other port takes.
    pub smoke_arg: &'static str,
    pub full_arg: &'static str,
}

pub struct Benchmark {
    pub name: &'static str,
    pub title: &'static str,
    /// The `<!-- <tag>-report:start -->` marker in `benchmarks/README.md`.
    pub tag: &'static str,
    pub group: Group,
    pub workload: Option<Workload>,
}

impl Benchmark {
    pub fn flux_program(&self, full: bool) -> String {
        match &self.workload {
            Some(w) if full => w.full_flux.to_string(),
            Some(w) => w.smoke_flux.to_string(),
            None => format!("benchmarks/flux/{}.flx", self.name),
        }
    }

    /// The argument the other ports take, if this benchmark has one.
    pub fn port_arg(&self, full: bool) -> Option<&'static str> {
        self.workload.as_ref().map(|w| if full { w.full_arg } else { w.smoke_arg })
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

const fn plain(
    name: &'static str,
    title: &'static str,
    tag: &'static str,
    group: Group,
) -> Benchmark {
    Benchmark { name, title, tag, group, workload: None }
}

pub const BENCHMARKS: &[Benchmark] = &[
    Benchmark {
        name: "binarytrees",
        title: "Binary Trees",
        tag: "binarytrees",
        group: Group::Core,
        workload: Some(Workload {
            smoke_flux: "benchmarks/flux/binarytrees_smoke.flx",
            full_flux: "benchmarks/flux/binarytrees.flx",
            smoke_arg: "8",
            full_arg: "21",
        }),
    },
    plain("cfold", "Constant Folding", "cfold", Group::Core),
    plain("deriv", "Symbolic Differentiation", "deriv", Group::Core),
    plain("nqueens", "N-Queens", "nqueens", Group::Core),
    plain("qsort", "Quicksort", "qsort", Group::Core),
    plain("rbtree_del", "Red-Black Tree Delete", "rbtree-del", Group::Core),
    plain("nbody", "N-Body", "nbody", Group::Core),
    plain("tak", "Takeuchi", "tak", Group::Core),
    plain("rbtree_ck", "Red-Black Tree", "rbtree-ck", Group::Extended),
    plain("rbtree", "Red-Black Tree Insert", "rbtree", Group::Extended),
    plain("rbtree2", "Red-Black Tree Insert BigInt", "rbtree2", Group::Extended),
    plain("sfib", "Wrapping Fibonacci", "sfib", Group::Extended),
];

/// A benchmark's name, or `core`, `extended` or `all`.
pub fn select(target: &str) -> Result<Vec<&'static Benchmark>, String> {
    let in_group = |group: Option<Group>| -> Vec<&'static Benchmark> {
        BENCHMARKS.iter().filter(|b| group.is_none_or(|g| b.group == g)).collect()
    };
    match target {
        "core" => Ok(in_group(Some(Group::Core))),
        "extended" => Ok(in_group(Some(Group::Extended))),
        "all" => Ok(in_group(None)),
        name => BENCHMARKS.iter().find(|b| b.name == name).map(|b| vec![b]).ok_or_else(|| {
            let names: Vec<&str> = BENCHMARKS.iter().map(|b| b.name).collect();
            format!("unknown benchmark {name}; one of {}, or core, extended, all", names.join(", "))
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
        assert_eq!(core + extended, select("all").unwrap().len());
        assert_eq!(core, 8);
    }

    #[test]
    fn each_benchmark_has_its_flux_source() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        for b in BENCHMARKS {
            for full in [false, true] {
                assert!(root.join(b.flux_program(full)).is_file(), "{}", b.flux_program(full));
            }
        }
    }

    #[test]
    fn port_names_follow_each_language() {
        let b = select("rbtree_del").unwrap()[0];
        assert_eq!(b.elixir_module(), "RbtreeDel");
        assert_eq!(b.haskell_source(), "benchmarks/haskell/rbtree-del.hs");
        assert_eq!(b.port_arg(true), None);
        assert_eq!(select("binarytrees").unwrap()[0].port_arg(true), Some("21"));
    }
}
