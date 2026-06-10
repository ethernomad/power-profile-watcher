mod cli;
mod dbus;
mod policy;
mod service;
mod watch;

use std::error::Error;

use clap::Parser;
use tokio::signal;
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use zbus::Connection;

use crate::cli::{Cli, Commands};
use crate::dbus::{
    LockStateProvider, active_profile, current_lock_state, current_power_source,
    discover_lock_state_provider, set_active_profile, spawn_lock_state_watcher,
    spawn_upower_watcher, verify_power_profiles_available, verify_upower_available,
};
use crate::policy::{LockState, ProfileDecision, decide_profile_action};
use crate::service::{install_service, uninstall_service, verify_service};
use crate::watch::WatchEvent;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let filter = resolve_filter(&cli);

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let result = match cli.command {
        Some(Commands::Install) => install_service().await,
        Some(Commands::Verify) => verify_service().await,
        Some(Commands::Uninstall) => uninstall_service().await,
        None => run().await,
    };

    if let Err(err) = result {
        error!(%err, "daemon failed");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let system_connection = Connection::system().await?;
    let session_connection = Connection::session().await?;

    verify_upower_available(&system_connection).await?;
    verify_power_profiles_available(&system_connection).await?;
    let lock_state_provider = discover_lock_state_provider(&session_connection).await?;

    apply_profile_for_current_state(
        &system_connection,
        &session_connection,
        lock_state_provider.as_ref(),
    )
    .await?;

    let (event_tx, mut event_rx) = mpsc::channel::<Result<WatchEvent, String>>(8);

    spawn_upower_watcher(system_connection.clone(), event_tx.clone());

    if let Some(provider) = lock_state_provider {
        spawn_lock_state_watcher(session_connection.clone(), provider, event_tx.clone());
        info!(
            destination = provider.destination,
            interface = provider.interface,
            path = provider.path,
            "watching session lock-state changes"
        );
    } else {
        warn!("no compatible session lock-state provider found; falling back to AC/battery-only behavior");
    }

    info!("watching UPower for power-source changes");

    loop {
        tokio::select! {
            maybe_event = event_rx.recv() => {
                let Some(event) = maybe_event else {
                    return Err("event stream ended unexpectedly".into());
                };

                match event {
                    Ok(WatchEvent::PowerSourceChanged) | Ok(WatchEvent::LockStateChanged) => {
                        apply_profile_for_current_state(
                            &system_connection,
                            &session_connection,
                            lock_state_provider.as_ref(),
                        ).await?;
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            ctrl_c = signal::ctrl_c() => {
                ctrl_c?;
                info!("received shutdown signal");
                break;
            }
        }
    }

    Ok(())
}

fn resolve_filter(cli: &Cli) -> tracing_subscriber::EnvFilter {
    if std::env::var_os("RUST_LOG").is_some() {
        tracing_subscriber::EnvFilter::from_default_env()
    } else {
        let level = verbosity_level(cli.verbose, cli.quiet).to_string();
        tracing_subscriber::EnvFilter::new(level)
    }
}

fn verbosity_level(verbose: u8, quiet: u8) -> &'static str {
    let delta = verbose as i16 - quiet as i16;
    match delta {
        i16::MIN..=-2 => "error",
        -1 => "warn",
        0 => "info",
        1 => "debug",
        2..=i16::MAX => "trace",
    }
}

async fn apply_profile_for_current_state(
    system_connection: &Connection,
    session_connection: &Connection,
    lock_state_provider: Option<&LockStateProvider>,
) -> Result<(), Box<dyn Error>> {
    let power_source = current_power_source(system_connection).await?;
    let lock_state = current_lock_state(session_connection, lock_state_provider).await?;
    let current_profile = active_profile(system_connection).await?;
    match decide_profile_action(power_source, lock_state, &current_profile) {
        ProfileDecision::Unchanged { desired_profile } => {
            info!(
                source = power_source.label(),
                locked = lock_state.map(LockState::label),
                profile = desired_profile,
                "current state already matches desired profile"
            );
        }
        ProfileDecision::Change { desired_profile } => {
            set_active_profile(system_connection, desired_profile).await?;
            info!(
                source = power_source.label(),
                locked = lock_state.map(LockState::label),
                profile = desired_profile,
                "set active profile"
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use clap::CommandFactory;

    use crate::cli::clap_styles;
    use crate::dbus::{FREEDESKTOP_SCREENSAVER_INTERFACE, UPOWER_INTERFACE};
    use crate::policy::{PowerSource, PROFILE_PERFORMANCE, PROFILE_POWERSAVE, desired_profile};
    use crate::service::{
        parse_exec_start, parse_systemctl_is_active, render_service_unit, service_dir,
        unescape_systemd_exec_argument, verify_service_executable,
    };
    use crate::watch::{parse_lock_state_changed_signal, should_handle_properties_changed};

    #[test]
    fn clap_styles_build_without_panicking() {
        let _ = clap_styles();
    }

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
    fn defaults_to_info_when_no_rust_log_and_no_verbosity_flags() {
        unsafe { std::env::remove_var("RUST_LOG") };
        let cli = Cli {
            verbose: 0,
            quiet: 0,
            command: None,
        };

        let filter = resolve_filter(&cli);
        assert_eq!(filter.to_string(), "info");
    }

    #[test]
    fn uses_rust_log_when_present() {
        unsafe { std::env::set_var("RUST_LOG", "debug") };
        let cli = Cli {
            verbose: 2,
            quiet: 0,
            command: None,
        };

        let filter = resolve_filter(&cli);
        unsafe { std::env::remove_var("RUST_LOG") };

        assert_eq!(filter.to_string(), "debug");
    }

    #[test]
    fn quiet_flag_reduces_default_info_to_warn() {
        unsafe { std::env::remove_var("RUST_LOG") };
        let cli = Cli {
            verbose: 0,
            quiet: 1,
            command: None,
        };

        let filter = resolve_filter(&cli);
        assert_eq!(filter.to_string(), "warn");
    }

    #[test]
    fn double_verbose_increases_default_info_to_trace() {
        unsafe { std::env::remove_var("RUST_LOG") };
        let cli = Cli {
            verbose: 2,
            quiet: 0,
            command: None,
        };

        let filter = resolve_filter(&cli);
        assert_eq!(filter.to_string(), "trace");
    }

    #[test]
    fn install_service_subcommand_parses() {
        let cli = Cli::parse_from(["power-profile-watcher", "install-service"]);
        assert!(matches!(cli.command, Some(Commands::Install)));
    }

    #[test]
    fn uninstall_service_subcommand_parses() {
        let cli = Cli::parse_from(["power-profile-watcher", "uninstall-service"]);
        assert!(matches!(cli.command, Some(Commands::Uninstall)));
    }

    #[test]
    fn verify_service_subcommand_parses() {
        let cli = Cli::parse_from(["power-profile-watcher", "verify-service"]);
        assert!(matches!(cli.command, Some(Commands::Verify)));
    }

    #[test]
    fn verify_service_subcommand_has_updated_help_text() {
        let command = Cli::command();
        let verify_service = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "verify-service")
            .expect("verify-service subcommand should exist");

        assert_eq!(
            verify_service.get_about().map(ToString::to_string),
            Some("Verify the installed systemd user service".to_string())
        );
    }

    #[test]
    fn uninstall_service_subcommand_has_updated_help_text() {
        let command = Cli::command();
        let uninstall_service = command
            .get_subcommands()
            .find(|subcommand| subcommand.get_name() == "uninstall-service")
            .expect("uninstall-service subcommand should exist");

        assert_eq!(
            uninstall_service.get_about().map(ToString::to_string),
            Some("Disable and uninstall the systemd user service".to_string())
        );
    }

    #[test]
    fn service_dir_is_under_home_config_systemd_user() {
        let original_home = std::env::var_os("HOME");
        unsafe { std::env::set_var("HOME", "/tmp/power-profile-watcher-home") };

        let dir = service_dir().expect("service dir should resolve");

        match original_home {
            Some(value) => unsafe { std::env::set_var("HOME", value) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        assert_eq!(
            dir,
            PathBuf::from("/tmp/power-profile-watcher-home/.config/systemd/user")
        );
    }

    #[test]
    fn rendered_service_uses_resolved_executable_path() {
        let unit = render_service_unit(std::path::Path::new(
            "/tmp/build output/power-profile-watcher",
        ));

        assert!(unit.contains("ExecStart=/tmp/build\\x20output/power-profile-watcher"));
        assert!(unit.contains("Environment=RUST_LOG=info"));
        assert!(unit.contains("PartOf=graphical-session.target"));
        assert!(unit.contains("WantedBy=graphical-session.target"));
    }

    #[test]
    fn rendered_service_does_not_pull_graphical_session_in_from_default_target() {
        let unit = render_service_unit(std::path::Path::new("/tmp/power-profile-watcher"));

        assert!(!unit.contains("Wants=graphical-session.target"));
        assert!(!unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn parses_exec_start_from_service_unit() {
        let unit = render_service_unit(std::path::Path::new(
            "/tmp/build output/power-profile-watcher",
        ));

        assert_eq!(
            parse_exec_start(&unit),
            Some("/tmp/build\\x20output/power-profile-watcher")
        );
    }

    #[test]
    fn parse_exec_start_returns_none_when_missing() {
        assert_eq!(parse_exec_start("[Service]\nType=simple\n"), None);
    }

    #[test]
    fn unescapes_systemd_exec_argument_spaces() {
        assert_eq!(
            unescape_systemd_exec_argument("/tmp/build\\x20output/power-profile-watcher"),
            "/tmp/build output/power-profile-watcher"
        );
    }

    #[test]
    fn extracts_existing_binary_path_from_rendered_service_unit() {
        let unit = render_service_unit(std::path::Path::new(
            "/tmp/build output/power-profile-watcher",
        ));
        let exec_start = parse_exec_start(&unit).expect("ExecStart should be present");

        assert_eq!(
            PathBuf::from(unescape_systemd_exec_argument(exec_start)),
            PathBuf::from("/tmp/build output/power-profile-watcher")
        );
    }

    #[test]
    fn verify_service_executable_accepts_expected_path() {
        let executable = std::path::Path::new("/tmp/power-profile-watcher");

        assert!(verify_service_executable(executable, executable).is_ok());
    }

    #[test]
    fn verify_service_executable_rejects_wrong_existing_path() {
        let result = verify_service_executable(
            std::path::Path::new("/usr/bin/power-profile-watcher"),
            std::path::Path::new("/home/jbrown/.cargo/bin/power-profile-watcher"),
        );

        assert_eq!(
            result.unwrap_err().to_string(),
            "service executable is incorrect: expected /home/jbrown/.cargo/bin/power-profile-watcher, found /usr/bin/power-profile-watcher"
        );
    }

    #[test]
    fn parses_lock_state_changed_signal_when_active_is_true() {
        assert_eq!(parse_lock_state_changed_signal(true), WatchEvent::LockStateChanged);
    }

    #[test]
    fn parses_lock_state_changed_signal_when_active_is_false() {
        assert_eq!(parse_lock_state_changed_signal(false), WatchEvent::LockStateChanged);
    }

    #[test]
    fn lock_state_from_active_true_is_locked() {
        assert_eq!(LockState::from_active(true), LockState::Locked);
    }

    #[test]
    fn lock_state_from_active_false_is_unlocked() {
        assert_eq!(LockState::from_active(false), LockState::Unlocked);
    }

    #[test]
    fn parses_active_systemctl_state() {
        assert_eq!(parse_systemctl_is_active("active"), Some(true));
    }

    #[test]
    fn parses_inactive_systemctl_states() {
        for state in ["inactive", "failed", "activating", "deactivating", "unknown"] {
            assert_eq!(parse_systemctl_is_active(state), Some(false));
        }
    }

    #[test]
    fn returns_none_for_unexpected_systemctl_state() {
        assert_eq!(parse_systemctl_is_active("reloading"), None);
    }
}
