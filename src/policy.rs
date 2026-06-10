pub const PROFILE_PERFORMANCE: &str = "performance";
pub const PROFILE_POWERSAVE: &str = "power-saver";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerSource {
    Ac,
    Battery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileDecision {
    Unchanged { desired_profile: &'static str },
    Change { desired_profile: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    Locked,
    Unlocked,
}

impl PowerSource {
    pub fn from_on_battery(on_battery: bool) -> Self {
        if on_battery { Self::Battery } else { Self::Ac }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ac => "ac",
            Self::Battery => "battery",
        }
    }
}

impl LockState {
    pub fn from_active(active: bool) -> Self {
        if active { Self::Locked } else { Self::Unlocked }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Locked => "true",
            Self::Unlocked => "false",
        }
    }
}

pub fn decide_profile_action(
    power_source: PowerSource,
    lock_state: Option<LockState>,
    current_profile: &str,
) -> ProfileDecision {
    let desired_profile = desired_profile(power_source, lock_state);

    if current_profile == desired_profile {
        ProfileDecision::Unchanged { desired_profile }
    } else {
        ProfileDecision::Change { desired_profile }
    }
}

pub fn desired_profile(power_source: PowerSource, lock_state: Option<LockState>) -> &'static str {
    match (power_source, lock_state) {
        (PowerSource::Battery, _) => PROFILE_POWERSAVE,
        (PowerSource::Ac, Some(LockState::Locked)) => PROFILE_POWERSAVE,
        (PowerSource::Ac, Some(LockState::Unlocked) | None) => PROFILE_PERFORMANCE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_ac_unlocked_to_performance_profile() {
        assert_eq!(desired_profile(PowerSource::Ac, Some(LockState::Unlocked)), PROFILE_PERFORMANCE);
    }

    #[test]
    fn maps_ac_locked_to_power_saver_profile() {
        assert_eq!(desired_profile(PowerSource::Ac, Some(LockState::Locked)), PROFILE_POWERSAVE);
    }

    #[test]
    fn maps_battery_to_power_saver_profile() {
        assert_eq!(desired_profile(PowerSource::Battery, Some(LockState::Unlocked)), PROFILE_POWERSAVE);
    }

    #[test]
    fn maps_ac_without_lock_provider_to_performance_profile() {
        assert_eq!(desired_profile(PowerSource::Ac, None), PROFILE_PERFORMANCE);
    }

    #[test]
    fn converts_false_on_battery_to_ac() {
        assert_eq!(PowerSource::from_on_battery(false), PowerSource::Ac);
    }

    #[test]
    fn converts_true_on_battery_to_battery() {
        assert_eq!(PowerSource::from_on_battery(true), PowerSource::Battery);
    }

    #[test]
    fn keeps_profile_unchanged_when_ac_unlocked_already_performance() {
        assert_eq!(
            decide_profile_action(PowerSource::Ac, Some(LockState::Unlocked), PROFILE_PERFORMANCE),
            ProfileDecision::Unchanged {
                desired_profile: PROFILE_PERFORMANCE,
            }
        );
    }

    #[test]
    fn changes_profile_when_ac_locked_is_not_power_saver() {
        assert_eq!(
            decide_profile_action(PowerSource::Ac, Some(LockState::Locked), PROFILE_PERFORMANCE),
            ProfileDecision::Change {
                desired_profile: PROFILE_POWERSAVE,
            }
        );
    }

    #[test]
    fn keeps_profile_unchanged_when_battery_already_power_saver() {
        assert_eq!(
            decide_profile_action(PowerSource::Battery, Some(LockState::Unlocked), PROFILE_POWERSAVE),
            ProfileDecision::Unchanged {
                desired_profile: PROFILE_POWERSAVE,
            }
        );
    }

    #[test]
    fn changes_profile_when_battery_is_not_power_saver() {
        assert_eq!(
            decide_profile_action(PowerSource::Battery, Some(LockState::Unlocked), PROFILE_PERFORMANCE),
            ProfileDecision::Change {
                desired_profile: PROFILE_POWERSAVE,
            }
        );
    }

    #[test]
    fn keeps_profile_unchanged_when_ac_without_lock_provider_already_performance() {
        assert_eq!(
            decide_profile_action(PowerSource::Ac, None, PROFILE_PERFORMANCE),
            ProfileDecision::Unchanged {
                desired_profile: PROFILE_PERFORMANCE,
            }
        );
    }

    #[test]
    fn power_source_labels_are_stable_for_logging() {
        assert_eq!(PowerSource::Ac.label(), "ac");
        assert_eq!(PowerSource::Battery.label(), "battery");
    }

    #[test]
    fn lock_state_labels_are_stable_for_logging() {
        assert_eq!(LockState::Locked.label(), "true");
        assert_eq!(LockState::Unlocked.label(), "false");
    }

    #[test]
    fn lock_state_from_active_true_is_locked() {
        assert_eq!(LockState::from_active(true), LockState::Locked);
    }

    #[test]
    fn lock_state_from_active_false_is_unlocked() {
        assert_eq!(LockState::from_active(false), LockState::Unlocked);
    }
}
