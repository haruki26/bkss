use bkss_integration::{BacklightSource, Source, SourceDescriptor};
use std::path::PathBuf;

#[test]
fn source_descriptor_accepts_a_configured_sysfs_root() {
    fn accepts_descriptor<S: Source>(_: &SourceDescriptor<S>) {}

    let root = PathBuf::from("/tmp/backlight-fixture");
    let source = BacklightSource::with_root(&root);
    accepts_descriptor(&source);
}
