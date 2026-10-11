# SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
#
# SPDX-License-Identifier: MIT OR Apache-2.0

"""Coffee shop (scalable benchmark). SimPy twin of `compare --model coffee`.

`--servers c` baristas, `ceil(c/2)` cashiers, Poisson(`--lambda`) arrivals,
barista prep Exp(mean 1/`--mu`). Mobile-order customers skip the cashier;
walk-ins renege if the cashier queue outlasts their patience. Baristas serve
walk-ins (priority 0) ahead of mobile orders (priority 1); milk drinks draw from
a milk fridge (`simpy.Container`) topped up every 30 minutes. All random numbers
are drawn at arrival in a fixed order, mirroring `run_coffee` in
examples/compare.rs, so per-seed metrics agree exactly.
"""

import math

import simpy

from _common import run
from _feed import SplitMix64

MOBILE_P = 0.3
ORDER_MEAN = 1.0
PATIENCE_MEAN = 5.0
MILK_P = 0.6
MILK_PER_DRINK = 0.25
MILK_CAP_PER_BARISTA = 2.0
RESTOCK_INTERVAL = 30.0


def run_seed(seed, args):
    n, lam, mu, servers = args.n, args.lam, args.mu, args.servers
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
        for _ in range(n):
            yield env.timeout(rng.exponential(1.0 / lam))
            mobile = rng.bernoulli(MOBILE_P)
            order = rng.exponential(ORDER_MEAN)
            patience = rng.exponential(PATIENCE_MEAN)
            prep = rng.exponential(1.0 / mu)
            needs_milk = rng.bernoulli(MILK_P)
            env.process(customer(mobile, order, patience, prep, needs_milk))

    def restock():
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


def events_per_seed(rec, args):
    return 2 * args.n  # one arrival + one departure (or renege) per customer


if __name__ == "__main__":
    run("coffee", run_seed, events_per_seed)
