use std::error::Error;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use tokio::process::Command;
use tracing::info;

pub const SERVICE_NAME: &str = "power-profile-watcher.service";

pub async fn install_service() -> Result<(), Box<dyn Error>> {
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

pub async fn uninstall_service() -> Result<(), Box<dyn Error>> {
    let service_path = service_dir()?.join(SERVICE_NAME);

    let disable_result = run_systemctl_user(["disable", "--now", SERVICE_NAME]).await;
    if let Err(err) = disable_result
        && service_path.exists()
    {
        return Err(err);
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

pub async fn verify_service() -> Result<(), Box<dyn Error>> {
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

pub fn service_dir() -> Result<PathBuf, Box<dyn Error>> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config/systemd/user"))
}

pub fn render_service_unit(executable: &Path) -> String {
    let escaped_executable = escape_systemd_exec_argument(executable);
    let mut unit = String::new();
    let _ = write!(
        unit,
        "[Unit]\nDescription=Watch power source and switch power profiles\nAfter=graphical-session.target\nPartOf=graphical-session.target\n\n[Service]\nType=simple\nExecStart={}\nEnvironment=RUST_LOG=info\nRestart=on-failure\nRestartSec=2\n\n[Install]\nWantedBy=graphical-session.target\n",
        escaped_executable
    );
    unit
}

pub fn parse_exec_start(unit: &str) -> Option<&str> {
    unit.lines()
        .find_map(|line| line.strip_prefix("ExecStart="))
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

pub fn escape_systemd_exec_argument(path: &Path) -> String {
    path.display().to_string().replace(' ', "\\x20")
}

pub fn unescape_systemd_exec_argument(value: &str) -> String {
    value.replace("\\x20", " ")
}

pub fn verify_service_executable(
    executable: &Path,
    expected_executable: &Path,
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

pub async fn run_systemctl_user<const N: usize>(args: [&str; N]) -> Result<(), Box<dyn Error>> {
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

pub async fn is_systemctl_user_active(unit: &str) -> Result<bool, Box<dyn Error>> {
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

pub fn parse_systemctl_is_active(stdout: &str) -> Option<bool> {
    match stdout {
        "active" => Some(true),
        "inactive" | "failed" | "activating" | "deactivating" | "unknown" => Some(false),
        _ => None,
    }
}

pub async fn run_systemctl_user_expect_output<const N: usize>(
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

#[cfg(test)]
mod tests {
    use super::*;

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
