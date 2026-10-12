// SPDX-FileCopyrightText: Copyright (c) Siemens 2026 contributed by Christoph Kuhmuench christoph.kuhmuench@gmail.com
//
// SPDX-License-Identifier: MIT OR Apache-2.0

use simu::SimEnv;

fn main() {
    // 1. Initialize the correct simulation environment
    let mut env = SimEnv::new();

    // 2. Create the event pair via env.event() -> (EventTrigger, EventAwaitable)
    let (trigger, ready) = env.event();

    // 3. Spawn customer processes using env.handle()
    for customer_id in 1..=3 {
        let env_handle = env.handle();
        let r = ready.clone();
        env.spawn(async move {
            env_handle.timeout(f64::from(customer_id) * 2.0).await; // arrival time of customer
            println!(
                "[Time: {:.1}] Customer {} arrives and waits outside.",
                env_handle.now(),
                customer_id
            );
            // Wait for the EventTrigger to fire
            r.await;
            println!(
                "[Time: {:.1}] Customer {} goes inside to get help!",
                env_handle.now(),
                customer_id
            );
        });
    }

    // 4. Spawn a staff process using env.handle() to trigger the event
    let env_handle = env.handle();
    env.spawn(async move {
        // Wait 5.5 units of virtual simulation time
        env_handle.timeout(5.5).await;

        println!(
            "[Time: {:.1}] Technician arrives and opens the doors!",
            env_handle.now()
        );

        // Call fire() on the EventTrigger to resume all listeners
        trigger.fire();
    });

    // 5. Run the discrete event simulation loop
    env.run();
}
