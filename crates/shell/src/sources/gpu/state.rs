#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GpuSupport {
    Amd,
    Intel,
    Unsupported,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GpuMetrics {
    /// Intel reports the busiest engine, not aggregate GPU utilization.
    pub utilization_percent: Option<f32>,
    pub memory_used_bytes: Option<u64>,
    pub memory_total_bytes: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub vendor_id: Option<String>,
    pub device_id: Option<String>,
    pub support: GpuSupport,
    pub metrics: GpuMetrics,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuState {
    pub devices: Vec<Gpu>,
}
