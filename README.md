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
offline, under instrumentation that records every call it makes and what Python returned
or raised. The result is `tests/port/corpus.json`. `tests/corpus_replay.rs` replays it in
pure Rust. Python is a build-time data source and never a runtime dependency.

**Conformance bridge (secondary).** `tools/bridge/croniter/` is a pure-Python shim that
satisfies `import croniter` and forwards each call to a long-lived `croniter-conformance`
subprocess over line-delimited JSON. This runs the byte-identical original test suite
against the port. Python calls the port as an external process; the port contains no
Python. See `DECISIONS.md` §1 for the full reasoning.

`tests/original/` is a verbatim copy of the upstream suite. `tests/original/HASHES.txt`
carries per-file SHA-256, and `.port-mortem.toml` records the aggregate. Nothing in it was
edited.

## Layout

```
src/expand.rs   cron expression parser        (croniter _expand)
src/calc.rs     next/prev search, naive time  (croniter _calc)
src/tz.rs       DST gap and fold resolution   (croniter _add_tzinfo)
src/lib.rs      public API and the DST tail   (croniter class)
src/bin/        conformance JSON server
tests/original/ upstream suite, unmodified, hash-pinned
tests/port/     golden corpus
tools/          corpus extractor and Python bridge
fuzz/           differential fuzz harness and log
bench/          benchmark methodology and results
```

## Divergences

Every non-trivial difference from the Python is written up in
[`DECISIONS.md`](./DECISIONS.md), including the one known behavioural divergence
(day-of-month/day-of-week union across a DST transition, §5).

## Licence

MIT, inherited from the original.
