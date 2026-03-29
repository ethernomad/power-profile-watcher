use std::error::Error;
use std::fmt::Write as _;
use std::path::PathBuf;

use clap::{ColorChoice, Parser, Subcommand};
use futures_util::StreamExt;
use tokio::process::Command;
use tokio::signal;
use tracing::{error, info};
use zbus::Connection;
use zbus::fdo::PropertiesProxy;
use zbus::names::InterfaceName;
use zbus::zvariant::Value;

const POWER_PROFILES_DESTINATION: &str = "net.hadess.PowerProfiles";
const POWER_PROFILES_PATH: &str = "/net/hadess/PowerProfiles";
const POWER_PROFILES_INTERFACE: &str = "net.hadess.PowerProfiles";
const UPOWER_DESTINATION: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const UPOWER_INTERFACE: &str = "org.freedesktop.UPower";
const PROFILE_PERFORMANCE: &str = "performance";
const PROFILE_POWERSAVE: &str = "power-saver";
const SERVICE_NAME: &str = "power-profile-watcher.service";

fn clap_styles() -> clap::builder::Styles {
    use clap::builder::styling::{AnsiColor, Effects, Styles};

    Styles::styled()
        .header(AnsiColor::Green.on_default() | Effects::BOLD)
        .usage(AnsiColor::Green.on_default() | Effects::BOLD)
        .literal(AnsiColor::Cyan.on_default() | Effects::BOLD)
        .placeholder(AnsiColor::Cyan.on_default())
}

#[derive(Debug, Parser)]
#[command(
    author,
    version,
    about,
    long_about = "Watches UPower power-source changes and updates power-profiles-daemon automatically.\n\nWatch service logs with:\n  journalctl --user -u power-profile-watcher.service -f",
    help_template = "{about-with-newline}\n{usage-heading} {usage}\n\n{all-args}",
    disable_help_subcommand = true,
    color = ColorChoice::Auto,
    styles = clap_styles()
)]
struct Cli {
    /// Increase log verbosity (-v, -vv)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    verbose: u8,

    /// Reduce log verbosity (-q, -qq)
    #[arg(short = 'q', long = "quiet", action = clap::ArgAction::Count, global = true)]
    quiet: u8,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Install and enable the systemd user service
    InstallService,

    /// Verify the installed systemd user service
    VerifyService,

    /// Disable and uninstall the systemd user service
    UninstallService,
}

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
async fn main() {
    let cli = Cli::parse();

    let filter = resolve_filter(&cli);

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();

    let result = match cli.command {
        Some(Commands::InstallService) => install_service().await,
        Some(Commands::VerifyService) => verify_service().await,
        Some(Commands::UninstallService) => uninstall_service().await,
        None => run().await,
    };

    if let Err(err) = result {
        error!(%err, "daemon failed");
        std::process::exit(1);
    }
}

async fn install_service() -> Result<(), Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    let service_dir = service_dir()?;
    let service_path = service_dir.join(SERVICE_NAME);

    if is_systemctl_user_active(SERVICE_NAME).await? {
        run_systemctl_user(["stop", SERVICE_NAME]).await?;
        info!(service = SERVICE_NAME, "stopped active systemd user service");
    }

    tokio::fs::create_dir_all(&service_dir).await?;
    tokio::fs::write(&service_path, render_service_unit(&executable)).await?;
    info!(service_path = %service_path.display(), executable = %executable.display(), "wrote systemd user service unit");

    run_systemctl_user(["daemon-reload"]).await?;
    info!("reloaded systemd user manager");

    run_systemctl_user(["enable", "--now", SERVICE_NAME]).await?;
    info!(service = SERVICE_NAME, "enabled and started systemd user service");

    Ok(())
}

async fn uninstall_service() -> Result<(), Box<dyn Error>> {
    let service_path = service_dir()?.join(SERVICE_NAME);

    let disable_result = run_systemctl_user(["disable", "--now", SERVICE_NAME]).await;
    if let Err(err) = disable_result {
        if service_path.exists() {
            return Err(err);
        }
    }

    match tokio::fs::remove_file(&service_path).await {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }

    run_systemctl_user(["daemon-reload"]).await?;

    info!(service_path = %service_path.display(), "uninstalled systemd user service");

    Ok(())
}

async fn verify_service() -> Result<(), Box<dyn Error>> {
    let service_path = service_dir()?.join(SERVICE_NAME);

    if !service_path.exists() {
        return Err(format!("service file not found: {}", service_path.display()).into());
    }

    let unit = tokio::fs::read_to_string(&service_path).await?;
    let exec_start = parse_exec_start(&unit).ok_or_else(|| {
        format!(
            "service file {} is missing ExecStart",
            service_path.display()
        )
    })?;
    let executable = PathBuf::from(unescape_systemd_exec_argument(exec_start));
    let expected_executable = std::env::current_exe()?;

    verify_service_executable(&executable, &expected_executable)?;

    if !executable.exists() {
        return Err(format!("service binary not found: {}", executable.display()).into());
    }

    run_systemctl_user_expect_output(["is-enabled", SERVICE_NAME], "enabled", "enabled").await?;
    run_systemctl_user_expect_output(["is-active", SERVICE_NAME], "active", "running").await?;

    info!(
        service_path = %service_path.display(),
        executable = %executable.display(),
        "verified systemd user service"
    );

    Ok(())
}

