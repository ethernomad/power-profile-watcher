use std::error::Error;

use futures_util::StreamExt;
use tokio::sync::mpsc;
use zbus::fdo::PropertiesProxy;
use zbus::names::InterfaceName;
use zbus::zvariant::Value;
use zbus::{Connection, Proxy};

use crate::policy::{LockState, PowerSource};
use crate::watch::{WatchEvent, parse_lock_state_changed_signal, should_handle_properties_changed};

pub const POWER_PROFILES_DESTINATION: &str = "net.hadess.PowerProfiles";
pub const POWER_PROFILES_PATH: &str = "/net/hadess/PowerProfiles";
pub const POWER_PROFILES_INTERFACE: &str = "net.hadess.PowerProfiles";
pub const UPOWER_DESTINATION: &str = "org.freedesktop.UPower";
pub const UPOWER_PATH: &str = "/org/freedesktop/UPower";
pub const UPOWER_INTERFACE: &str = "org.freedesktop.UPower";
pub const FREEDESKTOP_SCREENSAVER_DESTINATION: &str = "org.freedesktop.ScreenSaver";
pub const FREEDESKTOP_SCREENSAVER_PATH: &str = "/org/freedesktop/ScreenSaver";
pub const FREEDESKTOP_SCREENSAVER_INTERFACE: &str = "org.freedesktop.ScreenSaver";
pub const GNOME_SCREENSAVER_DESTINATION: &str = "org.gnome.ScreenSaver";
pub const GNOME_SCREENSAVER_PATH: &str = "/org/gnome/ScreenSaver";
pub const GNOME_SCREENSAVER_INTERFACE: &str = "org.gnome.ScreenSaver";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockStateProvider {
    pub destination: &'static str,
    pub path: &'static str,
    pub interface: &'static str,
}

pub async fn verify_upower_available(connection: &Connection) -> Result<(), Box<dyn Error>> {
    let properties_proxy = PropertiesProxy::builder(connection)
        .destination(UPOWER_DESTINATION)?
        .path(UPOWER_PATH)?
        .build()
        .await?;
    let _: bool = properties_proxy
        .get(InterfaceName::try_from(UPOWER_INTERFACE)?, "OnBattery")
        .await?
        .try_into()?;
    Ok(())
}

pub async fn verify_power_profiles_available(connection: &Connection) -> Result<(), Box<dyn Error>> {
    let properties_proxy = PropertiesProxy::builder(connection)
        .destination(POWER_PROFILES_DESTINATION)?
        .path(POWER_PROFILES_PATH)?
        .build()
        .await?;
    let _: zbus::zvariant::OwnedValue = properties_proxy
        .get(
            InterfaceName::try_from(POWER_PROFILES_INTERFACE)?,
            "Profiles",
        )
        .await?;
    Ok(())
}

pub async fn current_power_source(connection: &Connection) -> Result<PowerSource, Box<dyn Error>> {
    let properties_proxy = PropertiesProxy::builder(connection)
        .destination(UPOWER_DESTINATION)?
        .path(UPOWER_PATH)?
        .build()
        .await?;
    let value = properties_proxy
        .get(InterfaceName::try_from(UPOWER_INTERFACE)?, "OnBattery")
        .await?;
    let value: bool = value.try_into()?;
    Ok(PowerSource::from_on_battery(value))
}

pub async fn active_profile(connection: &Connection) -> Result<String, Box<dyn Error>> {
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

pub async fn set_active_profile(connection: &Connection, profile: &str) -> Result<(), Box<dyn Error>> {
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
            value,
        )
        .await?;
    Ok(())
}

pub async fn discover_lock_state_provider(
    session_connection: &Connection,
) -> Result<Option<LockStateProvider>, Box<dyn Error>> {
    for provider in [
        LockStateProvider {
            destination: FREEDESKTOP_SCREENSAVER_DESTINATION,
            path: FREEDESKTOP_SCREENSAVER_PATH,
            interface: FREEDESKTOP_SCREENSAVER_INTERFACE,
        },
        LockStateProvider {
            destination: GNOME_SCREENSAVER_DESTINATION,
            path: GNOME_SCREENSAVER_PATH,
            interface: GNOME_SCREENSAVER_INTERFACE,
        },
    ] {
        if lock_state(session_connection, &provider).await.is_ok() {
            return Ok(Some(provider));
        }
    }

    Ok(None)
}

pub async fn current_lock_state(
    session_connection: &Connection,
    lock_state_provider: Option<&LockStateProvider>,
) -> Result<Option<LockState>, Box<dyn Error>> {
    match lock_state_provider {
        Some(provider) => Ok(Some(lock_state(session_connection, provider).await?)),
        None => Ok(None),
    }
}

pub async fn lock_state(
    session_connection: &Connection,
    provider: &LockStateProvider,
) -> Result<LockState, Box<dyn Error>> {
    let proxy = Proxy::new(
        session_connection,
        provider.destination,
        provider.path,
        provider.interface,
    )
    .await?;
    let active: bool = proxy.call("GetActive", &()).await?;
    Ok(LockState::from_active(active))
}

pub fn spawn_upower_watcher(connection: Connection, event_tx: mpsc::Sender<Result<WatchEvent, String>>) {
    tokio::spawn(async move {
        let result: Result<(), String> = async {
            let properties_proxy = PropertiesProxy::builder(&connection)
                .destination(UPOWER_DESTINATION)
                .map_err(|err| err.to_string())?
                .path(UPOWER_PATH)
                .map_err(|err| err.to_string())?
                .build()
                .await
                .map_err(|err| err.to_string())?;
            let mut changes = properties_proxy
                .receive_properties_changed()
                .await
                .map_err(|err| err.to_string())?;

            loop {
                let Some(signal) = changes.next().await else {
                    return Err("UPower properties stream ended".to_string());
                };

                let args = signal.args().map_err(|err| err.to_string())?;
                let changed_property_names: Vec<&str> = args
                    .changed_properties
                    .keys()
                    .map(<_ as AsRef<str>>::as_ref)
                    .collect();
                if should_handle_properties_changed(
                    UPOWER_INTERFACE,
                    "OnBattery",
                    args.interface_name.as_str(),
                    &changed_property_names,
                ) && event_tx.send(Ok(WatchEvent::PowerSourceChanged)).await.is_err()
                {
                    break;
                }
            }

            Ok(())
        }
        .await;

        if let Err(err) = result {
            let _ = event_tx.send(Err(err)).await;
        }
    });
}

pub fn spawn_lock_state_watcher(
    connection: Connection,
    provider: LockStateProvider,
    event_tx: mpsc::Sender<Result<WatchEvent, String>>,
) {
    tokio::spawn(async move {
        let result: Result<(), String> = async {
            let proxy = Proxy::new(
                &connection,
                provider.destination,
                provider.path,
                provider.interface,
            )
            .await
            .map_err(|err| err.to_string())?;
            let mut changes = proxy
                .receive_signal("ActiveChanged")
                .await
                .map_err(|err| err.to_string())?;

            loop {
                let Some(signal) = changes.next().await else {
                    return Err("lock-state signal stream ended".to_string());
                };

                let active: bool = signal
                    .body()
                    .deserialize()
                    .map_err(|err| err.to_string())?;
                let event = parse_lock_state_changed_signal(active);
                if event_tx.send(Ok(event)).await.is_err() {
                    break;
                }
            }

            Ok(())
        }
        .await;

        if let Err(err) = result {
            let _ = event_tx.send(Err(err)).await;
        }
    });
}
