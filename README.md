# Flux

Flux is an experimental pure functional language written in Rust with two execution backends: a stack-based **bytecode VM** and an **LLVM native backend**. It started as a learning project for compiler construction. It features Hindley-Milner type inference, algebraic effects with row-polymorphic effect types, and familiar brace-style syntax. It is inspired by [Haskell](https://www.haskell.org/) (purity, type inference), [Koka](https://koka-lang.github.io/koka/doc/index.html) (algebraic effects, effect rows), [Elm](https://elm-lang.org/) (human-friendly errors), and [Rust](https://www.rust-lang.org/) (syntax, tooling).

## Building LLVM for Flux on Windows

Flux's LLVM backend needs LLVM command-line tools such as `llc`, `opt`, `llvm-config`, and `llvm-nm`.

Use **x64 Native Tools Command Prompt for VS 2026**. The commands below assume Visual Studio 2026, CMake, Ninja, Python, and Git are already installed.

### Build latest stable LLVM

```bat
cd /d E:\Github

git clone --depth 1 --branch llvmorg-22.1.5 https://github.com/llvm/llvm-project.git llvm-project-22
cd llvm-project-22

cmake -S llvm -B build -G Ninja -DCMAKE_BUILD_TYPE=Release -DLLVM_TARGETS_TO_BUILD="X86"

cmake --build build --target llc opt llvm-config llvm-nm
```

### Use this LLVM build in the current terminal

```bat
set PATH=E:\Github\llvm-project-22\build\bin;%PATH%
```

Verify:

```bat
where llc
llc --version
opt --version
llvm-config --version
llvm-nm --version
```

The first `where llc` result should be:

```text
E:\Github\llvm-project-22\build\bin\llc.exe
```

### Test Flux

```bat
cd /d E:\Github\flux
cargo run --features llvm -- --native examples\guide\variables.flx
```

For a broader VM/LLVM comparison:

```bat
cargo run --features llvm -- parity-check tests\parity --ways vm,llvm
```
