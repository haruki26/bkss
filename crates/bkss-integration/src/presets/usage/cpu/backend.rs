use sysinfo::System;

use super::super::state::Usage;

pub fn sample(system: &mut System) -> Usage {
    system.refresh_cpu_usage();
    Usage {
        used: 0,
        total: 100,
        percent: system.global_cpu_usage().clamp(0.0, 100.0),
    }
}
