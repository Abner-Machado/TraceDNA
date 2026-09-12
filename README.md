# TraceDNA

Freeze a failing run into a small text file, then ask the failure which code it needs.

```
test -> fail -> capture -> capsule -> edit -> replay -> analyze -> result
```

## The problem

A test fails. Two questions follow, and normally they are answered by two different
kinds of work.

1. **How do I get that failure back?** Usually: read the log, guess, add print
   statements, rerun and hope.
2. **Which part of the code is responsible?** Usually: read the diff, form a theory,
   comment things out by hand.

The second question is only answerable once the first one is settled — a bug you cannot
reproduce cannot be localized. So make the reproduction an artifact, and the artifact
becomes the fixed environment for the experiment that answers the second question.

## The idea

A capsule is a handful of `key=value` lines that fully describe one failure:

```
build=javac -d build examples/Payments.java
command=java -cp build Payments
source=examples/Payments.java

seed=42417
input=payment:100
env.CURRENCY=BRL

exit=1
error=no BRL rate for bracket 97
```

`tracedna replay` puts those conditions back and the failure returns. Because the file is
plain text you can edit any field and replay — change the currency, nudge the seed — and
each edit is a hypothesis you just tested.

Then `tracedna analyze` uses the capsule as a fixed environment and varies the *code*
instead: it removes one method at a time, keeps the signature so everything still
compiles, and replays. A method whose removal makes the failure disappear is a method the
failure depends on. The verdict is appended to the same file:

```
needs.bracket=yes
needs.fee=no
needs.format=no
needs.log=no
needs.rate=yes
```

Two methods out of five, named without anyone reading the source.

## Every field answers one question

| Field     | Question                                       |
| --------- | ---------------------------------------------- |
| `build`   | How is the subject made runnable?               |
| `command` | What was executed?                              |
| `source`  | Which file's components can be taken apart?     |
| `seed`    | Under which random draw?                        |
| `input`   | With which input?                               |
| `env.*`   | With which environment?                         |
| `exit`    | What counts as reproducing it?                  |
| `error`   | Which failure, specifically?                    |
| `needs.*` | Which code does that failure require?           |

The first seven are inputs you can edit. The last two are what the tool found out.
Nothing else is stored: a field that cannot change the outcome does not belong here.

## Try it

Needs a Rust toolchain (edition 2021, built with 1.98) and a JDK on `PATH`. The crate uses
the standard library only, so `cargo build` pulls nothing from crates.io.

```bash
git clone https://github.com/Abner-Machado/TraceDNA
cd TraceDNA
./demo.sh
```

`examples/Payments.java` prices a batch of eight payments. The BRL rate table was never
filled in above bracket 96, so the batch fails for roughly one seed in five — the kind of
test that passes when you rerun it.

**Capture.** `subject.capsule` says what to run; `capture` looks for a seed that breaks it.

```
$ tracedna capture subject.capsule
payment 0: amount=100 fee=10
...
no BRL rate for bracket 97

captured after 8 runs -> failure.capsule
```

**Replay.** Same file, same failure, as often as you like.

```
$ tracedna replay failure.capsule
REPRODUCED (exit 1)
```

**Edit.** Change `env.CURRENCY` from `BRL` to `USD` and replay:

```
NOT REPRODUCED (exit 0, the capsule records exit 1)
```

**Analyze.** Put the currency back, then take the code apart under those conditions:

```
$ tracedna analyze failure.capsule
bracket      yes
rate         yes
fee          no
format       no
log          no
```

`bracket` decides which fee bracket a payment lands in and `rate` looks that bracket up —
which is exactly where the bug lives. `fee`, `format` and `log` all run during the
failure and have nothing to do with it.

[`notebook/analysis.ipynb`](notebook/analysis.ipynb) runs the same pipeline and draws the
result.

## How a subject plugs in

The contract is three environment variables, so anything that runs in a shell can be a
subject:

- `SEED` — the run's seed. The program derives its randomness from this instead of the clock.
- `INPUT` — the run's input.
- every `env.*` field from the capsule, exported by name.

`capture` tries up to 200 seeds and stops at the first non-zero exit. `replay` exits `0`
when the recorded failure comes back and `1` when it does not, so a capsule doubles as a
regression check: run it in CI and it tells you the day the bug stops reproducing.

`analyze` is the only part that knows a language. It finds Java methods by scanning for
`<modifier> <type> <name>(…) {` and brace-matching the body, then rewrites `source` in
place for each run and restores it immediately afterwards. An interrupted run leaves
`<source>.tracedna-backup` behind, and the next `analyze` restores from it before starting.

## Limitations

- **Determinism is the subject's job.** TraceDNA hands over a seed; a program that reads
  the clock, hits the network or races threads will not reproduce from one.
- **The oracle is the exit code plus one error line.** A wrong answer that still exits `0`
  is invisible here. If the subject fails silently, `error` is empty and the exit code is
  all that is left, so any failure with the same code counts as the same failure.
- **`analyze` understands Java only**, and only methods whose declaration ends with `{` on
  the same line. Braces inside string literals or comments confuse the scanner.
- **A knockout can preserve a fault by accident.** If the stub value happens to also
  trigger the bug, the method is reported as `no`. The verdict is evidence, not proof.
- **No filesystem or database state is captured.** A failure that depends on what was in a
  table will not come back from these lines.
- **A capsule is executable input.** `build` and `command` are shell lines, so running a
  capsule someone sent you is running their code. Read one before you replay it.
- **`analyze` writes to your source file** while it runs. It works on a file it has backed
  up and restores after every run, but do not point it at a file with unsaved changes.

## Origin

TraceDNA is the merge of two smaller experiments:
[CodeDNA](https://github.com/Abner-Machado/CodeDNA), which removed methods to see which
ones a program needs, and [TraceSeed](https://github.com/Abner-Machado/TraceSeed), which
froze failing runs into editable files. Separately, each answered half a question. The
part that survived from both is the same primitive — run under a controlled variation and
compare — pointed at one target.

## AI Contributors

This project was developed by an independent developer with assistance from:

- OpenAI — ChatGPT
- Anthropic — Claude
- Moonshot AI — Kimi
- Alibaba Cloud — Qwen
- DeepSeek

AI assistance was used for ideation, analysis, implementation, review and experimentation.

## License

MIT
