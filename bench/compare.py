#!/usr/bin/env python3
"""Benchmarks the original Python croniter against this port and writes results.json.

The methodology this implements — which workloads, how p99 is obtained, what the
startup and RSS numbers do and do not mean — is written up in `bench/methodology.md`.
Read that first; this file is the mechanism, not the argument.

Both sides run the same expressions from the same start instant. The Rust side is driven
through a tiny `bench_runner` binary rather than through `cargo bench`, because divan
reports its own aggregates and this script needs the raw per-iteration samples to compute
a real p99 instead of estimating one from a mean.

Usage:
    python3 bench/compare.py [--out bench/results.json] [--samples N]

Requires the extractor venv (it has the pinned original croniter installed):
    tools/extract_corpus/run.sh
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import statistics
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
VENV_PYTHON = REPO_ROOT / "tools" / "extract_corpus" / ".venv" / "bin" / "python"

# Same set as benches/schedule.rs, and for the same reason: it spans the parser's cheap
# and expensive paths rather than only the cases that flatter the port.
EXPRS = [
    "* * * * *",
    "0 0 * * *",
    "*/5 9-17 * * mon-fri",
    "0 0 1 * *",
    "0 0 L * *",
    "0 0 * * 5#3",
    "0 0 1 1 * 0 2030",
]
WALK_EXPRS = ["* * * * *", "*/5 9-17 * * mon-fri", "0 0 * * 5#3"]

# `0 0 1 1 * 0 2030` pins a year in the future, so searching backwards from the 2026
# start has nothing to find and both implementations raise. Timing a guaranteed failure
# would measure the give-up path, not the search, so it is excluded here rather than
# quietly caught.
PREV_EXPRS = [e for e in EXPRS if e != "0 0 1 1 * 0 2030"]

START = (2026, 3, 8, 0, 0, 0)


def _reexec_into_venv() -> None:
    """Re-exec under the extractor venv, which has the pinned croniter installed."""
    if os.environ.get("_CRONITER_BENCH_REEXEC"):
        return
    try:
        import croniter  # noqa: F401
    except ImportError:
        if not VENV_PYTHON.is_file():
            sys.exit(
                f"croniter is not importable and {VENV_PYTHON} does not exist.\n"
                "Create it first:  tools/extract_corpus/run.sh"
            )
        os.environ["_CRONITER_BENCH_REEXEC"] = "1"
        os.execv(str(VENV_PYTHON), [str(VENV_PYTHON), __file__, *sys.argv[1:]])


_reexec_into_venv()

import datetime as dt  # noqa: E402

from croniter import croniter, croniter_range  # noqa: E402


def start_dt() -> dt.datetime:
    return dt.datetime(*START)


# ---------------------------------------------------------------------------
# Python side
# ---------------------------------------------------------------------------


def time_samples(fn, samples: int) -> list[float]:
    """Per-iteration wall time in nanoseconds.

    One timed call per sample, so the returned list is a real distribution rather than
    `samples` copies of an average. p99 is meaningless otherwise.
    """
    out = []
    for _ in range(samples):
        t0 = time.perf_counter_ns()
        fn()
        out.append(float(time.perf_counter_ns() - t0))
    return out


def py_workloads(samples: int) -> dict[str, dict[str, list[float]]]:
    results: dict[str, dict[str, list[float]]] = {}

    parse = {}
    for expr in EXPRS:
        parse[expr] = time_samples(lambda e=expr: croniter.is_valid(e), samples)
    results["parse"] = parse

    next_once, prev_once = {}, {}
    for expr in EXPRS:
        next_once[expr] = time_samples(
            lambda e=expr: croniter(e, start_dt()).get_next(float), samples
        )
    for expr in PREV_EXPRS:
        prev_once[expr] = time_samples(
            lambda e=expr: croniter(e, start_dt()).get_prev(float), samples
        )
    results["next_once"] = next_once
    results["prev_once"] = prev_once

    # Parse cost is deliberately outside the timed region: this is the throughput
    # number, and including the parse would flatter it.
    def walk(expr: str) -> None:
        it = croniter(expr, start_dt())
        for _ in range(1000):
            it.get_next(float)

    walk_samples = max(3, samples // 40)
    results["walk_1000"] = {e: time_samples(lambda x=e: walk(x), walk_samples) for e in WALK_EXPRS}

    def one_year() -> None:
        list(croniter_range(start_dt(), dt.datetime(2027, 3, 8), "0 0 * * *"))

    results["range_one_year"] = {"0 0 * * *": time_samples(one_year, max(3, samples // 40))}

    def dst_walk() -> None:
        import zoneinfo

        tz = zoneinfo.ZoneInfo("America/New_York")
        it = croniter("0 * * * *", dt.datetime(2026, 3, 8, tzinfo=tz))
        for _ in range(24):
            it.get_next(dt.datetime)

    results["dst_transition_walk"] = {"0 * * * *": time_samples(dst_walk, max(3, samples // 10))}
    return results


# ---------------------------------------------------------------------------
# Rust side
# ---------------------------------------------------------------------------


def rust_workloads(samples: int) -> dict[str, dict[str, list[float]]]:
    binary = REPO_ROOT / "target" / "release" / "bench_runner"
    subprocess.run(
        ["cargo", "build", "--release", "--bin", "bench_runner"],
        cwd=REPO_ROOT,
        check=True,
    )
    proc = subprocess.run(
        [str(binary), str(samples)], cwd=REPO_ROOT, capture_output=True, text=True, check=True
    )
    return json.loads(proc.stdout)


# ---------------------------------------------------------------------------
# Startup and RSS, measured externally
# ---------------------------------------------------------------------------


def measure_startup(cmd: list[str], runs: int = 10) -> dict[str, float]:
    """Process start to first result, from the outside.

    For Python this includes interpreter boot and `import croniter`; for the Rust binary
    it is process start to first answer. They are not the same thing, and the writeup
    says so rather than presenting the ratio as a like-for-like speedup.
    """
    times = []
    for _ in range(runs):
        t0 = time.perf_counter_ns()
        subprocess.run(cmd, cwd=REPO_ROOT, capture_output=True, check=True)
        times.append(float(time.perf_counter_ns() - t0))
    return summarize(times)


def measure_rss(cmd: list[str]) -> int | None:
    """Peak RSS in kilobytes for one child process, via `/usr/bin/time -v`.

    Measured on the whole process for each side, running the walk_1000 workload. Returns
    None where GNU time is unavailable rather than substituting a number from somewhere
    else, since the two would not be comparable.
    """
    gnu_time = "/usr/bin/time"
    if not os.path.isfile(gnu_time):
        return None
    proc = subprocess.run(
        [gnu_time, "-v", *cmd], cwd=REPO_ROOT, capture_output=True, text=True, check=False
    )
    for line in proc.stderr.splitlines():
        if "Maximum resident set size" in line:
            return int(line.rsplit(":", 1)[1].strip())
    return None


def summarize(samples: list[float]) -> dict[str, float]:
    ordered = sorted(samples)
    return {
        "n": len(ordered),
        "min_ns": ordered[0],
        "median_ns": statistics.median(ordered),
        "mean_ns": statistics.fmean(ordered),
        # Taken from the ordered samples, not derived from mean and stdev.
        "p95_ns": ordered[min(len(ordered) - 1, int(round(0.95 * (len(ordered) - 1))))],
        "p99_ns": ordered[min(len(ordered) - 1, int(round(0.99 * (len(ordered) - 1))))],
        "max_ns": ordered[-1],
    }


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=str(REPO_ROOT / "bench" / "results.json"))
    ap.add_argument("--samples", type=int, default=2000)
    args = ap.parse_args()

    print(f"==> Python side ({args.samples} samples/workload)", file=sys.stderr)
    py = py_workloads(args.samples)
    print("==> Rust side", file=sys.stderr)
    rs = rust_workloads(args.samples)

    workloads: dict[str, dict] = {}
    for name in sorted(set(py) | set(rs)):
        per_expr = {}
        for expr in sorted(set(py.get(name, {})) | set(rs.get(name, {}))):
            entry = {}
            if expr in py.get(name, {}):
                entry["python"] = summarize(py[name][expr])
            if expr in rs.get(name, {}):
                entry["rust"] = summarize(rs[name][expr])
            if "python" in entry and "rust" in entry:
                entry["speedup_median"] = round(
                    entry["python"]["median_ns"] / max(entry["rust"]["median_ns"], 1e-9), 2
                )
                entry["speedup_p99"] = round(
                    entry["python"]["p99_ns"] / max(entry["rust"]["p99_ns"], 1e-9), 2
                )
            per_expr[expr] = entry
        workloads[name] = per_expr

    print("==> startup and RSS", file=sys.stderr)
    runner = str(REPO_ROOT / "target" / "release" / "bench_runner")
    py_first_result = [
        sys.executable,
        "-c",
        "from croniter import croniter; croniter('* * * * *').get_next()",
    ]
    startup = {
        "python": measure_startup(py_first_result),
        "rust": measure_startup([runner, "0"]),
    }

    # RSS over the walk_1000 workload, per methodology.md.
    py_walk = [
        sys.executable,
        "-c",
        "from croniter import croniter\n"
        "import datetime as dt\n"
        "it = croniter('* * * * *', dt.datetime(2026, 3, 8))\n"
        "[it.get_next(float) for _ in range(1000)]\n",
    ]
    rss = {
        "python_kb": measure_rss(py_walk),
        "rust_kb": measure_rss([runner, "40"]),
        "workload": "walk_1000",
        "tool": "/usr/bin/time -v (Maximum resident set size)",
    }

    results = {
        "schema": 1,
        "generated_utc": dt.datetime.now(dt.timezone.utc).isoformat(timespec="seconds"),
        "methodology": "bench/methodology.md",
        "environment": {
            "platform": platform.platform(),
            "processor": platform.processor() or platform.machine(),
            "python": platform.python_version(),
            "rustc": subprocess.run(
                ["rustc", "--version"], capture_output=True, text=True, check=False
            ).stdout.strip(),
            "cpu_pinning": False,
            "note": "single machine, single run-set; order-of-magnitude, not a regression baseline",
        },
        "samples_per_workload": args.samples,
        "workloads": workloads,
        "startup": startup,
        "peak_rss": rss,
    }

    out = Path(args.out)
    out.write_text(json.dumps(results, indent=2) + "\n")
    print(f"wrote {out}", file=sys.stderr)

    for name, per_expr in workloads.items():
        for expr, entry in per_expr.items():
            if "speedup_median" in entry:
                print(
                    f"{name:22} {expr:24} median x{entry['speedup_median']:<8} "
                    f"p99 x{entry['speedup_p99']}"
                )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
