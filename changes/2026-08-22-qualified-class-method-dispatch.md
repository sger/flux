### Fixed

- A qualified call `Module.f(..)` now reaches the module's own function when
  `f` merely shares a name with a type-class method. Previously any qualified
  call whose member name matched a class method was routed to that class, so
  `Flow.Stream.append` was unreachable — even fully qualified — failing with
  `error[E444]: No instance for Semigroup<Stream<Int>>`. There was no
  workaround: the function could not be called at all.

  A qualified call dispatches as a class method only when the qualifier names
  the class, which is how the legitimate uses are written
  (`Foldable.fold`, `Comparable.same`, `Matchable.same`). Matching on the
  class's declaring module does not work as a rule: an instance may live in a
  different module from its class, and the qualifier at the call site is an
  import alias rather than a path.

  `Flow.Stream.append` is now callable and covered, including a test that it
  stays lazy when the right-hand stream is infinite.
