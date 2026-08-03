# Recording the demo video (deliverable #7)

`make demo` (`tools/demo.sh`) is the scripted walkthrough. It computes every figure live
and nothing on screen is a saved constant. This doc covers how to record it.

## Pre-flight checklist (do this before hitting record)

1. **Run `make demo` once, throwaway, first.** Section 3 shells out to
   `tools/bridge/run_original_suite.sh`, which bootstraps a venv via `uv pip install` on
   first run. That's slow and ugly on camera. One dry run warms the venv so the recorded
   take is fast.
   ```sh
   make demo   # let it finish or ctrl-C after section 3; just needs the venv built once
   ```
2. Confirm `cargo build --release` is warm too (section 2). A cold release build can run
   long. Run `make build` once beforehand if you want that section snappy on tape.
3. Terminal setup: large font (18–22pt), full-window or wide, clear scrollback
   (`clear` or new tab), dark theme with good contrast for screen capture.
4. Close notification banners / do-not-disturb on, so nothing pops mid-recording.
5. Mic check: levels good, no background noise. This is a narrated demo, not a silent cast.
6. Decide pacing: `make demo` pauses for `[enter]` between sections by default. For a
   recorded take you can either narrate over each pause and press enter live, or run
   unattended and narrate over playback:
   ```sh
   DEMO_NO_PAUSE=1 make demo
   ```
   Narrated-live (pausing) gives you room to talk without racing the terminal; unattended
   is safer if you'll voice over the recording afterward.

## Version A: screen recording with narration (recommended)

This is what a judge means by "5-minute video." Zero-install options per OS:

- **Windows**, Xbox Game Bar: `Win+Alt+R` starts/stops recording, saves MP4 to
  `Videos\Captures`. Built into Windows 10/11, no install.
- **macOS**, QuickTime Player: File → New Screen Recording, or `Cmd+Shift+5` for the
  capture toolbar (pick "Record Selected Portion" for just the terminal window).
- **Linux**, OBS Studio (free, not preinstalled but one package or flatpak), or just use
  any meeting tool's (Zoom, Meet, Teams) share-screen-and-record, with no separate install if
  you already have one.

In all three: **enable microphone audio** before recording, narrate as `make demo` runs.

## Version B: terminal cast (asciinema), fallback only

Honest caveat: this produces a `.cast` file (terminal I/O only, **no audio**). `agg`
converts it to a GIF. Neither has narration, and a GIF/cast alone does not really satisfy
"5-minute video". Use this only as a supplement, for example embedded in the README, or if
screen recording genuinely isn't available.

Neither tool is installed. Install:

```sh
pipx install asciinema
cargo install --locked agg
```

Record and convert:

```sh
asciinema rec demo.cast -c "make demo"
agg demo.cast demo.gif
```

If you go this route, still narrate separately (voiceover track or a written walkthrough)
because the cast alone has no audio track to carry the talking points below.

## ~5-minute shot list

`tools/demo.sh` has 8 sections. Budget below assumes narrated-live with pauses; adjust if
running `DEMO_NO_PAUSE=1` and voicing over.

| # | Section (as printed) | Time | Talking point |
| --- | --- | --- | --- |
| 1 | The upstream suite is untouched | ~20s | Per-file SHA-256 pinned at kickoff proves `tests/original/` was never edited to make the port look better. |
| 2 | One command builds it | ~30s | `cargo build --release`, pinned toolchain, no Python or system libs needed. |
| 3 | The original Python test suite, live, against the Rust port | ~45s | The unmodified upstream suite runs through a pure-Python shim that forwards every call to the Rust binary over a pipe. Python never runs Rust logic, it is calling it as a subprocess. |
| 4 | Behavioural equivalence: the golden corpus | ~45s | 15,840 real calls captured from that suite (expression, instant, result) replayed in pure Rust. Python is a build-time data source only. |
| 5 | Properties that hold regardless of what Python does | ~30s | Independent of Python: `next` always advances, never skips, round-trips through `prev`, never stalls. Catches a bug both implementations might share. |
| 6 | Differential fuzzing | ~30s | Random expressions and DST boundaries thrown at both implementations; this is how the interesting bug below was found. |
| 7 | Performance | ~45s | Headline speedups (x8 to x185 depending on workload). Mention the methodology caveats briefly and do not oversell single-machine numbers. |
| 8 | What is not equivalent | ~60–90s | **The best beat.** Walk through DECISIONS.md: the microsecond-rounding bug (§ below) and the chrono-tz DST cutoff (§19). |

Total: ~4.5–5.5 min depending on how long you linger on section 8.

### The equivalence case to highlight (section 8)

**Microsecond rounding, pinned by `tests/regressions.rs`.** Python's `datetime` stores
whole microseconds; chrono internally kept nanoseconds. A sub-microsecond remainder that
Python would have rounded away instead pushed `get_prev` a full day into the past: same
computation, different precision, wildly different answer. It's the kind of bug that
looks fine in isolation and only shows up once you diff against a real reference
implementation, which is the whole point of the golden-corpus approach.

**Second beat if you have time, DECISIONS.md entry 19.** Differential fuzzing turned up that
chrono-tz stops projecting DST transitions after 2099 and holds the last known offset
forever, while Python's `zoneinfo` keeps applying the POSIX rule indefinitely. A
`get_prev` in `Australia/Sydney` starting in 2100 came back exactly an hour off. Good
"innovation" beat: this wasn't found by reading docs, it was found by throwing random
inputs at both implementations and diffing.

## Where to put the finished file

Don't commit a multi-minute MP4 to git. Upload it (YouTube unlisted, Google Drive, Loom,
etc.) and link it from the README deliverables table, row 7:

```markdown
| 7 | 5-minute demo video | `make demo` is the scripted walkthrough it records. Recording: <URL> |
```
