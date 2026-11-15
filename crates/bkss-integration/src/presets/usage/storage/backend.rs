use sysinfo::Disks;

use super::super::state::Usage;

pub fn sample(disks: &mut Disks) -> Usage {
    disks.refresh_list();
    let (total, available) = disks
        .list()
        .iter()
        .fold((0u64, 0u64), |(total, available), disk| {
            (
                total.saturating_add(disk.total_space()),
                available.saturating_add(disk.available_space()),
            )
        });
    Usage::from_parts(total.saturating_sub(available), total)
}
