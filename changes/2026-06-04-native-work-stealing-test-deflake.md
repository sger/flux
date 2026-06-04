### Fixed
- De-flake `native_work_stealing_tests::stolen_fiber_runs_parameterized_handler`:
  the test asserts a stolen `fast` fiber wins a `race` against a `slow` one, but the
  20ms-vs-1ms margin could collapse under CI load or coarse timer resolution (a 1ms
  sleep can round up toward 10-15ms), letting `slow` win and print `99` instead of
  `6`. Widened the loser's sleep to 200ms — `race` cancels the loser on first
  completion, so this adds headroom without adding test runtime.
