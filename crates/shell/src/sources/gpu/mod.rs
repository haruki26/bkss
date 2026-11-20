mod newintel;
mod source;
mod state;
pub use source::GpuSource;
pub use state::GpuState;
#[cfg(test)]
pub use state::{Gpu, GpuMetrics, GpuSupport};
