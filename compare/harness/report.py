# SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
#
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Run both harnesses and write compare/REPORT.md. Exit non-zero on any FAIL."""

import datetime
import os

import run_correctness
import run_performance

REPORT = os.path.join(run_correctness.contract.COMPARE_DIR, "REPORT.md")


def main():
    import argparse
    p = argparse.ArgumentParser()
    p.add_argument("--scale", type=float, default=1.0,
                   help="scale seeds/arrivals down (e.g. 0.1) for a quick run")
    p.add_argument("--perf-scale", type=float, default=1.0)
    p.add_argument("--no-perf", action="store_true")
    a = p.parse_args()

    correctness = run_correctness.run_all(scale=a.scale)
    sections = [
        "# simu vs. SimPy comparison report",
        "",
        f"_Generated {datetime.datetime.now().isoformat(timespec='seconds')}_",
        "",
        run_correctness.render_markdown(correctness),
    ]
    if not a.no_perf:
        perf = run_performance.run_all(scale=a.perf_scale)
        sections.append(run_performance.render_markdown(perf))
        scaling = run_performance.run_all_scaling(scale=a.perf_scale)
        sections.append(run_performance.render_scaling(scaling))
        montecarlo = run_performance.run_all_montecarlo(scale=a.perf_scale)
        sections.append(run_performance.render_montecarlo(montecarlo))

    with open(REPORT, "w") as f:
        f.write("\n".join(sections) + "\n")
    print(f"Wrote {REPORT}")

    if not all(r["all_passed"] for r in correctness):
        raise SystemExit("correctness check failed — see REPORT.md")


if __name__ == "__main__":
    main()
