# Devfolio submission text

Copy each block into the matching field. Nothing here is rounded up or invented; every
number is reproducible from the repo.

---

## Tagline

```
A Rust port of Python's croniter, proven equivalent against the original's own test suite
```

---

## The problem it solves

```markdown
croniter is the library that answers one deceptively simple question: given a cron
expression like `*/30 1-3 * * *` and a point in time, when does it fire next, and when did
it last fire. A lot of Python schedulers lean on it to decide when work actually runs.

The problem with rewriting something like this in a faster language is that cron is not
simple. It is fifteen years of accumulated edge cases. Daylight saving time creates local
times that happen twice and local times that never happen at all. There is a
day-of-month/day-of-week rule inherited from vixie-cron that looks like a bug and is
deliberate. There is `L` for last day of month, `W` for nearest weekday, `#` for nth
weekday, and hash-scheduled expressions that spread load deterministically across a fleet.

A rewrite that gets any of those subtly wrong does not crash. It just runs your job at the
wrong time, quietly, maybe once a year when the clocks change. That is the failure mode we
cared about.

So we did not set out to write a cron library. We set out to write a cron library that we
could prove behaves the same as the one people already depend on. What you get is a single
Rust binary with no Python runtime needed, a good deal faster, that we can show returns
the same answers as the original on 15,827 recorded calls and passes the original
project's own 248-test suite without us editing a single line of it.

The verification is honestly the part we would want someone to look at. The hackathon rule
was that the port cannot link into the Python runtime, no PyO3, no embedded interpreter.
So we ran the original test suite once under instrumentation and recorded every call it
made, what was asked and what Python answered, into a golden corpus that pure Rust replays
with no Python present. Then, separately, we wrote a small Python shim that satisfies
`import croniter` and forwards every call to the Rust binary over a pipe, which lets the
untouched original test suite run live against our port. Python calls out to Rust as an
external program, which is the legal direction. Either layer would stand on its own.
```

---

## Challenges we ran into

```markdown
The one that took the longest to find was three orders of magnitude smaller than anything
we thought we were looking for.

`get_prev` was occasionally returning a fire time a whole day early. Not slightly wrong, a
whole day. Python's `datetime` stores whole microseconds. chrono, the Rust date library,
keeps nanoseconds. A start time one microsecond past the second lands about 954
nanoseconds past it once it goes through a float. The backward search subtracts one
microsecond to look just before that instant, which in Python lands exactly on the second
and in Rust landed 46 nanoseconds below it. Zeroing the seconds field then rounded that
into the previous minute, and the whole search fell through to the day before.

You cannot find that by reading the code. Both versions look correct. We only found it
because we were diffing against a real reference implementation at full precision.

The more uncomfortable lesson was why it took so long. Our corpus extractor had a bug of
its own: it recorded the constructor keyword arguments onto one record type and rebuilt
everything else from scratch, so zero of 3,128 `next` records carried a single keyword.
Around 1,538 calls made against non-default croniters were being replayed as defaults. Our
evidence was making the port look more correct than it was, which is worse than the port
being wrong, because you do not go looking. Fixing the extractor is what made the
microsecond bug visible at all.

Two other things worth mentioning.

Differential fuzzing found a case in Sydney in the year 2100 where our port and Python
disagreed by exactly one hour. Same local time, different UTC offset. That one was not
ours. chrono-tz compiles a fixed table of DST transitions and holds the last offset
forever once it runs past the end, while Python keeps evaluating the rule indefinitely.
They agree through 2099. There was nothing to fix, so we pinned the boundary with a test
that will fail if chrono-tz ever extends its table, and capped the fuzzer at 2099 so it
stops comparing two timezone databases and goes back to comparing two croniters.

And late on, while checking our work, we found three open bug reports against the original
Python library filed by other people. We tested all three against our port. We reproduce
all three exactly. That was the right outcome rather than a disappointing one, since the
whole goal is behavioural equivalence, and fixing a bug that callers may already have
worked around would quietly break them. They are pinned by tests now, so if upstream ever
fixes one, our test fails and tells us to go and look.
```

---

## Technologies used

Type these as separate tags:

```
Rust
Python
chrono
chrono-tz
proptest
divan
serde
Docker
GitHub Actions
pytest
```

---

## Platforms

Tick or type, depending on how the field behaves:

```
Linux
Docker
```

---

## Project links

```
https://github.com/moneytosms/croniter-rs
```

---

## If a longer description field appears

```markdown
Port Mortem 2026, Track D, Python to Rust.

Results, all reproducible from the repo:

- 248 / 248 on the original Python test suite, unmodified and hash-pinned, run live
  against the Rust binary
- 15,827 / 15,827 golden-corpus records match, which is 100.00%
- 0 divergences across 14,610 differential fuzz cases
- 85 Rust tests across unit, property, regression and corpus-replay suites
- 0 unsafe blocks, and no unwrap, panic or unreachable in library code
- Roughly 8x to 185x faster depending on workload, with the one workload that falls below
  the 10x target reported rather than dropped

DECISIONS.md documents all 19 places we diverge from the original and why. docs/DEFECTS.md
lists every defect we found, ordered by severity, all of them in our own port rather than
in the original, plus a security review of the original's regexes, search bounds and
deserialisation paths that came back clean.
```
