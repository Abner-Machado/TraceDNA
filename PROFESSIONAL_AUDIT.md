# Professional audit

An honest reading of what this repository is, as of the first release. Nothing is marked
missing because professional projects usually have it — only where this project has a
concrete reason to need it.

Legend: **PROFESSIONAL** (done, no reservations) · **ENOUGH FOR AN MVP** (works, with a
named boundary) · **MISSING FOR PRODUCTION** (a real gap with a concrete reason) ·
**NOT NEEDED** (would be ornament here).

| Area | Verdict | Reasoning |
| --- | --- | --- |
| Architecture | PROFESSIONAL | One binary, three verbs, one data format. `capture`, `replay` and `analyze` all reduce to "run a shell line under the conditions this capsule describes". There are no layers because there is nothing for a layer to separate. |
| CLI | ENOUGH FOR AN MVP | Three subcommands, a usage message on anything else, and meaningful exit codes (`replay` exits `1` when the failure does not come back, which is what makes a capsule usable in CI). `--help` and `--version` fall through to the usage text and exit `1` rather than `0`. |
| Error handling | PROFESSIONAL | Every fallible path returns `Result<_, String>` and surfaces through one printer in `main`. No `unwrap` on I/O. Capsules are hand-edited, so `parse` rejects a malformed number or an unknown field with the line number instead of silently coercing it — `seed=abc` used to become `seed=0` and quietly change the verdict. |
| Tests | ENOUGH FOR AN MVP | Eight unit tests cover the parts where a bug would be silent: capsule round-trip, comment handling, typo rejection, method detection, annotated declarations, knockout rewriting, and both halves of the reproduction predicate. They need no JDK, so they run anywhere. The `analyze` backup/restore path is covered by CI running the demo, not by a unit test. |
| Reproducibility | PROFESSIONAL | It is the product, and CI proves it: every push runs the full capture → replay → edit → analyze demo on a clean machine. |
| Artifact persistence | PROFESSIONAL | A plain file the user owns. Written atomically enough for its size, regenerated on demand, never hidden in a cache directory. |
| Data format | ENOUGH FOR AN MVP | `key=value`, one field per line, map keys sorted so diffs stay stable. A value cannot contain a newline, which is why only the first stderr line is recorded. That is a real ceiling; it has not been hit yet. |
| Versioning | ENOUGH FOR AN MVP | `0.1.0` in `Cargo.toml`, and the capsule format is small enough to read in full. A changelog for a first release would document nothing. |
| Cross-platform | MISSING FOR PRODUCTION | Everything runs through `sh -c`. On Windows that means Git Bash or WSL. The reason to care is concrete: the tool's whole audience is CI and developer machines, and Windows CI runners are part of that. |
| Security | MISSING FOR PRODUCTION | A capsule is executable input: `build` and `command` are shell lines, so running a capsule from an untrusted source is running that source's code. Fine for a file you wrote, not fine for one attached to a bug report by a stranger. Documented in the README; a real fix means refusing to execute a capsule the user has not reviewed. |
| Determinism | ENOUGH FOR AN MVP | The tool is deterministic given a capsule. `capture` is deliberately not — it searches. Determinism of the subject is the subject's responsibility, which is the honest place for it, and the README says so. |
| Documentation | PROFESSIONAL | README states the problem before the solution, every field is mapped to the question it answers, the demo output is real, and the limitations are listed rather than implied. |
| Installation | ENOUGH FOR AN MVP | `cargo build --release`, no dependencies to resolve. No published binaries and no `cargo install` path, which matters only once someone outside this repo wants it. |
| Repository structure | PROFESSIONAL | Eleven files. Source, one example, one demo, one notebook, docs, CI. Nothing exists that the concept does not need. |
| CI | PROFESSIONAL | `clippy -D warnings`, `cargo test`, then the demo end to end. It fails if the idea stops working, not just if the code stops compiling. |
| Code quality | PROFESSIONAL | ~300 lines including tests, zero dependencies, clippy clean at `-D warnings`. |
| Licensing | PROFESSIONAL | MIT, declared both in `LICENSE` and in `Cargo.toml`. |
| Observability | NOT NEEDED | The artifact is the observability. Adding log levels to a 300-line CLI whose output is already the record would be ornament. |

## Known weaknesses, stated plainly

- **The oracle is thin.** A failure is "this exit code plus this stderr line". A subject
  that returns a wrong answer and exits `0` is invisible to every part of this tool, and a
  subject that fails without writing to stderr degrades the oracle to the exit code alone —
  every failure sharing that code then looks like the same failure.
- **`analyze` rewrites your source file.** It backs the file up, restores it after each
  run, and recovers from an interrupted run on the next start — but for the duration of
  the analysis, the file on disk is not the file you wrote.
- **A knockout can preserve the fault by accident.** If the stub return value also
  triggers the bug, the method is reported as `no`. The verdict is evidence, not proof.
- **The Java scanner is a line matcher.** A declaration split across lines is skipped, and
  braces inside string literals confuse it. Annotations are handled, but an annotation
  carrying arguments on the declaration line (`@SuppressWarnings("x") void f() {`) is not.

## External review

The Rust core was reviewed adversarially by a second model (GPT-5.5, run headless through
the local producer fleet) before release. Eight findings came back; each was checked
against the code rather than accepted.

- **Applied:** `reproduces` matched the recorded error as a substring, so a short `error`
  value passed against any longer message that contained it — now matched as a whole line.
  `signature` rejected annotated declarations such as `@Deprecated public int old()`, which
  is common enough in real Java to matter — annotations are now skipped.
- **Already fixed** before the review landed: silent coercion of a malformed `seed`/`exit`.
- **Already documented:** braces inside string literals confusing the scanner; a failed
  restore leaving the source knocked out until the next run recovers it from the backup.
- **Rejected as incorrect:** a claimed panic in `knockout` from a failed `body_end` —
  `signature` propagates that `None` and skips the method, so the case never reaches
  `knockout`; and a claimed parse failure on generic return types — `split_whitespace`
  keeps `List<String>` whole, and the spaced form still resolves to a reference stub that
  compiles.

## Verdict

**Can it be published as an open-source MVP?** YES. It does one thing, the demo is real,
the tests pass, and the boundaries are written down.

**Can it be presented professionally?** YES. The architecture matches the problem, CI
proves the behaviour on a clean machine, and nothing in the repository is decoration.

**Can it be used in production?** NO. Two blockers: the oracle is too thin to trust for
anything beyond crash-shaped failures, and `analyze` mutating a real source tree is not
acceptable against a working copy someone else is editing.

**Main deficiency:** the failure oracle. Exit code plus one stderr line makes every
verdict — `REPRODUCED`, `needs.x=yes` — only as sharp as the program's crash message.

**Biggest technical risk:** `analyze` writing to `source` in place. The backup and restore
work, and an interrupted run is recoverable, but the failure mode is someone's source file,
which is the worst possible thing to be holding when the process dies.

**Highest-impact next improvement:** let a capsule name a test command and use its exit
status per test case as the oracle, instead of scraping stderr. It removes the thin-oracle
deficiency, makes `needs.*` verdicts sharper, and costs one field.
