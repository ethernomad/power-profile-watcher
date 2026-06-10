#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEvent {
    PowerSourceChanged,
    LockStateChanged,
}

pub fn should_handle_properties_changed(
    expected_interface_name: &str,
    expected_property: &str,
    interface_name: &str,
    changed_properties: &[&str],
) -> bool {
    interface_name == expected_interface_name && changed_properties.contains(&expected_property)
}

pub fn parse_lock_state_changed_signal(_active: bool) -> WatchEvent {
    WatchEvent::LockStateChanged
}
