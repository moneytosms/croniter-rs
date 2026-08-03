# Corpus Extraction Report

Total records: 15838 (deduped from 71516 raw calls)

By op:
- validate: 11662
- next: 3128
- current: 627
- prev: 396
- range: 18
- all_next: 5
- all_prev: 2

Failed calls (expect.ok == false): 60
- CroniterBadCronError: 21
- CroniterBadDateError: 20
- CroniterNotAlphaError: 13
- TypeError: 2
- CroniterUnsupportedSyntaxError: 2
- CroniterBadTypeRangeError: 1
- ValueError: 1

Distinct cron expressions: 312

Records with null expr or null start: 0 (verified via
`python3 -c "import json;d=json.load(open('tests/port/corpus.json'));print(sum(1 for r in d if r.get('expr') is None or r.get('start') is None))"` -> prints 0)

Test files that produced no records: none. All 6 files (test_croniter.py,
test_croniter_hash.py, test_croniter_range.py, test_croniter_dst_repetition.py,
test_croniter_random.py, test_croniter_speed.py) exercise the wrapped API at
least once. Note: within test_croniter_speed.py, test_large_comma_list_expands_quickly
alone contributes 0 records (it only calls the unwrapped `croniter.expand`
classmethod) but test_not_long_time in the same file drives get_next/get_prev
heavily, so the file as a whole is represented.

Tests errored under instrumentation: none. Full original suite passes
unmodified: 248 passed, 92 subtests passed.

## What each record carries

Every record stores the *effective* call, not just the expression:

- the constructor kwargs the instance was built with (`day_or`, `hash_id`,
  `second_at_beginning`, `implement_cron_bug`, `expand_from_start_time`,
  `max_years_between_matches`, `is_prev`) - on **every** op, not only on the
  `validate` record the constructor emits. A `get_next` on a `day_or=False`
  instance is a different call from the same `get_next` on a default one, and a
  replay that cannot tell them apart silently reinterprets the record as a default.
- `update_current` for `all_next`/`all_prev`, because `update_current=False`
  makes the generator repeat one instant instead of walking.
- `state_max_years_between_matches`, recorded only when the suite reassigned
  croniter's `_max_years_between_matches` attribute after construction
  (`test_explicit_year_forward` does), which bounds the search *without* setting
  the "explicitly set" flag that turns a raise into a silent stop.
- the timezone as an IANA name wherever one is recoverable, including from
  `dateutil.tz.gettz()` tzfile objects, which expose neither `.zone` nor `.key`.
  Recording those as offset-less "fixed_offset" would erase exactly the DST
  transitions the DST tests exist to check.

## Replay result

`tests/corpus_replay.rs` replays 15,824 of the 15,838 records and all of them
match. The 14 excluded records are excluded by name and reported separately by
the test rather than counted as passes:

- 10 `R` / `R(a-b)` expressions, random by construction (DECISIONS.md section 12)
- 4 calls that cannot be built against the Rust API at all (DECISIONS.md section 15)
