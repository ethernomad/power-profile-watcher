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
