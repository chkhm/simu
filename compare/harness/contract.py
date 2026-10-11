# SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
#
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Shared result schema + subprocess runners for both tools.

Both the Rust runner (`examples/compare`) and each SimPy model emit one JSON
object with this shape:

    {"tool", "model", "seeds", "n", "lambda", "mu", "servers",
     "events", "wall_secs", "per_seed": [ {metric: value, ...}, ... ]}

`per_seed` keys differ per model but are identical across the two tools for the
same model, which is what makes the metric-by-metric comparison meaningful.
"""

import json
import os
import re
import subprocess
import sys
import time

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
COMPARE_DIR = os.path.join(REPO_ROOT, "compare")
MODELS_DIR = os.path.join(COMPARE_DIR, "models")
SIMU_BIN = os.path.join(REPO_ROOT, "target", "release", "examples", "compare")

# Map model name -> SimPy script.
SIMPY_SCRIPTS = {
    "mm1": "mm1.py",
    "mmc": "mmc.py",
    "priority": "priority_queue.py",
    "container": "container.py",
    "coffee": "coffee.py",
    "hospital": "hospital.py",
}


def _args_list(model, seeds, n, lam, mu, servers):
    return [
        "--model", model,
        "--seeds", str(seeds),
        "--n", str(n),
        "--lambda", str(lam),
        "--mu", str(mu),
        "--servers", str(servers),
    ]


# Wrap the child in /usr/bin/time so peak RSS is measured *per process* (the
# getrusage(RUSAGE_CHILDREN) counter is monotonic across all reaped children and
# cannot isolate a single one). macOS uses `-l` (bytes), GNU/Linux `-v` (kbytes).
_TIME_BIN = "/usr/bin/time"
_TIME_FLAG = "-l" if sys.platform == "darwin" else "-v"


def _parse_peak_rss_mb(stderr_text):
    m = re.search(r"(\d+)\s+maximum resident set size", stderr_text)  # macOS, bytes
    if m:
        return int(m.group(1)) / (1024 * 1024)
    m = re.search(r"Maximum resident set size \(kbytes\):\s*(\d+)", stderr_text)  # GNU
    if m:
        return int(m.group(1)) / 1024
    return float("nan")


def _measure(cmd, cwd=None):
    """Run `cmd`, capturing stdout JSON plus wall time and isolated peak RSS (MB)."""
    use_time = os.path.exists(_TIME_BIN)
    full = ([_TIME_BIN, _TIME_FLAG] + cmd) if use_time else cmd
    t0 = time.perf_counter()
    proc = subprocess.run(full, cwd=cwd, capture_output=True, text=True)
    wall = time.perf_counter() - t0
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        raise RuntimeError(f"command failed ({proc.returncode}): {' '.join(cmd)}")
    peak_mb = _parse_peak_rss_mb(proc.stderr) if use_time else float("nan")
    result = json.loads(proc.stdout.strip().splitlines()[-1])
    result["_proc_wall_secs"] = wall
    result["_peak_mb"] = peak_mb
    return result


def run_simu(model, seeds, n=1000, lam=0.9, mu=1.0, servers=1, parallel=False):
    if not os.path.exists(SIMU_BIN):
        raise FileNotFoundError(
            f"{SIMU_BIN} not found — build it with "
            "`cargo build --release --features monte-carlo --example compare`"
        )
    cmd = [SIMU_BIN] + _args_list(model, seeds, n, lam, mu, servers)
    if parallel:
        # Fan the seeds out across threads (SimPy has no equivalent — the GIL
        # serialises it). Needs the rayon-backed `monte-carlo` feature build.
        cmd.append("--parallel")
    return _measure(cmd)


def run_simpy(model, seeds, n=1000, lam=0.9, mu=1.0, servers=1, python=None):
    script = os.path.join(MODELS_DIR, SIMPY_SCRIPTS[model])
    python = python or sys.executable
    cmd = [python, script] + _args_list(model, seeds, n, lam, mu, servers)
    # cwd=MODELS_DIR so `from _common import ...` resolves.
    return _measure(cmd, cwd=MODELS_DIR)


def metric_keys(result):
    """The per-seed metric names (sorted) for a result object."""
    if not result["per_seed"]:
        return []
    return sorted(result["per_seed"][0].keys())


def column(result, key):
    """Extract one metric across all seeds as a list of floats."""
    return [float(s[key]) for s in result["per_seed"]]
