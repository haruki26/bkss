use bkss_integration::{GpuSource, Source, SourceDescriptor};
use std::path::PathBuf;

#[test]
fn source_descriptor_accepts_a_configured_drm_root() {
    fn accepts_descriptor<S: Source>(_: &SourceDescriptor<S>) {}

    let root = PathBuf::from("/tmp/drm-fixture");
    let source = GpuSource::with_root(&root);
    accepts_descriptor(&source);
}
