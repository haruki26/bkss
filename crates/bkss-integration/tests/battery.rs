use bkss_integration::{BatteryEndpoint, BatterySource, Source, SourceDescriptor};

#[test]
fn source_identity_includes_its_bus_endpoint() {
    fn accepts_descriptor<S: Source>(_: &SourceDescriptor<S>) {}

    let system = BatterySource::new();
    let fake =
        BatterySource::with_endpoint(BatteryEndpoint::address("unix:path=/tmp/bkss-test-bus"));
    accepts_descriptor(&system);
    accepts_descriptor(&fake);
    assert_ne!(
        BatteryEndpoint::system(),
        BatteryEndpoint::address("unix:path=/tmp/bkss-test-bus")
    );
}
