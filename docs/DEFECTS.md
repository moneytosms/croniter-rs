# Defects found during verification

Every defect the verification work surfaced, ordered by severity, with what fixed it and
the test that now pins it.

The honest headline first: **this port's own verification work found no new defects in the
original Python library.** Everything in section B is a defect in this port, caught by
checking the port against the original.

Three genuine upstream bugs do exist, all reported by other people and open at submission
time. This port reproduces all three, which is the intended outcome rather than a
shortfall, and section A records the evidence for that. Section A also lists what was
examined upstream and cleared, because "we looked and found nothing" is a different and
weaker claim than "we did not look", and only the first is true here.

---

## A. The original Python library

### A1. Known open upstream bugs, and what this port does about them

Three issues were open against `pallets-eco/croniter` at submission time. None was filed
by this project. Each was run against the pinned Python and against this port.

| Issue | Defect | Python | This port | Verdict |
| --- | --- | --- | --- | --- |
| [#259](https://github.com/pallets-eco/croniter/issues/259) | `croniter_range` stop test compares wall clock, not instants, so a window spanning a fall-back fold silently loses results | returns 1 fire | returns 1 fire | equivalent |
| [#258](https://github.com/pallets-eco/croniter/issues/258) | on a 30-minute DST shift (`Australia/Lord_Howe`), `get_next` skips a fire that `get_prev` and `match` accept, so a backward step lands after the start that produced it | `prev > start` | `prev > start` | equivalent |
| [#252](https://github.com/pallets-eco/croniter/issues/252) | `expand_from_start_time` discards a stepped range's declared bounds, firing outside them | `10-50/15` becomes `[7, 22, 37]` | same | equivalent |

**These are deliberately not fixed.** The goal of the port is behavioural equivalence.
Correcting a bug that downstream callers may already have worked around would be a
divergence, and would belong in [`DECISIONS.md`](../DECISIONS.md) with a rationale rather
than being applied quietly. All three are now pinned by
[`tests/upstream_issues.rs`](../tests/upstream_issues.rs), so if a future upstream release
fixes one, the corresponding test fails and flags it for review.

Worth noting on #259: this port arrives at the same wrong answer by a different route.
Python's comparison ignores `tzinfo` when both operands share it, discarding `fold`. This
port reduces both bounds to naive local time before re-localising, which discards the same
information. The internal comment in `croniter_range_iter` claiming Python "is comparing
aware datetimes, i.e. instants" is therefore wrong about Python's mechanism, even though
the resulting behaviour matches.

### A2. Robustness and security review: nothing found

The differential fuzzer ran 14,610 cases against the pinned original at commit `f64665e`
with `divergences=0` and `warnings=0`. On top of that, the following were reviewed
specifically for robustness and security problems a port author is well placed to notice.

### Regular-expression denial of service: not present

`croniter.py` compiles six module-level regexes (lines 108 to 121). A pattern with nested
quantifiers over an overlapping character class can backtrack catastrophically on a
crafted input, so each was reviewed and then timed against adversarial payloads on the
pinned interpreter:

| Pattern | Line | Adversarial input | Time |
| --- | --- | --- | --- |
| `step_search_re` | 108 | 50,000 chars | 0.297 ms |
| `step_search_re` | 108 | `("a-" * 2000) + "!"` | 0.001 ms |
| `special_dow_re` | 115 | 120 chars | 0.003 ms |
| `special_dow_re` | 115 | `("l" * 40) + "#"` | 0.002 ms |
| `hash_expression_re` | 121 | 20,000 chars | 0.001 ms |
| `nearest_weekday_re` | 119 | 30,001 chars | 0.398 ms |

All are anchored, and their alternations use negated character classes such as `[^-]+`
rather than nested quantifiers, so matching is linear. No blowup was reproducible.
Reproduce with the interpreter in `tools/extract_corpus/.venv`.

### Unbounded search: not present

The `_calc` search loop (croniter.py:761) is bounded by `_max_years_between_matches`,
which defaults to 50 and is floored at 1 during construction (croniter.py:314 to 317). A
hostile expression that never matches terminates with `CroniterBadDateError` rather than
spinning.

### Unsafe deserialisation: not present

No `eval`, `exec`, `pickle`, `__reduce__` or `yaml.load` anywhere in `croniter.py`.

### Resource exhaustion in `croniter_range`: not present upstream

Python's `croniter_range` is a generator and yields lazily, so a wide window costs nothing
until it is consumed. Note that this is the reverse of the usual direction: the eager
version was a defect in *this port*, recorded as B2 below.

### One thing that is upstream behaviour, not an upstream bug

The vixie-cron day-of-month / day-of-week union, where both fields being restricted makes
them behave as OR rather than AND, is frequently mistaken for a bug. It is intentional in
croniter, matches vixie-cron, and is reproduced here deliberately. See
[`DECISIONS.md`](../DECISIONS.md) entry 5.

---

## B. This port

Ordered by severity. "Severity" here means the consequence had the defect shipped
unnoticed: a wrong scheduling answer outranks a performance cliff, which outranks a
missing feature.

### B1. High: `get_prev` could return a result a whole day early

**Consequence.** A silently wrong fire time. The worst class of bug for a scheduler,
because nothing errors.

**Cause.** Python's `datetime` holds whole microseconds. chrono keeps nanoseconds. A start
of `...T00:00:00.000001` lands about 954 ns past the second, so the one-microsecond offset
at the top of the prev search left the cursor 46 ns *below* the second instead of exactly
on it. Zeroing the seconds field then dropped it into the previous minute, and the search
answered a whole day early.

**Fix.** `Croniter::split_epoch_micros` rounds the epoch cursor to whole microseconds, so
the two implementations agree on what instant is being asked about.

**Pinned by.** `tests/regressions.rs::prev_from_one_microsecond_past_a_fire_returns_that_fire`.
Reverting the fix fails this test and one other, with a message that explains the bug.

**Found by.** Golden-corpus replay, only after the corpus extractor defect (B8) was fixed.

### B2. High: `croniter_range` materialised the entire window

**Consequence.** A year-long range at second granularity took 92 seconds and allocated
every fire before returning the first. On a wide enough window this is an out-of-memory
crash rather than a slow answer, so it is an availability problem, not just a performance
one.

**Cause.** The port collected into a `Vec` where Python yields from a generator.
Behaviourally identical for anything a test would write, which is why the corpus never
caught it.

**Fix.** A `CroniterRange` iterator. The `Vec`-returning functions are now thin
`collect()` wrappers over it.

**Pinned by.** `tests/regressions.rs::range_iterator_does_not_materialize_the_whole_window`,
which takes three fires from a decade-long per-second window and asserts it returns
promptly.

**Found by.** Differential fuzzing, via a generated case that stalled.

### B3. Medium: `all_next` and `all_prev` ignored `update_current`

**Consequence.** With `update_current = false` the cursor must not advance, so the caller
peeks at the next fire repeatedly without consuming it. The port advanced anyway, turning
a peek into a walk.

**Fix.** `all_from` threads the flag through to `step`.

**Pinned by.** `tests/regressions.rs::all_from_without_update_current_repeats_one_instant`.

### B4. Medium: the `max_years_between_matches` bound lost its "explicitly set" flag

**Consequence.** croniter stops silently when the caller set the bound and *raises* when it
did not (croniter.py:447 to 450). Setting the bound after construction has to narrow the
search without setting the flag. The port conflated the two, inverting raise and silent
stop.

**Fix.** `set_max_years_between_matches` sets the bound only.

**Pinned by.** `tests/regressions.rs::bound_set_after_construction_still_raises`.

### B5. Medium: `croniter_range` had no timezone support and ignored `second_at_beginning`

**Consequence.** Ranges over a DST transition returned wrong offsets, and
`second_at_beginning` was silently dropped.

**Fix.** `croniter_range_tz` plus an explicit `second_at_beginning` parameter. Range bounds
are now compared on instants rather than local wall time, which only differ across a DST
boundary.

**Pinned by.** `tests/regressions.rs::range_over_a_dst_transition_keeps_offsets` and
`::range_honours_second_at_beginning`.

### B6. Medium: `strict` / `strict_year` cross-validation was unimplemented

**Consequence.** Impossible dates such as `0 0 31 2 *`, February 31st, were accepted.

**Aggravating factor.** A doc comment claimed the branch was unreachable. The upstream
suite reaches it. An unverified comment asserting an invariant is worse than no comment.

**Fix.** `expand::check_strict`, including the leap-year narrowing rule.

**Pinned by.** `tests/regressions.rs::strict_validation_rejects_impossible_dates`.

### B7. Medium: `start_time` was missing, and misfiled as a type-system win

**Consequence.** One corpus record was excluded as "unrepresentable against the Rust API".
It was not unrepresentable. The port simply lacked croniter's `start_time` parameter, and
the exclusion list was hiding a missing feature.

**Fix.** `get_next_from` and `get_prev_from` taking a `StartTime`, plus croniter's guard
that refuses `start_time` when `expand_from_start_time` is set. Verified records rose from
15,824 to 15,827 and the unrepresentable count fell from 4 to 3.

**Pinned by.** `tests/regressions.rs::get_next_from_searches_from_the_given_instant` and
`::get_next_from_is_refused_when_expanding_from_start_time`.

**Worth noting.** This is the one defect that was hiding inside a claim of correctness
rather than inside code.

### B8. Medium: the corpus extractor recorded no constructor keywords

**Consequence.** Not a defect in the port, but a defect in the *evidence*, which is
arguably worse: it made the port look more correct than it was. Zero of 3,128 `next`
records carried a single behaviour-changing kwarg, so 1,538 calls built from non-default
croniters were replayed as defaults. B1 and B3 were hiding behind it.

**Fix.** `corpus_plugin.py` stashes the kwargs on the instance and merges them at all four
recording sites.

**Pinned by.** CI regenerates the corpus on every push and asserts the fresh extraction
still replays clean.

### B9. Low: random expressions were redrawn on every bridge call

**Consequence.** croniter draws an `R` expression's values once in `__init__` and reuses
them. The bridge rebuilt the parse per call, so consecutive `get_next()` calls wandered
instead of advancing, and one to three of the four tests in `test_croniter_random.py`
failed depending on the draw. A bridge defect, not a library one, but it made the suite
look flaky.

**Fix.** A handle protocol. `__init__` issues `create`, the engine holds the parse, later
requests quote the handle.

**Result.** 248/248, deterministically, across thirteen consecutive runs.

### B10. Low: negative UTC offsets were unparseable in the tooling

**Consequence.** Splitting on `['+', 'Z']` never matches `-02:00`, so the replay harness
and conformance server rejected negative offsets. Tooling only.

**Fix.** Correct offset parsing on both sides.

---

## What is not a defect

For completeness, since these appear in `DECISIONS.md` and could be mistaken for bugs:

- **chrono-tz stops projecting DST after a zone's last explicit transition** (entry 19).
  A third-party library boundary, not a defect in either croniter. Python is the one
  behaving correctly. Pinned by a test that fails if chrono-tz extends its table.
- **The day-of-month / day-of-week union across a DST transition** (entry 5). A known,
  narrow, documented divergence introduced by moving DST resolution out of the search
  loop.
- **`OVERFLOW32B_MODE` is always false** (entry 6). It works around a CPython `time_t`
  limitation that does not exist in chrono.
