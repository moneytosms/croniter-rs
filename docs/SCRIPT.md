# SCRIPT.md: narration script for the 5-minute demo recording

## TL;DR
- Read every `SAY:` block verbatim, word for word, out loud.
- `DO:` lines are actions, not speech. Never read them aloud.
- Follow `tools/demo.sh`'s 8 sections in order; each maps to one section below.
- Total runtime target: 5:00. Hard ceiling: 5:30.
- Fluffed a line? Stop, pause the recording, re-read just that block, resume.

**Word count / pacing:** 670 words of `SAY:` prose, at 140 wpm is about 4:47 of
speaking time, which leaves headroom for pauses and for the terminal to catch up.
Section time budgets below are the pacing checkpoints. If you are running long by
section 4, tighten your pauses in 5 and 6, not your words.

---

## Cold open (0:00–0:20)

DO: black terminal, prompt visible, nothing run yet.

SAY:
This is croniter-rs. A Rust port of Python's croniter, the library a lot of schedulers
use to answer one question: given a cron expression, when does it fire next. Port Mortem
twenty twenty-six, track D. The rule that matters most: the port cannot link into the
Python runtime. No interpreter, no FFI. Let's prove it works.

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
Passing the suite is one bar. Here's a higher one. I recorded every call it makes
against real Python, and exactly what Python returned or raised. Fifteen thousand eight
hundred and forty calls, replayed in pure Rust with no Python involved.

DO: let the corpus output print, point at the match count.

SAY:
Fifteen thousand eight hundred twenty-seven out of fifteen thousand eight hundred
twenty-seven match. That's one hundred percent. The thirteen left out are named, not
hidden. Ten use croniter's random syntax with no fixed answer to check, and three are
calls Rust's own type system won't let you make in the first place.

DO: press enter.

---

## 5. Properties that hold regardless of what Python does (2:15–2:35)

SAY:
The corpus proves I agree with Python. It doesn't prove either of us is correct. So
there's a second layer: eleven properties, four hundred generated cases each, checking
the iterator against its own rules. It always advances, never skips a fire, and
round-trips.

DO: let properties output print, point at pass line, press enter.

---

## 6. Differential fuzzing (2:35 to 3:35). The two story beats live here.

SAY:
This is where the real bugs showed up. I ran a fuzzer that throws random expressions and
random start times at both implementations and diffs the answers.

DO: let the fuzz summary print, point at the numbers.

SAY:
Fourteen thousand six hundred ten cases here, zero divergences. The log that matters
most, though, is what an earlier run found.

SAY:
First finding. Python's datetime stores whole microseconds. Chrono keeps nanoseconds.
The backward search subtracts one microsecond to look just before an instant. In Python
that lands clean. In Rust it landed forty-six nanoseconds below the second, zeroing the
seconds field rounded that into the previous minute, and the search answered a whole day
early. You don't find that by reading code. Only by diffing against a real reference at
nanosecond resolution. It's pinned now, with a named test.

DO: pause half a beat, move on.

SAY:
Second finding. A case in Sydney, year twenty-one hundred, where both agreed on local
time but disagreed on the UTC offset by exactly one hour. Not my bug. Rust's timezone
library compiles a fixed table of DST transitions and holds the last offset forever past
it. Python keeps evaluating the rule. They agree through twenty ninety-nine. Nothing in
the port was wrong, and I'd rather say that than have someone else find it. A test pins
the boundary.

DO: press enter.

---

## 7. Performance (3:35–4:20)

SAY:
None of that matters if it's slow. It isn't.

DO: let the performance table print, point at a few rows.

SAY:
Parsing, eight to fifty-seven times faster. Next fire time, twelve to sixty-three.
A one-year range, a hundred eighty-five. Startup drops from fifty-six milliseconds to
under two, and peak memory by four and a half times. Medians across real expressions,
not a cherry-picked case.

DO: press enter.

---

## 8. What is not equivalent (4:20–4:45)

SAY:
Last thing. Every place this port genuinely differs from Python is written up, not
buried. Nineteen entries in the decisions log, including the Sydney finding you just
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
