# SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
#
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Performance comparison: wall-clock, events/sec, and peak memory.

Each tool is launched as a subprocess so memory is measured identically via
`getrusage(RUSAGE_CHILDREN).ru_maxrss`. Wall time is the in-process simulation
time the tool self-reports (`wall_secs`), which excludes interpreter/binary
startup so events/sec reflects the engine rather than process spawn. Peak RSS is
measured around the whole child process (startup included) and reported as-is.

The scaling section runs the coffee-shop model at growing day lengths (1k to
1M customers) to show that both engines scale linearly and the speedup holds
at every size.

The Monte Carlo section additionally pits simu's *parallel* seed execution
(`monte_carlo::run`, rayon) against SimPy's sequential loop — the GIL prevents
SimPy from running replications on multiple threads — to show the full advantage
on a real many-replication workload.
"""

import os

import contract


def bench_one(model, n, lam, mu, servers, seeds=1):
    # seeds=1 with large n: one big replication isolates engine throughput.
    simu = contract.run_simu(model, seeds, n, lam, mu, servers)
    simpy = contract.run_simpy(model, seeds, n, lam, mu, servers)

    def stats(r):
        wall = r["wall_secs"]
        return {
            "wall_secs": wall,
            "events": r["events"],
            "events_per_sec": (r["events"] / wall) if wall > 0 else 0.0,
            "peak_mb": r["_peak_mb"],
        }

    return {"model": model, "seeds": seeds, "n": n,
            "simu": stats(simu), "simpy": stats(simpy)}


def default_config(scale=1.0):
    base = max(int(100_000 * scale), 2_000)
    return [
        dict(model="mm1", n=base, lam=0.9, mu=1.0, servers=1),
        dict(model="mmc", n=base, lam=1.6, mu=1.0, servers=2),
        dict(model="priority", n=base, lam=0.9, mu=1.0, servers=1),
        dict(model="coffee", n=base, lam=1.7, mu=0.5, servers=4),
    ]


def run_all(scale=1.0):
    return [bench_one(**cfg) for cfg in default_config(scale)]


def render_markdown(results):
    lines = ["## Performance: simu vs. SimPy", ""]
    lines.append(
        "Each model runs one large replication (`seeds=1`, large `n`). The "
        "coffee model is the coffee shop (4 baristas, 2 cashiers, reneging, "
        "milk fridge). The `events` column "
        "is the common, model-specific event tally and is identical across the "
        "two tools by construction.\n"
    )
    lines.append(
        "| model | workload | events | tool | wall (s) | events/sec | peak RSS (MB) |"
    )
    lines.append("|---|---|---|---|---|---|---|")
    speedups = []
    for r in results:
        workload = f"{r['n']:,} arrivals"
        for tool in ("simu", "simpy"):
            s = r[tool]
            lines.append(
                f"| {r['model']} | {workload} | {s['events']:,} | {tool} "
                f"| {s['wall_secs']:.3f} | {s['events_per_sec']:,.0f} "
                f"| {s['peak_mb']:.1f} |"
            )
        sp = (r["simpy"]["wall_secs"] / r["simu"]["wall_secs"]
              if r["simu"]["wall_secs"] > 0 else float("nan"))
        mem = (r["simpy"]["peak_mb"] / r["simu"]["peak_mb"]
               if r["simu"]["peak_mb"] > 0 else float("nan"))
        speedups.append((r["model"], sp, mem))
    lines.append("")
    lines.append("| model | simu speedup × | simu memory advantage × |")
    lines.append("|---|---|---|")
    for model, sp, mem in speedups:
        lines.append(f"| {model} | {sp:.1f}× | {mem:.1f}× |")
    lines.append("")
    lines.append(
        "_Note: all seeds run **sequentially** in a single process for both "
        "tools — these figures measure single-thread engine efficiency, not "
        "parallelism. (simu can parallelize independent seeds across cores via "
        "`monte_carlo::run`, which CPython's GIL effectively denies SimPy, but "
        "that is intentionally not exercised here.)_"
    )
    lines.append("")
    return "\n".join(lines)


def scaling_config(scale=1.0):
    # Coffee shop at growing day lengths, one replication each.
    sizes = [1_000, 10_000, 100_000, 1_000_000]
    return [dict(model="coffee", n=max(int(n * scale), 500), lam=1.7, mu=0.5,
                 servers=4) for n in sizes]


def run_all_scaling(scale=1.0):
    return [bench_one(**cfg) for cfg in scaling_config(scale)]


def render_scaling(results):
    lines = ["## Scaling: coffee shop from 1k to 1M customers", ""]
    lines.append(
        "One replication of the coffee model (4 baristas, λ=1.7/min, mean prep "
        "2 min) at growing day lengths. Both engines should scale linearly "
        "(constant events/sec); the speedup column shows whether simu's advantage "
        "holds at every size.\n"
    )
    lines.append(
        "| customers | events | simu wall (s) | SimPy wall (s) | simu events/sec "
        "| SimPy events/sec | simu peak RSS (MB) | SimPy peak RSS (MB) | speedup × |"
    )
    lines.append("|---|---|---|---|---|---|---|---|---|")
    for r in results:
        a, b = r["simu"], r["simpy"]
        sp = b["wall_secs"] / a["wall_secs"] if a["wall_secs"] > 0 else float("nan")
        lines.append(
            f"| {r['n']:,} | {a['events']:,} | {a['wall_secs']:.4f} "
            f"| {b['wall_secs']:.3f} | {a['events_per_sec']:,.0f} "
            f"| {b['events_per_sec']:,.0f} | {a['peak_mb']:.1f} "
            f"| {b['peak_mb']:.1f} | {sp:.1f}× |"
        )
    lines.append("")
    return "\n".join(lines)


def montecarlo_config(scale=1.0):
    # Many independent replications — the workload Monte Carlo parallelism
    # targets: many coffee-shop days, and many moderate M/M/1 runs.
    cf_seeds = max(int(1_000 * scale), 50)
    cf_n = max(int(2_000 * scale), 500)
    q_seeds = max(int(300 * scale), 50)
    q_n = max(int(4_000 * scale), 1_000)
    return [
        dict(model="coffee", n=cf_n, lam=1.7, mu=0.5, servers=4, seeds=cf_seeds),
        dict(model="mm1", n=q_n, lam=0.9, mu=1.0, servers=1, seeds=q_seeds),
    ]


def bench_montecarlo(model, seeds, n, lam, mu, servers):
    # Total wall-clock to complete all replications, three ways.
    simu_seq = contract.run_simu(model, seeds, n, lam, mu, servers, parallel=False)
    simu_par = contract.run_simu(model, seeds, n, lam, mu, servers, parallel=True)
    simpy = contract.run_simpy(model, seeds, n, lam, mu, servers)
    return {
        "model": model, "seeds": seeds, "n": n,
        "simu_seq_wall": simu_seq["wall_secs"],
        "simu_par_wall": simu_par["wall_secs"],
        "simpy_wall": simpy["wall_secs"],
    }


def run_all_montecarlo(scale=1.0):
    return [bench_montecarlo(**cfg) for cfg in montecarlo_config(scale)]


def render_montecarlo(results):
    cores = os.cpu_count() or 1
    lines = ["## Monte Carlo: parallel seeds (simu) vs sequential (SimPy)", ""]
    lines.append(
        f"simu fans independent replications across threads via `monte_carlo::run` "
        f"(rayon; {cores} logical cores on this machine); SimPy runs them "
        f"sequentially because CPython's GIL prevents thread parallelism. Wall-clock "
        f"is the total time to complete **all** replications. Output is byte-identical "
        f"to the sequential run (same seeds, results returned in seed order).\n"
    )
    lines.append(
        "| model | replications | simu seq (s) | simu ‖ (s) | SimPy seq (s) "
        "| simu parallel speedup × | simu ‖ vs SimPy × |"
    )
    lines.append("|---|---|---|---|---|---|---|")
    for r in results:
        par_vs_seq = (r["simu_seq_wall"] / r["simu_par_wall"]
                      if r["simu_par_wall"] > 0 else float("nan"))
        par_vs_simpy = (r["simpy_wall"] / r["simu_par_wall"]
                        if r["simu_par_wall"] > 0 else float("nan"))
        reps = f"{r['seeds']:,} seeds × {r['n']:,}"
        lines.append(
            f"| {r['model']} | {reps} | {r['simu_seq_wall']:.3f} "
            f"| {r['simu_par_wall']:.3f} | {r['simpy_wall']:.3f} "
            f"| {par_vs_seq:.1f}× | {par_vs_simpy:.1f}× |"
        )
    lines.append("")
    lines.append(
        f"_The **simu ‖ vs SimPy** column is the headline cross-tool advantage when "
        f"replications run in parallel: roughly the single-thread per-event speedup "
        f"times the parallel scaling (which saturates near the ~{cores}× logical-core "
        f"count and is trimmed by per-run setup and memory bandwidth). This is the axis "
        f"SimPy cannot follow — its replications are GIL-serialised._"
    )
    lines.append("")
    return "\n".join(lines)


def main():
    import argparse
    p = argparse.ArgumentParser()
    p.add_argument("--scale", type=float, default=1.0)
    p.add_argument("--no-montecarlo", action="store_true")
    a = p.parse_args()
    print(render_markdown(run_all(scale=a.scale)))
    print(render_scaling(run_all_scaling(scale=a.scale)))
    if not a.no_montecarlo:
        print(render_montecarlo(run_all_montecarlo(scale=a.scale)))


if __name__ == "__main__":
    main()
