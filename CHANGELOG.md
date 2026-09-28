# Changelog

## 0.2.0 (2026-09-28)

### Added

- `check=` capsule field. A second shell line that receives the subject's stdout on stdin
  and its exit code in `EXIT`. When a capsule has one, the check's exit code and first
  stderr line are the outcome `capture`, `replay` and `analyze` judge, so a program that
  exits `0` with a wrong answer can be captured and taken apart. Capsules without a check
  behave exactly as in 0.1.0 and render byte for byte the same.
- `tracedna capture <capsule> [output]`. The output path was hardcoded to
  `failure.capsule`, so a second capture in the same directory overwrote the first.
- `examples/Invoice.java`, `examples/check-invoice.sh` and `invoice.capsule`: a bug that
  never crashes. `demo.sh` runs it as steps 7 and 8, and CI runs the demo, so the check
  path is exercised end to end on every push.

### Fixed

- The thin oracle listed in `PROFESSIONAL_AUDIT.md`. Before, a failure had to be a
  non-zero exit, and a wrong result with exit `0` was invisible to every command.
- The check is fed from a separate thread. Writing its stdin from the main thread would
  deadlock on any check that starts printing before it has read all of its input.
- A check can exit with a third code for output it cannot judge. `analyze` already maps
  any code other than `0` or the recorded one to `inconclusive`, so knocking out the
  method that prints the result no longer shows up as a false `yes`.
- `PROFESSIONAL_AUDIT.md` had stale numbers: the line count said about 300 when it was
  already over 400, and the file count predated the second example. Both corrected, and
  the verdict now names what still blocks production (`analyze` editing the source in
  place) instead of the oracle.

### Tests

Ten unit tests, up from eight: a capsule with a check survives a round trip and a capsule
without one never gains a `check=` line; and a check that fails overrides a subject that
exited `0`. Neither needs a shell, so both run on the Windows CI job too.

## 0.1.0 (2026-08-26)

First release: `capture`, `replay`, `analyze`, Java method knockout.
