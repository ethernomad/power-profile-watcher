use std::error::Error;

use futures_util::StreamExt;
use tokio::signal;
use zbus::fdo::PropertiesProxy;
use zbus::Connection;
use zbus::names::InterfaceName;
use zvariant::Value;

const POWER_PROFILES_DESTINATION: &str = "net.hadess.PowerProfiles";
const POWER_PROFILES_PATH: &str = "/net/hadess/PowerProfiles";
const POWER_PROFILES_INTERFACE: &str = "net.hadess.PowerProfiles";
const UPOWER_DESTINATION: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const UPOWER_INTERFACE: &str = "org.freedesktop.UPower";
const PROFILE_PERFORMANCE: &str = "performance";
const PROFILE_POWERSAVE: &str = "power-saver";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerSource {
    Ac,
    Battery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProfileDecision {
    Unchanged { desired_profile: &'static str },
    Change { desired_profile: &'static str },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let connection = Connection::system().await?;

    apply_profile_for_current_power_source(&connection).await?;

    let properties_proxy = PropertiesProxy::builder(&connection)
        .destination(UPOWER_DESTINATION)?
        .path(UPOWER_PATH)?
        .build()
        .await?;
    let mut changes = properties_proxy.receive_properties_changed().await?;

    println!("Watching UPower for power-source changes");

    loop {
        tokio::select! {
            maybe_signal = changes.next() => {
                let Some(signal) = maybe_signal else {
                    return Err("UPower properties stream ended".into());
                };

                let args = signal.args()?;
                let changed_property_names: Vec<&str> = args
                    .changed_properties
                    .keys()
                    .map(|name| <_ as AsRef<str>>::as_ref(name))
                    .collect();
                if !should_handle_properties_changed(
                    args.interface_name.as_str(),
                    &changed_property_names,
                ) {
                    continue;
                }

                apply_profile_for_current_power_source(&connection).await?;
            }
            ctrl_c = signal::ctrl_c() => {
                ctrl_c?;
                println!("Received shutdown signal");
                break;
            }
        }
    }

    Ok(())
}

async fn apply_profile_for_current_power_source(connection: &Connection) -> Result<(), Box<dyn Error>> {
    let power_source = current_power_source(connection).await?;
    let current_profile = active_profile(connection).await?;
    match decide_profile_action(power_source, &current_profile) {
        ProfileDecision::Unchanged { desired_profile } => {
            println!(
                "Power source unchanged for profile selection: source={}, profile={desired_profile}",
                power_source.label()
            );
        }
        ProfileDecision::Change { desired_profile } => {
            set_active_profile(connection, desired_profile).await?;
            println!(
                "Set active profile to {desired_profile} (source={})",
                power_source.label()
            );
        }
    }

    Ok(())
}

async fn current_power_source(connection: &Connection) -> Result<PowerSource, Box<dyn Error>> {
    let properties_proxy = PropertiesProxy::builder(connection)
        .destination(UPOWER_DESTINATION)?
        .path(UPOWER_PATH)?
        .build()
        .await?;
    let value = properties_proxy
        .get(
            InterfaceName::try_from(UPOWER_INTERFACE)?,
            "OnBattery",
        )
        .await?;
    let value: bool = value.try_into()?;
    Ok(PowerSource::from_on_battery(value))
}

async fn active_profile(connection: &Connection) -> Result<String, Box<dyn Error>> {
    let properties_proxy = PropertiesProxy::builder(connection)
        .destination(POWER_PROFILES_DESTINATION)?
        .path(POWER_PROFILES_PATH)?
        .build()
        .await?;
    let profile = properties_proxy
        .get(
            InterfaceName::try_from(POWER_PROFILES_INTERFACE)?,
            "ActiveProfile",
        )
        .await?;
    let profile: String = profile.try_into()?;
    Ok(profile)
}

async fn set_active_profile(connection: &Connection, profile: &str) -> Result<(), Box<dyn Error>> {
    let properties_proxy = PropertiesProxy::builder(connection)
        .destination(POWER_PROFILES_DESTINATION)?
        .path(POWER_PROFILES_PATH)?
        .build()
        .await?;
    let value = Value::from(profile);
    properties_proxy
        .set(
            InterfaceName::try_from(POWER_PROFILES_INTERFACE)?,
            "ActiveProfile",
            &value,
        )
        .await?;
    Ok(())
}

fn decide_profile_action(power_source: PowerSource, current_profile: &str) -> ProfileDecision {
    let desired_profile = power_source.desired_profile();

    if current_profile == desired_profile {
        ProfileDecision::Unchanged { desired_profile }
    } else {
        ProfileDecision::Change { desired_profile }
    }
}

fn should_handle_properties_changed(interface_name: &str, changed_properties: &[&str]) -> bool {
    interface_name == UPOWER_INTERFACE && changed_properties.contains(&"OnBattery")
}

impl PowerSource {
    fn from_on_battery(on_battery: bool) -> Self {
        if on_battery {
            Self::Battery
        } else {
            Self::Ac
        }
    }

    fn desired_profile(self) -> &'static str {
        match self {
            Self::Ac => PROFILE_PERFORMANCE,
            Self::Battery => PROFILE_POWERSAVE,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Ac => "ac",
            Self::Battery => "battery",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_ac_to_performance_profile() {
        assert_eq!(PowerSource::Ac.desired_profile(), PROFILE_PERFORMANCE);
    }

    #[test]
    fn maps_battery_to_power_saver_profile() {
        assert_eq!(PowerSource::Battery.desired_profile(), PROFILE_POWERSAVE);
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
    fn keeps_profile_unchanged_when_ac_already_performance() {
        assert_eq!(
            decide_profile_action(PowerSource::Ac, PROFILE_PERFORMANCE),
            ProfileDecision::Unchanged {
                desired_profile: PROFILE_PERFORMANCE,
            }
        );
    }

    #[test]
    fn changes_profile_when_ac_is_not_performance() {
        assert_eq!(
            decide_profile_action(PowerSource::Ac, PROFILE_POWERSAVE),
            ProfileDecision::Change {
                desired_profile: PROFILE_PERFORMANCE,
            }
        );
    }

    #[test]
    fn keeps_profile_unchanged_when_battery_already_power_saver() {
        assert_eq!(
            decide_profile_action(PowerSource::Battery, PROFILE_POWERSAVE),
            ProfileDecision::Unchanged {
                desired_profile: PROFILE_POWERSAVE,
            }
        );
    }

    #[test]
    fn changes_profile_when_battery_is_not_power_saver() {
        assert_eq!(
            decide_profile_action(PowerSource::Battery, PROFILE_PERFORMANCE),
            ProfileDecision::Change {
                desired_profile: PROFILE_POWERSAVE,
            }
        );
    }

    #[test]
    fn ignores_unrelated_interface_changes() {
        assert!(!should_handle_properties_changed(
            "org.example.Other",
            &["OnBattery"],
        ));
    }

    #[test]
    fn ignores_upower_changes_without_on_battery_property() {
        assert!(!should_handle_properties_changed(
            UPOWER_INTERFACE,
            &["LidIsClosed", "DaemonVersion"],
        ));
    }

    #[test]
    fn handles_upower_on_battery_property_changes() {
        assert!(should_handle_properties_changed(
            UPOWER_INTERFACE,
            &["OnBattery"],
        ));
    }

    #[test]
    fn handles_upower_changes_when_on_battery_is_one_of_many_properties() {
        assert!(should_handle_properties_changed(
            UPOWER_INTERFACE,
            &["LidIsClosed", "OnBattery", "DaemonVersion"],
        ));
    }

    #[test]
    fn power_source_labels_are_stable_for_logging() {
        assert_eq!(PowerSource::Ac.label(), "ac");
        assert_eq!(PowerSource::Battery.label(), "battery");
    }
}
