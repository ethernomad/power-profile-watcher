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
