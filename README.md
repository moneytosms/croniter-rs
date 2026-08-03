# croniter-rs

A Rust port of [`pallets-eco/croniter`](https://github.com/pallets-eco/croniter), built for
[Port Mortem 2026](https://coderesurrection.com/2026/), Track D (Python → Rust).

Source pinned at commit `f64665e`. 1,586 lines of Python library against 4,755 lines of
test suite.

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

## Test

```sh
make test           # native Rust: unit tests + golden-corpus replay
make test-original  # the original Python suite, live, against the port
```

`make test` is the one that must be green and needs nothing but Rust.

## How this port is verified

Port Mortem Rule 05 forbids the port linking or FFI-ing into the Python runtime, so there
is no PyO3 here and no embedded interpreter. Correctness is established two ways.

**Golden corpus (primary).** `tools/extract_corpus/` runs the original Python suite once,
offline, under instrumentation that records every call it makes — expression, start
instant, timezone, every constructor keyword — together with what Python returned or
raised. The result is `tests/port/corpus.json`. `tests/corpus_replay.rs` replays it in
pure Rust. Python is a build-time data source and never a runtime dependency.

> **15,824 / 15,824 records match (100.00%).**
>
> Of the 15,838 records extracted, 14 are excluded *by name* and reported separately by
> the test rather than counted as passes: 10 use croniter's `R` random syntax and have no
> stable answer to check against (§12), and 4 are calls that cannot be constructed against
> the Rust API at all — a bad `ret_type`, a `dict` as `hash_id`, mismatched `croniter_range`
> bound types (§15).

**Conformance bridge (secondary).** `tools/bridge/croniter/` is a pure-Python shim that
satisfies `import croniter` and forwards each call to a long-lived `croniter-conformance`
subprocess over line-delimited JSON. This runs the byte-identical original test suite
against the port. Python calls the port as an external process; the port contains no
Python. See `DECISIONS.md` §1 for the full reasoning.

> **245–248 of 248 tests pass** (plus all 92 subtests), measured over 8 runs; one run
> passed all 248. Every failure is one of the 4 tests in `test_croniter_random.py`, and
> which of them fail varies run to run because the values are drawn randomly: the bridge is
> stateless, so an `R` expression re-draws on each call instead of being fixed at
> construction. That is a property of the transport, not of the port — the crate's own API
> holds the expansion for the object's lifetime. See §16.

**Differential fuzzing (continuous).** `fuzz/harness.py` generates random expressions,
start instants and DST-boundary cases, runs each against both the pinned Python and the
port, and compares. Latest run: **2,681 cases, 0 divergences** (`fuzz/log.txt`).

`tests/original/` is a verbatim copy of the upstream suite. `tests/original/HASHES.txt`
carries per-file SHA-256, and `.port-mortem.toml` records the aggregate. Nothing in it was
edited.

## Layout

```
src/expand.rs   cron expression parser        (croniter _expand)
src/calc.rs     next/prev search, naive time  (croniter _calc)
src/tz.rs       DST gap and fold resolution   (croniter _add_tzinfo)
src/lib.rs      public API and the DST tail   (croniter class)
src/bin/        conformance JSON server, benchmark sample runner
tests/original/ upstream suite, unmodified, hash-pinned
tests/port/     golden corpus
tools/          corpus extractor and Python bridge
fuzz/           differential fuzz harness and log
bench/          benchmark methodology, comparison script, results
```

## Performance

Full numbers and their caveats are in [`bench/results.json`](./bench/results.json) and
[`bench/methodology.md`](./bench/methodology.md). Reproduce with `python3 bench/compare.py`.

| workload | median speedup vs Python |
| --- | --- |
| `parse` | x6 – x30 |
| `next_once` | x10 – x52 |
| `walk_1000` (throughput) | x45 – x92 |
| `range_one_year` | x81 |
| `dst_transition_walk` | x51 |

Startup 36.5 ms → 1.6 ms; peak RSS 15.6 MB → 3.7 MB. Read `methodology.md` before quoting
any of these: the startup comparison is an interpreter boot against a compiled binary and
is not like-for-like, the DST walk is where the port is *least* ahead, and every number is
single-machine with no CPU pinning.

## Divergences

Every non-trivial difference from the Python is written up in
[`DECISIONS.md`](./DECISIONS.md) — 16 entries, including the one known behavioural
divergence (day-of-month/day-of-week union across a DST transition, §5), the four calls
Rust's type system makes unconstructible (§15), and the bridge's statelessness (§16).

## Licence

MIT, inherited from the original.
