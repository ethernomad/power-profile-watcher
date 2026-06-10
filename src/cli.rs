use clap::{ColorChoice, Parser, Subcommand};

pub fn clap_styles() -> clap::builder::Styles {
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
pub struct Cli {
    /// Increase log verbosity (-v, -vv)
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Reduce log verbosity (-q, -qq)
    #[arg(short = 'q', long = "quiet", action = clap::ArgAction::Count, global = true)]
    pub quiet: u8,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Install and enable the systemd user service
    #[command(name = "install-service")]
    Install,

    /// Verify the installed systemd user service
    #[command(name = "verify-service")]
    Verify,

    /// Disable and uninstall the systemd user service
    #[command(name = "uninstall-service")]
    Uninstall,
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
}
