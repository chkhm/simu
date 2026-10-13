# SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
#
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Coffee shop, benchmark version, in SimPy: the Python twin of
examples/coffee_shop_benchmark.rs.

This is a standalone, readable copy of compare/models/coffee.py, the SimPy
model that compare/run_comparison.sh checks seed-by-seed against the Rust
model. The simulation code is identical; what changes:

  * The portable random generator (SplitMix64 + the two sampling transforms
    the model uses) is copied in from compare/models/_feed.py, so this one
    file shows where every random number comes from.
  * The command line and the printed summary mirror the Rust example, so
    the two programs can be run side by side and compared line by line.

For the same seed, this program and the Rust example print the same metrics.

Usage (needs only SimPy: `pip install simpy`):
    python3 compare/standalone/coffee_shop_benchmark.py
    python3 compare/standalone/coffee_shop_benchmark.py --customers 2000
    python3 compare/standalone/coffee_shop_benchmark.py --seeds 100 --customers 2000

There is no --parallel option: CPython's global interpreter lock prevents
running the replications on several threads, so they always run one after
another. Units are minutes.
"""

import argparse
import math
import time

import simpy

# ---------------------------------------------------------------------------
# Portable random generator (copied from compare/models/_feed.py, which is a
# byte-for-byte twin of SplitMix64 and simu::rng::sample in src/rng.rs).
# Seeded with the same value, Rust and Python draw the same numbers.
# ---------------------------------------------------------------------------

MASK64 = (1 << 64) - 1
INV_2_POW_53 = 1.0 / 9007199254740992.0  # 2^-53


class SplitMix64:
    """Deterministic SplitMix64 generator plus the transforms the model uses."""

    __slots__ = ("state",)

    def __init__(self, seed):
        self.state = seed & MASK64

    def next_u64(self):
        self.state = (self.state + 0x9E3779B97F4A7C15) & MASK64
        z = self.state
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK64
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK64
        return z ^ (z >> 31)

    def uniform01(self):
        """Uniform float in [0, 1) from the top 53 bits of a u64."""
        return (self.next_u64() >> 11) * INV_2_POW_53

    def exponential(self, mean):
        """Exponential variate with the given mean (inverse CDF), like sample::exponential."""
        u = self.uniform01()
        return -mean * math.log1p(-u)

    def bernoulli(self, p):
        """True with probability p, consuming exactly one uniform, like sample::bernoulli."""
        return self.uniform01() < p


# ---------------------------------------------------------------------------
# The model (identical to compare/models/coffee.py)
# ---------------------------------------------------------------------------

ARRIVALS_PER_MIN = 1.7  # `--lambda` in the comparison harness
PREP_RATE = 0.5  # `--mu`: prep time ~ Exp(mean 1 / 0.5 = 2 min)

MOBILE_P = 0.3
ORDER_MEAN = 1.0
PATIENCE_MEAN = 5.0
MILK_P = 0.6
MILK_PER_DRINK = 0.25
MILK_CAP_PER_BARISTA = 2.0
RESTOCK_INTERVAL = 30.0


def run_coffee(seed, n, lam, mu, servers):
    """One replication: n customers, `servers` baristas, randomness from SplitMix64(seed)."""
    rng = SplitMix64(seed)
    env = simpy.Environment()
    cashier = simpy.Resource(env, capacity=math.ceil(servers / 2))
    barista = simpy.PriorityResource(env, capacity=servers)
    milk_cap = MILK_CAP_PER_BARISTA * servers
    milk = simpy.Container(env, capacity=milk_cap, init=milk_cap)
    # (walk_in, milk, wait_cashier, wait_barista, wait_milk, busy, in_shop, departure)
    recs = []
    counts = {"reneged": 0, "departed": 0}

    def customer(mobile, order, patience, prep, needs_milk):
        arrival = env.now
        wait_cashier = 0.0
        if not mobile:
            with cashier.request() as req:
                # Race the cashier queue against the customer's patience.
                result = yield req | env.timeout(patience)
                if req not in result:
                    counts["reneged"] += 1
                    counts["departed"] += 1
                    return
                wait_cashier = env.now - arrival
                yield env.timeout(order)

        queued = env.now
        with barista.request(priority=1 if mobile else 0) as req:
            yield req
            start = env.now
            if needs_milk:
                yield milk.get(MILK_PER_DRINK)
            wait_milk = env.now - start
            yield env.timeout(prep)

        departure = env.now
        recs.append((not mobile, needs_milk, wait_cashier, start - queued,
                     wait_milk, departure - start, departure - arrival, departure))
        counts["departed"] += 1

    def arrivals():
        # Spawn n customers, drawing every random value at arrival in a fixed order.
        for _ in range(n):
            yield env.timeout(rng.exponential(1.0 / lam))
            mobile = rng.bernoulli(MOBILE_P)
            order = rng.exponential(ORDER_MEAN)
            patience = rng.exponential(PATIENCE_MEAN)
            prep = rng.exponential(1.0 / mu)
            needs_milk = rng.bernoulli(MILK_P)
            env.process(customer(mobile, order, patience, prep, needs_milk))

    def restock():
        # Top the milk fridge up to capacity every interval until every customer has left.
        while True:
            yield env.timeout(RESTOCK_INTERVAL)
            if counts["departed"] == n:
                return
            top_up = milk.capacity - milk.level
            if top_up > 0.0:
                yield milk.put(top_up)

    env.process(arrivals())
    env.process(restock())
    env.run()

    def mean(xs):
        return sum(xs) / len(xs) if xs else 0.0

    t_end = max((r[7] for r in recs), default=0.0)
    busy = sum(r[5] for r in recs)
    return {
        "served": float(len(recs)),
        "reneged": float(counts["reneged"]),
        "mean_wait_cashier": mean([r[2] for r in recs if r[0]]),
        "mean_wait_barista": mean([r[3] for r in recs]),
        "mean_wait_milk": mean([r[4] for r in recs if r[1]]),
        "mean_time_in_shop": mean([r[6] for r in recs]),
        "barista_utilization": busy / (servers * t_end) if t_end > 0 else 0.0,
    }


# ---------------------------------------------------------------------------
# Command line + readable summary (mirrors examples/coffee_shop_benchmark.rs)
# ---------------------------------------------------------------------------


def main():
    p = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    p.add_argument("--customers", type=int, default=100_000)
    p.add_argument("--seeds", type=int, default=1)
    p.add_argument("--baristas", type=int, default=4)
    a = p.parse_args()

    t0 = time.perf_counter()
    results = [run_coffee(seed, a.customers, ARRIVALS_PER_MIN, PREP_RATE, a.baristas)
               for seed in range(a.seeds)]
    wall = time.perf_counter() - t0

    def mean(key):
        return sum(r[key] for r in results) / len(results)

    events = 2 * a.customers * a.seeds  # one arrival + one departure (or walk-out) per customer

    print(f"{a.seeds} run(s) × {a.customers} customers, {a.baristas} baristas, "
          f"{math.ceil(a.baristas / 2)} cashiers (sequential)\n")
    print("seed 0:" if a.seeds == 1 else "mean over seeds:")
    print(f"  served               {mean('served'):.2f}")
    print(f"  reneged (walk-outs)  {mean('reneged'):.2f}")
    print(f"  mean_wait_cashier    {mean('mean_wait_cashier'):.6f} min")
    print(f"  mean_wait_barista    {mean('mean_wait_barista'):.6f} min")
    print(f"  mean_wait_milk       {mean('mean_wait_milk'):.6f} min")
    print(f"  mean_time_in_shop    {mean('mean_time_in_shop'):.6f} min")
    print(f"  barista_utilization  {mean('barista_utilization'):.6f}")
    print(f"\nwall time {wall:.3f} s · {events / wall / 1e6:.2f} M events/s")


if __name__ == "__main__":
    main()
