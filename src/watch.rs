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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::dbus::{FREEDESKTOP_SCREENSAVER_INTERFACE, UPOWER_INTERFACE};

    #[test]
    fn ignores_unrelated_interface_changes() {
        assert!(!should_handle_properties_changed(
            UPOWER_INTERFACE,
            "OnBattery",
            "org.example.Other",
            &["OnBattery"],
        ));
    }

    #[test]
    fn ignores_upower_changes_without_on_battery_property() {
        assert!(!should_handle_properties_changed(
            UPOWER_INTERFACE,
            "OnBattery",
            UPOWER_INTERFACE,
            &["LidIsClosed", "DaemonVersion"],
        ));
    }

    #[test]
    fn handles_upower_on_battery_property_changes() {
        assert!(should_handle_properties_changed(
            UPOWER_INTERFACE,
            "OnBattery",
            UPOWER_INTERFACE,
            &["OnBattery"],
        ));
    }

    #[test]
    fn handles_upower_changes_when_on_battery_is_one_of_many_properties() {
        assert!(should_handle_properties_changed(
            UPOWER_INTERFACE,
            "OnBattery",
            UPOWER_INTERFACE,
            &["LidIsClosed", "OnBattery", "DaemonVersion"],
        ));
    }

    #[test]
    fn handles_lock_state_active_property_changes() {
        assert!(should_handle_properties_changed(
            FREEDESKTOP_SCREENSAVER_INTERFACE,
            "Active",
            FREEDESKTOP_SCREENSAVER_INTERFACE,
            &["Active"],
        ));
    }

    #[test]
    fn parses_lock_state_changed_signal_when_active_is_true() {
        assert_eq!(parse_lock_state_changed_signal(true), WatchEvent::LockStateChanged);
    }

    #[test]
    fn parses_lock_state_changed_signal_when_active_is_false() {
        assert_eq!(parse_lock_state_changed_signal(false), WatchEvent::LockStateChanged);
    }
}
