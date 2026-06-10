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
}
