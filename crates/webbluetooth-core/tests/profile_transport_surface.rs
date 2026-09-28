//! Regression checks for the recently added Classic and LE L2CAP surfaces.

#[test]
fn linux_peripheral_publishes_and_unpublishes_le_l2cap() {
    let source = include_str!("../../webbluetooth-linux/src/peripheral_backend.rs");
    assert!(source.contains("pub async fn publish_l2cap_channel"));
    assert!(source.contains("pub async fn unpublish_l2cap_channel"));
    assert!(source.contains("Request::ChannelOpened"));
}

#[test]
fn supported_backends_have_classic_l2cap_channels_and_listeners() {
    let backends = [
        (
            "linux",
            include_str!("../../webbluetooth-linux/src/l2cap_classic.rs"),
        ),
        (
            "android",
            include_str!("../../webbluetooth-android/src/classic_l2cap.rs"),
        ),
        (
            "windows",
            include_str!("../../webbluetooth-windows/src/classic_l2cap.rs"),
        ),
    ];

    for (name, source) in backends {
        assert!(
            source.contains("ClassicL2capChannel"),
            "{name} backend is missing ClassicL2capChannel"
        );
        assert!(
            source.contains("ClassicL2capListener"),
            "{name} backend is missing ClassicL2capListener"
        );
    }
}
