# SCRIPT.md — narration script for the 5-minute demo recording

## TL;DR
- Read every `SAY:` block verbatim, word for word, out loud.
- `DO:` lines are actions, not speech — never read them aloud.
- Follow `tools/demo.sh`'s 8 sections in order; each maps to one section below.
- Total runtime target: 5:00. Hard ceiling: 5:30.
- Fluffed a line? Stop, pause the recording, re-read just that block, resume.

**Word count / pacing:** 758 words of `SAY:` prose, at 140 wpm ≈ 5:25 of speaking time.
Section time budgets below are the pacing checkpoints — if you're running long by section
4, tighten your pauses in 5 and 6, not your words.

---

## Cold open (0:00–0:20)

DO: black terminal, prompt visible, nothing run yet.

SAY:
This is croniter-rs. A Rust port of Python's croniter library, the thing a lot of
schedulers use to answer one question: given a cron expression, when does it fire next.
Built for Port Mortem twenty twenty-six, track D, Python to Rust. The rule that matters
most: the port cannot link into the Python runtime. No embedded interpreter, no FFI.
Let's prove it works.

DO: run `make demo`.

---

## 1. The upstream suite is untouched (0:20–0:45)

SAY:
First thing: I did not touch the original test suite. Every file in it is checksummed,
and here's that check running live.

DO: let section 1 print, point at the PASS line.

SAY:
Original Python test suite, byte for byte, unedited.

DO: press enter to continue.

---

## 2. One command builds it (0:45–1:05)

SAY:
Building it is one command. No Python, no system libraries, just a pinned Rust
toolchain.

DO: let the build tail print, then press enter.

---

## 3. The original suite, live, against the Rust port (1:05–1:40)

SAY:
Now the interesting part. That same untouched Python test suite is running live, right
now, against the Rust binary. A tiny Python shim makes `import croniter` resolve to a
pipe into the Rust process. Python calls out to Rust as an external program. That's the
legal direction under rule five. The port itself contains zero Python.

DO: let the suite output print, point at the pass count.

SAY:
Two hundred forty-eight tests, all passing, against Rust.

DO: press enter.

---

## 4. Behavioral equivalence: the golden corpus (1:40–2:15)

SAY:
Passing the test suite is one bar. Here's a higher one. I recorded every call that suite
makes against real Python — expression, start time, timezone, and exactly what Python
returned or raised. Fifteen thousand eight hundred and forty calls. I replay all of them
in pure Rust, no Python involved.

DO: let the corpus output print, point at the match count.

SAY:
Fifteen thousand eight hundred twenty-seven out of fifteen thousand eight hundred
twenty-seven match. That's one hundred percent. The thirteen left out are named, not
hidden — ten use croniter's random syntax with no fixed answer to check, and three are
calls Rust's own type system won't let you make in the first place.

DO: press enter.

---

## 5. Properties that hold regardless of what Python does (2:15–2:35)

SAY:
The corpus proves I agree with Python. It doesn't prove either of us is correct. So
there's a second layer: eleven properties, four hundred generated cases each, checking
the iterator against its own rules — it always advances, never skips a fire, and
round-trips.

DO: let properties output print, point at pass line, press enter.

---

## 6. Differential fuzzing (2:35–3:35) — the two story beats live here

SAY:
This is where the real bugs showed up. I ran a fuzzer that throws random expressions and
random start times at both implementations and diffs the answers.

DO: let the fuzz summary print, point at the numbers.

SAY:
Fourteen thousand six hundred ten cases here, zero divergences. The log that matters
most, though, is what an earlier run found.

SAY:
First finding. Python's datetime only stores whole microseconds. Chrono, the Rust time
library, keeps full nanoseconds. A start time one microsecond past midnight sits about
nine hundred fifty-four nanoseconds past the second in Rust. The backward search
subtracts one microsecond to look just before that instant. In Python that's clean. In
Rust it landed forty-six nanoseconds below the second instead of on it. Zeroing the
seconds field then rounded that down into the previous minute, and the search answered a
whole day early. You don't find that by reading code. You only find it by checking
against a real reference implementation at nanosecond resolution. It's pinned now, with
a named test.

DO: pause half a beat, move on.

SAY:
Second finding. The fuzzer found a case in Sydney, year twenty-one hundred, where Rust
and Python agreed on the local time but disagreed on the UTC offset by exactly one hour.
Not my bug. Rust's timezone library compiles a fixed table of DST transitions, and past
the last entry it just holds the final offset forever. Python keeps evaluating the
actual rule indefinitely. The two agree through the year twenty ninety-nine and diverge
after. Nothing in the port was wrong, and I'd rather say that out loud than have someone
else find it first. There's a test that pins the exact boundary.

DO: press enter.

---

## 7. Performance (3:35–4:20)

SAY:
None of that matters if it's slow. It isn't.

DO: let the performance table print, point at a few rows.

SAY:
Parsing is eight to fifty-seven times faster. Finding the next fire time is twelve to
sixty-three times faster. A one-year range is a hundred eighty-five times faster.
Startup drops from fifty-six milliseconds to under two. Peak memory drops by more than
four times. These are medians across real cron expressions, not a cherry-picked case.

DO: press enter.

---

## 8. What is not equivalent (4:20–4:45)

SAY:
Last thing. Every place this port genuinely differs from Python is written up, not
buried — nineteen entries in the decisions log, including the Sydney finding you just
saw. And the safety numbers: zero unsafe blocks, zero unwrap or panic calls anywhere in
the library outside tests. Seven expect calls remain, each on a literal that provably
cannot fail, each documented.

DO: let the DECISIONS.md list and safety line print.

---

## Close (4:45–5:00)

SAY:
That's croniter-rs. A Python library ported to Rust with no Python inside it, and honest
about where it disagrees. The repo is on GitHub, moneytosms slash croniter-rs. Remember
one thing: the fuzzer found a real bug and a real library boundary, and both are pinned
by tests now, not just written down.

DO: stop recording.
