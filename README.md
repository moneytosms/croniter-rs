# croniter-rs

[![CI](https://github.com/moneytosms/croniter-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/moneytosms/croniter-rs/actions/workflows/ci.yml)

A Rust port of [`pallets-eco/croniter`](https://github.com/pallets-eco/croniter), built for
[Port Mortem 2026](https://coderesurrection.com/2026/), Track D (Python → Rust).

Source pinned at commit `f64665e`. 1,586 lines of Python library against 4,755 lines of
test suite.

| | |
| --- | --- |
| Original test suite, unmodified | **248 / 248** + 92 subtests |
| Golden-corpus equivalence | **15,827 / 15,827** (100.00%) |
| Differential fuzzing | **0 divergences** in 14,610 logged cases |
| Property tests | **88,000 generated cases** |
| `unsafe` blocks | **0** |
| `unwrap`/`panic` outside tests | **0** |

## Why this repo

croniter answers one question well: given a cron expression and an instant, when does it
next fire, and when did it last fire. It is load-bearing in a lot of Python schedulers,
and its behaviour is pinned by an unusually large test suite relative to its size, which
is exactly what a behavioural-equivalence port wants. It also carries fifteen years of
accumulated edge cases — DST folds, the vixie-cron day-of-month/day-of-week bug, `L`/`W`/`#`
syntax, hash-scheduled expressions — that a naive rewrite gets wrong silently rather than
loudly. Those quirks are the interesting part, so this port reproduces them rather than
correcting them.

## Build

```sh
make build
```

That is `cargo build --release` behind a pinned toolchain (`rust-toolchain.toml`, Rust
1.97.1, edition 2024). It needs no Python, no system libraries, and no environment setup.

Or, with nothing installed but Docker — this builds the port and runs the original Python
suite against it:

```sh
docker build -t croniter-rs . && docker run --rm croniter-rs
```

## Test

```sh
make test           # native Rust: unit + property + regression tests, golden-corpus replay
make test-original  # the original Python suite, live, against the port
make demo           # the scripted walkthrough, every figure computed live
```

`make test` is the one that must be green, and it needs nothing but Rust: the golden corpus
is committed data, so behavioural equivalence is checked with no Python on the machine.

`make corpus`, `make fuzz` and `make bench-compare` regenerate the artifacts; each fetches
the pinned upstream commit into a gitignored scratch directory if it isn't already there.

## How this port is verified

Port Mortem Rule 05 forbids the port linking or FFI-ing into the Python runtime, so there
is no PyO3 here and no embedded interpreter. Correctness is established two ways.

**Golden corpus (primary).** `tools/extract_corpus/` runs the original Python suite once,
offline, under instrumentation that records every call it makes — expression, start
instant, timezone, every constructor keyword — together with what Python returned or
raised. The result is `tests/port/corpus.json`. `tests/corpus_replay.rs` replays it in
pure Rust. Python is a build-time data source and never a runtime dependency.

> **15,827 / 15,827 records match (100.00%).**
>
> Of the 15,840 records extracted, 13 are excluded *by name* and reported separately by the
> test rather than counted as passes: 10 use croniter's `R` random syntax and have no stable
> answer to check against (§12), and 3 are calls the Rust API cannot express at all — a bad
> `ret_type`, a `dict` as `hash_id`, mismatched `croniter_range` bound types (§15).

**Conformance bridge (secondary).** `tools/bridge/croniter/` is a pure-Python shim that
satisfies `import croniter` and forwards each call to a long-lived `croniter-conformance`
subprocess over line-delimited JSON. This runs the byte-identical original test suite
against the port. Python calls the port as an external process; the port contains no
Python. See `DECISIONS.md` §1 for the full reasoning.

> **248 / 248 tests pass**, plus all 92 subtests — ten consecutive runs, no flakes.
>
> This needed a handle protocol: the shim parses once in `__init__` and the engine holds
> that parse for the object's lifetime. croniter draws an `R` expression's random values
> during construction and reuses them, so re-expanding per call made consecutive
> `get_next()`s wander instead of advance. See §16.

**Differential fuzzing (continuous).** `fuzz/harness.py` generates random expressions,
start instants and DST-boundary cases, runs each against both the pinned Python and the
port, and compares. The committed log (`fuzz/log.txt`) is the seed-11 run:
**14,610 cases, 0 divergences, 0 warnings**, 149.5 cases/s. Seeds 12, 13 and 21 were run
the same way and also came back clean; only the last run's log is committed, and its
header records the seed and the port commit it ran against. CI runs a fresh 90-second
seed on every push and fails the build on any divergence.

**Property tests (independent).** The three checks above all measure agreement *with
Python*, which says nothing about a bug both share. `tests/properties.rs` asserts what a
cron iterator must satisfy on its own terms — `next` advances and lands on a fire, skips
nothing, round-trips through `prev`, and a long walk never stalls — against ordinary
expressions and again against `L`/`W`/`#`. **88,000 generated cases, all passing** (§18).

`tests/original/` is a verbatim copy of the upstream suite. `tests/original/HASHES.txt`
carries per-file SHA-256, and `.port-mortem.toml` records the aggregate. Nothing in it was
edited, and CI verifies the hashes on every push.

## Deliverables

| # | Deliverable | Where |
| --- | --- | --- |
| 1 | Public repo, OSI licence | this repo, MIT (inherited) — [`LICENSE`](./LICENSE) |
| 2 | One-command build, runnable artifact | `make build`, or `docker build -t croniter-rs . && docker run --rm croniter-rs` — built and run in CI |
| 3 | Original suite, hash-pinned, passing | [`tests/original/`](./tests/original) + [`HASHES.txt`](./tests/original/HASHES.txt); `make test-original` → 248/248 |
| 4 | Differential fuzz harness + log | [`fuzz/harness.py`](./fuzz/harness.py), [`fuzz/log.txt`](./fuzz/log.txt) |
| 5 | DECISIONS.md | [19 entries](./DECISIONS.md) |
| 6 | Benchmark report + methodology | [`bench/methodology.md`](./bench/methodology.md), [`bench/results.json`](./bench/results.json), [`bench/compare.py`](./bench/compare.py) |
| 7 | 5-minute demo video | `make demo` is the scripted walkthrough it records; **the recording itself is not yet in the repo** |

Everything above except the video is re-checked by
[CI](./.github/workflows/ci.yml) on every push: build, `fmt --check`, `clippy -D warnings`,
the full Rust test suite, the corpus replay, the upstream suite through the bridge, the
suite's own hashes, a Docker build-and-run, and a 90-second fuzz run on a fresh seed that
fails the build on any divergence.

## Layout

```
src/expand.rs   cron expression parser        (croniter _expand)
src/calc.rs     next/prev search, naive time  (croniter _calc)
src/tz.rs       DST gap and fold resolution   (croniter _add_tzinfo)
src/lib.rs      public API and the DST tail   (croniter class)
src/bin/        conformance JSON server, benchmark sample runner
.github/        CI: build, lint, tests, upstream suite, docker, fuzz
tests/original/ upstream suite, unmodified, hash-pinned
tests/port/     golden corpus
tests/          corpus replay, property tests, regression tests
tools/          corpus extractor and Python bridge
fuzz/           differential fuzz harness and log
bench/          benchmark methodology, comparison script, results
```

## Performance

Full numbers and their caveats are in [`bench/results.json`](./bench/results.json) and
[`bench/methodology.md`](./bench/methodology.md). Reproduce with `python3 bench/compare.py`.

Measured 2026-08-03, 2,000 samples per workload per expression, CPython 3.12.3 against
`--release` + LTO on rustc 1.97.1. Range is across the seven expressions in the set.

| workload | expressions | median speedup | p99 speedup |
| --- | --- | --- | --- |
| `parse` | 7 | x8 – x57 | x9 – x189 |
| `next_once` | 7 | x12 – x63 | x11 – x117 |
| `prev_once` | 6 | x13 – x88 | x17 – x147 |
| `walk_1000` (throughput) | 3 | x66 – x114 | x91 – x134 |
| `range_one_year` | 1 | x185 | x172 |
| `dst_transition_walk` | 1 | x117 | x115 |

| | Python | Rust | |
| --- | --- | --- | --- |
| Startup to first result | 55.8 ms | 1.4 ms | x39 |
| Peak RSS (`walk_1000`) | 17.4 MB | 3.9 MB | 4.4x smaller |

Read [`methodology.md`](./bench/methodology.md) before quoting any of these. Three caveats
matter most: the startup comparison is an interpreter boot measured against a compiled
binary and is **not** like-for-like; the spread within a workload is wide, so the range
is reported rather than a single flattering figure; and every number is single-machine
with no CPU pinning, on a cloud VM, and moves run to run. They are order-of-magnitude
evidence, not a regression baseline.

The narrowest margin is `parse` on `*/5 9-17 * * mon-fri` (x8) — the expression with the
most alias and range expansion, where the port does the most string work per call.

## Divergences

Every non-trivial difference from the Python is written up in
[`DECISIONS.md`](./DECISIONS.md) — 19 entries, including the one known behavioural
divergence (day-of-month/day-of-week union across a DST transition, §5), the three calls
Rust's type system makes unconstructible (§15), the bridge's handle protocol (§16), the
range walk that streams rather than collecting (§17), and the point past which chrono-tz
stops projecting DST at all (§19).

That last one is what differential fuzzing is for. A `get_prev` in `Australia/Sydney` at a
2100 start came back exactly an hour off — same local time, wrong UTC offset. It was not a
search bug: chrono-tz's transition table runs out after a zone's last explicit transition
and then holds the final offset forever, while Python's `zoneinfo` keeps applying the POSIX
rule. The two agree through 2099. The boundary is now asserted by a test rather than
assumed, so a chrono-tz release that extends the table will say so.

## Safety and error handling

No `unsafe`, anywhere. Library code contains no `unwrap`, `panic!` or `unreachable!`
outside test modules — every fallible path returns `Result<_, CroniterError>`, and the
places that once relied on a preceding guard to make an `unwrap` safe now carry the
invariant in the type instead. `cargo clippy --all-targets -- -D warnings` is clean.

Seven `expect` calls remain in library code, and they are deliberate rather than
overlooked. Each constructs a value from a literal that cannot fail — the 1970-01-01 date
and midnight time (`lib.rs:73,75`), the Unix epoch (`lib.rs:319`), a literal `h/m/s` triple
(`calc.rs:160`), and three `str::parse` calls on input a preceding `only_int` check has
already proved is decimal digits (`expand.rs:681,696,699`). None depends on a caller
honouring a contract, and none is reachable by any input. The message on each states the
invariant it relies on. Verify with:

```sh
rg -n '\.unwrap\(\)|\.expect\(|panic!|unreachable!|unsafe' src/ --glob '!bin/'
```

`src/bin/` is excluded from that count throughout: the conformance server and the
benchmark sample runner are verification tooling, not part of the published crate
(`Cargo.toml` `exclude`), and holding them to the crate's bar would inflate the figure.

## Licence

MIT, inherited from the original.
