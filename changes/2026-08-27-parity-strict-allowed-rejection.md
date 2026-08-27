### Fixed

- Parity check: an allowed strict-mode rejection (strict `CompileError` while the
  non-strict counterpart succeeds) is no longer reported as backend IR/runtime
  divergence. The rejection is excluded from the generic backend comparison —
  without ever being picked as the comparison baseline, so the remaining ways are
  still compared regardless of `--ways` order — and a new strict-vs-strict pass
  requires `vm_strict` and `llvm_strict` to agree with each other: both must
  reject the same programs, with the same diagnostic codes.
