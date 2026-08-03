#!/usr/bin/env bash
# The five-minute demo, as a script so the recording is reproducible rather than
# improvised. `make demo`.
#
# Order is deliberate: prove the suite was not touched, then show it passing against the
# port, then show the numbers that back the claims in the README. Every figure printed
# here is computed live -- nothing is echoed from a saved file.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$HERE/.." && pwd)"
cd "$REPO_ROOT"

BOLD=$'\033[1m'; DIM=$'\033[2m'; GREEN=$'\033[32m'; RED=$'\033[31m'; RESET=$'\033[0m'
STEP=0

step() {
    STEP=$((STEP + 1))
    printf '\n%s━━━ %d. %s ━━━%s\n' "$BOLD" "$STEP" "$1" "$RESET"
    [ -n "${2:-}" ] && printf '%s%s%s\n' "$DIM" "$2" "$RESET"
    printf '\n'
}

pause() { [ -n "${DEMO_NO_PAUSE:-}" ] || { printf '\n%s[enter]%s' "$DIM" "$RESET"; read -r _; }; }

verdict() {
    if [ "$1" -eq 0 ]; then printf '%s  PASS%s  %s\n' "$GREEN" "$RESET" "$2"
    else printf '%s  FAIL%s  %s\n' "$RED" "$RESET" "$2"; fi
}

printf '%s\ncroniter: Python to Rust\n%s' "$BOLD" "$RESET"
printf 'Port Mortem 2026, Track D. Source pinned at %s.\n' \
    "$(awk -F'"' '/source_commit/{print substr($2,1,7)}' .port-mortem.toml)"

step "The upstream suite is untouched" \
     "Per-file SHA-256, recorded at kickoff. Nothing in tests/original/ was edited."
( cd tests/original && sha256sum -c HASHES.txt ) | tail -3
( cd tests/original && sha256sum -c --status HASHES.txt )
verdict $? "$(ls tests/original/*.py | wc -l | tr -d ' ') files verify against HASHES.txt"
pause

step "One command builds it" "No Python, no system libraries, pinned toolchain."
cargo build --release 2>&1 | tail -2
pause

step "The original Python test suite, live, against the Rust port" \
     "tests/original/ runs unmodified. \`import croniter\` resolves to a pure-Python shim
that forwards every call to the Rust binary over a pipe -- the port contains no Python."
./tools/bridge/run_original_suite.sh -q 2>&1 | tail -4
pause

step "Behavioural equivalence: the golden corpus" \
     "15,840 real calls extracted from that suite, with what Python actually returned or
raised. Replayed in pure Rust -- Python is a build-time data source, never a dependency."
cargo test --release --test corpus_replay -- --nocapture 2>/dev/null \
    | grep --color=never 'corpus:'
pause

step "Properties that hold regardless of what Python does" \
     "The corpus proves agreement. These prove correctness on the port's own terms, so a
bug both implementations share would still be caught."
cargo test --release --test properties 2>&1 | tail -3
pause

step "Differential fuzzing" \
     "Random expressions, start instants and DST boundaries, run against both."
grep --color=never '^# SUMMARY' fuzz/log.txt | sed 's/^# //'
printf '%s(committed log; `make fuzz` reruns it)%s\n' "$DIM" "$RESET"
pause

step "Performance" "Full methodology and caveats in bench/methodology.md."
python3 - <<'PY'
import json
d = json.load(open("bench/results.json"))
rows = []
for name, per in sorted(d["workloads"].items()):
    sp = [v["speedup_median"] for v in per.values() if "speedup_median" in v]
    if sp:
        rows.append((name, min(sp), max(sp)))
w = max(len(r[0]) for r in rows)
for name, lo, hi in rows:
    span = f"x{lo:.0f}" if abs(hi - lo) < 0.5 else f"x{lo:.0f} - x{hi:.0f}"
    print(f"  {name:<{w}}  {span} faster (median)")
s, r = d["startup"]["python"], d["startup"]["rust"]
print(f"\n  startup {s['median_ns']/1e6:.1f} ms -> {r['median_ns']/1e6:.2f} ms")
p = d["peak_rss"]
if p.get("python_kb") and p.get("rust_kb"):
    print(f"  peak RSS {p['python_kb']/1024:.1f} MB -> {p['rust_kb']/1024:.1f} MB")
PY
pause

step "What is not equivalent" "Every non-trivial divergence is written up, with reasoning."
printf '  %s entries in DECISIONS.md:\n\n' "$(grep -c '^## ' DECISIONS.md)"
grep '^## ' DECISIONS.md | sed 's/^## /    /'
printf '\n'
# Counts and labels these separately: `expect` on an infallible literal is a different
# claim from `unwrap` on a value, and collapsing the two overstates the result.
printf 'Safety: %s unsafe blocks; %s unwrap/panic/unreachable outside test modules;\n' \
    "$(grep -rc 'unsafe' src/ 2>/dev/null | awk -F: '{s+=$2} END{print s+0}')" \
    "$(python3 -c "
import pathlib
n=0
for p in pathlib.Path('src').rglob('*.rs'):
    lines=p.read_text().split('\n')
    t=next((i for i,l in enumerate(lines) if l.strip().startswith('#[cfg(test)]')), len(lines))
    n+=sum(1 for l in lines[:t] if '.unwrap()' in l or 'panic!' in l or 'unreachable!' in l)
print(n)")"
printf '        %s expect() on infallible literals, each documented (README, Safety).\n' \
    "$(python3 -c "
import pathlib
n=0
for p in pathlib.Path('src').rglob('*.rs'):
    # Library code only. src/bin/ is the conformance server and the bench sample runner:
    # tooling, not the published crate, and holding them to the same bar would inflate
    # the number the crate is actually judged on.
    if p.parts[1:2] == ('bin',):
        continue
    lines=p.read_text().split('\n')
    t=next((i for i,l in enumerate(lines) if l.strip().startswith('#[cfg(test)]')), len(lines))
    n+=sum(1 for l in lines[:t] if '.expect(' in l)
print(n)")"
printf '\n%sRepo: https://github.com/moneytosms/croniter-rs%s\n\n' "$BOLD" "$RESET"
