# Corpus Extraction Report

Total records: 15831 (deduped from 71516 raw calls)

By op:
- validate: 11663
- next: 3129
- current: 619
- prev: 395
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
