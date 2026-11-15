use sysinfo::System;

use super::super::state::Usage;

pub fn sample(system: &mut System) -> Usage {
    system.refresh_memory();
    Usage::from_parts(system.used_memory(), system.total_memory())
}
