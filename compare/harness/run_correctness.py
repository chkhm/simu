# SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
#
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Correctness comparison: are simu and SimPy indistinguishable?

Both tools now draw from the *same* portable feed (SplitMix64 + shared
transforms — see `compare/models/_feed.py` and `src/rng.rs`), seeded identically
per replication. So for the order-insensitive models the comparison runs in
**exact mode**: seed-by-seed, every metric must agree to a tight relative
tolerance (`--exact-tol`, default 1e-9). The only residual is floating-point
summation order, which lands ~1e-15.

One model is *not* exact-eligible and stays on the distributional test:

  * `hospital` — the `early_discharged` metric is sensitive to simultaneous-event
    tie-breaking during the eviction handoff, which legitimately differs between
    the engines' execution models (see `compare/README.md`). It is an *engine*
    ordering difference, not an RNG one, so a shared feed cannot remove it.

For distributional (non-exact) models a metric FAILS only when the difference is
both **statistically significant** (Welch's t-test p < alpha) **and material**
(relative mean difference > tol) — the "significant AND material" gate avoids the
large-sample trap where a negligible difference becomes significant purely
because R is large.
"""

import math

from scipy import stats

import contract

# Models whose draw stream AND scheduling align seed-by-seed, so they support
# exact per-seed comparison. `hospital` is excluded (eviction-handoff ordering).
EXACT_MODELS = {"mm1", "mmc", "priority", "container", "coffee"}

# Accepted, documented divergences keyed by (model, metric). A failure here is
# reported as a *known exception* — surfaced in the table but NOT counted as a
# build failure — so CI can gate on real regressions only. Each entry must have
# a corresponding explanation in compare/README.md. Keep this list as small as
# the evidence allows: anything not listed that fails is a hard FAIL.
KNOWN_EXCEPTIONS = {
    # Eviction-handoff event ordering at identical timestamps differs between
    # simu (poll/waker) and SimPy (generator/callback); not an RNG difference,
    # so the shared feed cannot remove it. See compare/README.md.
    ("hospital", "early_discharged"),
}


def _summary(xs):
    n = len(xs)
    mean = sum(xs) / n if n else 0.0
    sem = (stats.tstd(xs) / math.sqrt(n)) if n > 1 else 0.0
    return mean, sem


def _max_rel_per_seed(a, b):
    """Largest relative difference between aligned per-seed values."""
    if len(a) != len(b):
        return float("nan")
    worst = 0.0
    for ai, bi in zip(a, b):
        denom = max(abs(ai), abs(bi), 1e-12)
        worst = max(worst, abs(ai - bi) / denom)
    return worst


def compare_metric(a, b, tol, alpha, exact, exact_tol):
    """Compare metric arrays `a` (simu) and `b` (simpy), aligned by seed."""
    mean_a, sem_a = _summary(a)
    mean_b, sem_b = _summary(b)
    denom = max(abs(mean_a), abs(mean_b), 1e-12)
    rel_diff = abs(mean_a - mean_b) / denom
    max_rel = _max_rel_per_seed(a, b)

    # Welch t-test on means; KS on the whole distribution (informational).
    if len(set(a)) <= 1 and len(set(b)) <= 1:
        t_p = 1.0 if mean_a == mean_b else 0.0
    else:
        t_p = float(stats.ttest_ind(a, b, equal_var=False).pvalue)
    # method="asymp": several metrics are small integers with many ties, for
    # which scipy's default exact KS calculation is invalid and warns before
    # falling back to the asymptotic one anyway. Ask for asymptotic up front.
    ks_p = float(stats.ks_2samp(a, b, method="asymp").pvalue)

    if exact:
        # Shared feed ⇒ identical draws ⇒ per-seed metrics must match exactly
        # (to floating-point tolerance).
        passed = max_rel <= exact_tol
    else:
        significant = t_p < alpha
        material = rel_diff > tol
        passed = not (significant and material)
    return {
        "mean_simu": mean_a, "sem_simu": sem_a,
        "mean_simpy": mean_b, "sem_simpy": sem_b,
        "rel_diff": rel_diff, "max_rel_per_seed": max_rel,
        "t_p": t_p, "ks_p": ks_p,
        "passed": passed,
    }


def compare_model(model, seeds, n, lam, mu, servers, tol, alpha, exact_tol):
    exact = model in EXACT_MODELS
    simu = contract.run_simu(model, seeds, n, lam, mu, servers)
    simpy = contract.run_simpy(model, seeds, n, lam, mu, servers)
    keys = contract.metric_keys(simu)
    rows = []
    for key in keys:
        a = contract.column(simu, key)
        b = contract.column(simpy, key)
        row = compare_metric(a, b, tol, alpha, exact, exact_tol)
        row["metric"] = key
        # A failing metric on the allowlist is a tolerated, documented exception.
        row["known"] = (model, key) in KNOWN_EXCEPTIONS
        rows.append(row)
    hard_failures = [r for r in rows if not r["passed"] and not r["known"]]
    known_failures = [r for r in rows if not r["passed"] and r["known"]]
    return {
        "model": model, "seeds": seeds, "n": n,
        "lam": lam, "mu": mu, "servers": servers,
        "exact": exact,
        "rows": rows,
        # Genuinely clean (every metric matched).
        "genuine_pass": all(r["passed"] for r in rows),
        # Build-gating status: clean OR only known exceptions failed.
        "all_passed": len(hard_failures) == 0,
        "n_known": len(known_failures),
    }


# Closed-form M/M/c mean wait in queue (Erlang C). Used as an extra ground-truth
# check that *both* tools land near theory, independent of each other.
def erlang_c_wq(lam, mu, c):
    a = lam / mu
    rho = a / c
    if rho >= 1.0:
        return float("nan")
    s = sum(a ** k / math.factorial(k) for k in range(c))
    last = a ** c / (math.factorial(c) * (1 - rho))
    p_wait = last / (s + last)
    return p_wait / (c * mu - lam)


def default_config(scale=1.0):
    s = max(int(200 * scale), 20)
    n = max(int(1000 * scale), 200)
    return [
        dict(model="mm1", seeds=s, n=n, lam=0.85, mu=1.0, servers=1),
        dict(model="mmc", seeds=s, n=n, lam=1.6, mu=1.0, servers=2),
        dict(model="priority", seeds=s, n=n, lam=0.85, mu=1.0, servers=1),
        dict(model="container", seeds=s, n=n, lam=0.9, mu=1.0, servers=1),
        dict(model="coffee", seeds=s, n=n, lam=1.7, mu=0.5, servers=4),
        dict(model="hospital", seeds=s, n=n, lam=0.9, mu=1.0, servers=1),
    ]


def run_all(scale=1.0, tol=0.05, alpha=0.01, exact_tol=1e-9):
    results = []
    for cfg in default_config(scale):
        results.append(compare_model(tol=tol, alpha=alpha, exact_tol=exact_tol, **cfg))
    return results


def render_markdown(results):
    lines = ["## Correctness: simu vs. SimPy", ""]
    lines.append(
        "Both tools draw from the same portable feed (SplitMix64 + shared "
        "transforms), seeded identically per replication.\n"
    )
    lines.append(
        "- **Exact mode** (mm1, mmc, priority, container, coffee): every metric must "
        "agree **seed-by-seed** within 1e-9 relative; FAIL otherwise.\n"
        "- **Distributional mode** (hospital): FAIL = statistically significant "
        "(t-test p < 0.01) **and** material (relative mean difference > 5%).\n"
        "- **Known exceptions** (marked `known ⚠`): accepted, documented "
        "divergences that are surfaced but do **not** fail the run — currently "
        "`hospital.early_discharged` (eviction-handoff event ordering; see "
        "README). The harness exits non-zero only on a non-allowlisted FAIL.\n"
    )
    for res in results:
        if res["genuine_pass"]:
            status = "✅ PASS"
        elif res["all_passed"]:
            n = res["n_known"]
            status = f"✅ PASS ({n} known exception{'s' if n != 1 else ''})"
        else:
            status = "❌ FAIL"
        mode = "exact" if res["exact"] else "distributional"
        lines.append(
            f"### {res['model']}  ({status}, {mode}) — "
            f"{res['seeds']} seeds × {res['n']} arrivals, "
            f"λ={res['lam']}, μ={res['mu']}, c={res['servers']}"
        )
        lines.append("")
        lines.append(
            "| metric | simu mean | simpy mean | rel.diff | max/seed Δ | "
            "t-test p | KS p | result |"
        )
        lines.append("|---|---|---|---|---|---|---|---|")
        for r in res["rows"]:
            if r["passed"]:
                mark = "pass"
            elif r["known"]:
                mark = "known ⚠"
            else:
                mark = "**FAIL**"
            lines.append(
                f"| {r['metric']} | {r['mean_simu']:.4g} ± {r['sem_simu']:.2g} "
                f"| {r['mean_simpy']:.4g} ± {r['sem_simpy']:.2g} "
                f"| {r['rel_diff']*100:.2f}% | {r['max_rel_per_seed']:.1e} "
                f"| {r['t_p']:.3f} | {r['ks_p']:.3f} | {mark} |"
            )
        # Analytical ground-truth note for queue models.
        if res["model"] in ("mm1", "mmc"):
            wq = erlang_c_wq(res["lam"], res["mu"], res["servers"])
            simu_wq = next(r["mean_simu"] for r in res["rows"] if r["metric"] == "mean_wait")
            simpy_wq = next(r["mean_simpy"] for r in res["rows"] if r["metric"] == "mean_wait")
            lines.append("")
            lines.append(
                f"_Analytical mean wait (Erlang C) = **{wq:.3f}**; "
                f"simu = {simu_wq:.3f}, simpy = {simpy_wq:.3f}._"
            )
        # Expected-divergence note for the hospital model.
        if res["model"] == "hospital":
            lines.append("")
            lines.append(
                "_Note: differences here are **expected**, not a bug. Both engines "
                "draw the identical random stream, so every metric that depends on "
                "the random values matches exactly. The diverging metrics "
                "(`early_discharged`, and its cascade into `mean_bed_wait` / "
                "`ed_cleared_at`) depend instead on the **order in which events "
                "scheduled at the same simulated timestamp fire** — which is "
                "implementation-specific and differs between simu (poll/waker) and "
                "SimPy (generator/callback) during the bed-eviction handoff. No "
                "random number is consumed at that decision point, so a shared feed "
                "cannot remove the difference. See `compare/README.md`._"
            )
        lines.append("")
    return "\n".join(lines)


def main():
    import argparse
    p = argparse.ArgumentParser()
    p.add_argument("--scale", type=float, default=1.0)
    p.add_argument("--tol", type=float, default=0.05)
    p.add_argument("--alpha", type=float, default=0.01)
    p.add_argument("--exact-tol", dest="exact_tol", type=float, default=1e-9)
    a = p.parse_args()
    results = run_all(scale=a.scale, tol=a.tol, alpha=a.alpha, exact_tol=a.exact_tol)
    print(render_markdown(results))
    # Known exceptions (allowlisted divergences) are surfaced above but do not
    # fail the run — only a non-allowlisted FAIL does.
    failed = [r["model"] for r in results if not r["all_passed"]]
    if failed:
        raise SystemExit(f"correctness FAILED for: {', '.join(failed)}")


if __name__ == "__main__":
    main()
