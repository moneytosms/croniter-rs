# Benchmark methodology

Numbers live in `results.json`. This file says how they were produced and what they do
and do not mean.

## What is being compared

- **Original:** `pallets-eco/croniter` at commit `f64665e`, running under the CPython in
  `tools/extract_corpus/.venv`, with `python_dateutil` installed. This is the same
  interpreter and library the golden corpus was extracted from.
- **Port:** this crate, `--release`, LTO on, `codegen-units = 1`, toolchain pinned to
  1.97.1 by `rust-toolchain.toml`.

Both sides are driven with identical expressions and identical start instants. The
expression set is the one in `benches/schedule.rs`, chosen to span the parser's cheap and
expensive paths rather than to flatter the port:

| expression | why it is in the set |
| --- | --- |
| `* * * * *` | degenerate case, every minute matches, search exits immediately |
| `0 0 * * *` | the common real-world case |
| `*/5 9-17 * * mon-fri` | steps plus ranges plus weekday names |
| `0 0 1 * *` | month rollover |
| `0 0 L * *` | last-day-of-month, needs leap-year logic |
| `0 0 * * 5#3` | nth-weekday, the expensive `proc_day_of_week_nth` path |
| `0 0 1 1 * 0 2030` | 7-field with an explicit year, longest search |

## Workloads

1. **parse** — `is_valid`, i.e. expression to parse tree, no date search. Isolates the
   parser.
2. **next_once / prev_once** — one step from a fresh instance. This is the latency number
   that matters for a scheduler asking "when next".
3. **walk_1000** — 1,000 consecutive steps from a single parse. This is the throughput
   number, and it deliberately excludes parse cost so it is not flattered by it.
4. **range_one_year** — a year of daily fires through `croniter_range`, the workload
   closest to real scheduler use.
5. **dst_transition_walk** — 24 hourly steps across the 2026-03-08 America/New_York
   spring-forward, exercising the `calc_with_tz` re-entrant path. The expensive case, kept
   in the set on purpose.

## How measured

- Rust: `divan` (`cargo bench`), which reports median and percentiles across its own
  sample count, plus allocation counts.
- Python: `timeit` over the same workloads, same iteration counts, in a subprocess so
  import cost is attributed to startup rather than to the workload.
- **p99** is reported per workload from the raw sample distribution, not estimated from a
  mean and a standard deviation.
- **Startup** is process start to first result, measured externally with `hyperfine`-style
  repeated invocation: for Python that includes interpreter boot and `import croniter`;
  for Rust it is the `croniter-conformance` binary answering its first request. These are
  not the same thing and the comparison is reported as such.
- **RSS** is peak resident set from `/usr/bin/time -v` on the whole process for each side,
  running the `walk_1000` workload.

## Honesty notes

- Throughput-only numbers are misleading here. A cron search is a loop over a small,
  bounded state space, so the port's advantage is mostly interpreter overhead rather than
  algorithmic. The p99 figures matter more than the medians, and both are reported.
- The DST workload is in the table because it is the expensive path — `calc_with_tz`
  re-runs the naive search when a local time does not exist — not because it flatters the
  port. On the current run-set it happens to score well (x117); on earlier run-sets it was
  the *lowest* of the group. That swing is itself the point: treat any single workload's
  ratio as noisy and read the range.
- The narrowest margin is `parse` on `*/5 9-17 * * mon-fri` (x8 median), the expression
  with the most alias and range expansion. Reported because a summary that quoted only the
  x185 `range_one_year` figure would be dishonest.
- Startup comparison flatters the port structurally: a compiled binary versus an
  interpreter import is not an apples-to-apples measurement, and it is reported only
  because the submission asks for it.
- Every number is single-machine, single-run-set, no CPU pinning. Treat them as
  order-of-magnitude, not as a regression baseline.

## Reproducing

```sh
make bench          # Rust side, divan
python3 bench/compare.py   # both sides, writes results.json
```
