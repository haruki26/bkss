/// Selects which system resource the usage source samples.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum UsageKind {
    Cpu,
    Memory,
    Storage,
}

/// A sampled usage value.
///
/// For CPU samples, only `percent` is meaningful: `used` and `total` are
/// placeholders. For memory and storage, `used` and `total` are measured
/// quantities in bytes, and `percent` is calculated from them.
#[derive(Clone, Debug, PartialEq)]
pub struct Usage {
    pub used: u64,
    pub total: u64,
    pub percent: f32,
}

impl Usage {
    pub fn from_parts(used: u64, total: u64) -> Self {
        Self {
            used,
            total,
            percent: if total == 0 {
                0.0
            } else {
                (used as f64 * 100.0 / total as f64).clamp(0.0, 100.0) as f32
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentage_is_bounded_and_zero_total_is_safe() {
        assert_eq!(Usage::from_parts(1, 0).percent, 0.0);
        assert_eq!(Usage::from_parts(8, 4).percent, 100.0);
        assert_eq!(Usage::from_parts(1, 4).percent, 25.0);
    }
}