async fn run() -> Result<(), Box<dyn Error>> {
    let connection = Connection::system().await?;

    verify_upower_available(&connection).await?;
    verify_power_profiles_available(&connection).await?;

    apply_profile_for_current_power_source(&connection).await?;

    let properties_proxy = PropertiesProxy::builder(&connection)
        .destination(UPOWER_DESTINATION)?
        .path(UPOWER_PATH)?
        .build()
        .await?;
    let mut changes = properties_proxy.receive_properties_changed().await?;

    info!("watching UPower for power-source changes");

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

fn service_dir() -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/systemd/user"))
}

fn render_service_unit(executable: &std::path::Path) -> String {
    let escaped_executable = escape_systemd_exec_argument(executable);
    let mut unit = String::new();
    let _ = write!(
        unit,
        "[Unit]\nDescription=Watch power source and switch power profiles\nAfter=graphical-session.target\nWants=graphical-session.target\n\n[Service]\nType=simple\nExecStart={}\nEnvironment=RUST_LOG=info\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=default.target\n",
        escaped_executable
    );
    unit
}

fn parse_exec_start(unit: &str) -> Option<&str> {
    unit.lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

fn escape_systemd_exec_argument(path: &std::path::Path) -> String {
    path.display().to_string().replace(' ', "\\x20")
}

fn unescape_systemd_exec_argument(value: &str) -> String {
    value.replace("\\x20", " ")
}

fn verify_service_executable(
    executable: &std::path::Path,
    expected_executable: &std::path::Path,
) -> Result<(), Box<dyn Error>> {
    if executable == expected_executable {
        return Ok(());
    }

    Err(format!(
        "service executable is incorrect: expected {}, found {}",
        expected_executable.display(),
        executable.display()
    )
    .into())
}

async fn run_systemctl_user<const N: usize>(args: [&str; N]) -> Result<(), Box<dyn Error>> {
    let output = Command::new("systemctl")
        .args(["--user"])
        .args(args)
        .output()
        .await?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let details = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("systemctl exited with status {}", output.status)
    };

    Err(format!("systemctl --user {} failed: {}", args.join(" "), details).into())
}

async fn is_systemctl_user_active(unit: &str) -> Result<bool, Box<dyn Error>> {
    let output = Command::new("systemctl")
        .args(["--user", "is-active", unit])
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if let Some(is_active) = parse_systemctl_is_active(&stdout) {
        return Ok(is_active);
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let details = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("systemctl exited with status {}", output.status)
    };

    Err(format!("systemctl --user is-active {unit} failed: {details}").into())
}

fn parse_systemctl_is_active(stdout: &str) -> Option<bool> {
    match stdout {
        "active" => Some(true),
        "inactive" | "failed" | "activating" | "deactivating" | "unknown" => Some(false),
        _ => None,
    }
}

async fn run_systemctl_user_expect_output<const N: usize>(
    args: [&str; N],
    expected: &str,
    state_description: &str,
) -> Result<(), Box<dyn Error>> {
    let output = Command::new("systemctl")
        .args(["--user"])
        .args(args)
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if output.status.success() && stdout == expected {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let details = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        format!("expected {expected}, got {stdout}")
    } else {
        format!("systemctl exited with status {}", output.status)
    };

    Err(format!(
        "service is not {state_description}: systemctl --user {} failed: {}",
        args.join(" "),
        details
    )
    .into())
}

async fn verify_upower_available(connection: &Connection) -> Result<(), Box<dyn Error>> {
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

async fn verify_power_profiles_available(connection: &Connection) -> Result<(), Box<dyn Error>> {
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

async fn apply_profile_for_current_power_source(
    connection: &Connection,
) -> Result<(), Box<dyn Error>> {
    let power_source = current_power_source(connection).await?;
    let current_profile = active_profile(connection).await?;
    match decide_profile_action(power_source, &current_profile) {
        ProfileDecision::Unchanged { desired_profile } => {
            info!(
                source = power_source.label(),
                profile = desired_profile,
                "power source unchanged for profile selection"
            );
        }
        ProfileDecision::Change { desired_profile } => {
            set_active_profile(connection, desired_profile).await?;
            info!(
                source = power_source.label(),
                profile = desired_profile,
                "set active profile"
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
        .get(InterfaceName::try_from(UPOWER_INTERFACE)?, "OnBattery")
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
            value,
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
        if on_battery { Self::Battery } else { Self::Ac }
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
    use clap::CommandFactory;

    #[test]
    fn clap_styles_build_without_panicking() {
        let _ = clap_styles();
    }

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
        assert!(matches!(cli.command, Some(Commands::InstallService)));
    }

    #[test]
    fn uninstall_service_subcommand_parses() {
        let cli = Cli::parse_from(["power-profile-watcher", "uninstall-service"]);
        assert!(matches!(cli.command, Some(Commands::UninstallService)));
    }

    #[test]
    fn verify_service_subcommand_parses() {
        let cli = Cli::parse_from(["power-profile-watcher", "verify-service"]);
        assert!(matches!(cli.command, Some(Commands::VerifyService)));
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
        assert!(unit.contains("WantedBy=default.target"));
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
