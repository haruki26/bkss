use bkss_integration::{Source, SourceDescriptor, UsageKind, UsageSource, WorkerGuard};

fn uses_worker_guard<S: Source<Guard = WorkerGuard>>(_: &SourceDescriptor<S>) {}

#[test]
fn usage_sources_share_the_worker_guard_contract() {
    uses_worker_guard(&UsageSource::new(UsageKind::Cpu));
    uses_worker_guard(&UsageSource::new(UsageKind::Memory));
    uses_worker_guard(&UsageSource::new(UsageKind::Storage));
}
