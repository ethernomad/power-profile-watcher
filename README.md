# power-profile-watcher

`power-profile-watcher` is a small Rust daemon that watches `UPower` for AC/battery changes and updates `power-profiles-daemon` automatically:

- AC power: `performance`
- Battery power: `power-saver`

It does an initial sync on startup and then only changes profiles when the power source changes. Manual profile changes still work normally until the next plug/unplug event.

## Requirements

This program requires these services to be present and running:

- `org.freedesktop.UPower`
- `net.hadess.PowerProfiles`

On this machine, those are provided by:

- `upower`
- `power-profiles-daemon`

You can verify availability with:

```bash
busctl get-property org.freedesktop.UPower /org/freedesktop/UPower org.freedesktop.UPower OnBattery
busctl get-property net.hadess.PowerProfiles /net/hadess/PowerProfiles net.hadess.PowerProfiles ActiveProfile
```

## Build

From the project directory:

```bash
cargo build --release
```

The compiled binary will be:

```text
target/release/power-profile-watcher
```

## Install Binary

Install the binary into your local user bin directory:

```bash
mkdir -p ~/.local/bin
install -m 0755 target/release/power-profile-watcher ~/.local/bin/power-profile-watcher
```

Make sure `~/.local/bin` is in your `PATH`.

## Run Manually

You can test it directly before creating a service:

```bash
~/.local/bin/power-profile-watcher
```

You should see log lines when it starts and when it changes profiles. Set `RUST_LOG=info` for normal output.

Stop it with `Ctrl+C`.

## Install As A systemd User Service

Create the user service directory if needed:

```bash
mkdir -p ~/.config/systemd/user
```

Install the included service file:

```bash
install -m 0644 power-profile-watcher.service ~/.config/systemd/user/power-profile-watcher.service
```

If you want the service to always emit logs, add `Environment=RUST_LOG=info` to the unit file.

Reload the user manager and enable the service:

```bash
systemctl --user daemon-reload
systemctl --user enable --now power-profile-watcher.service
```

## Verify

Check service status:

```bash
systemctl --user status power-profile-watcher.service
```

Watch service logs:

```bash
journalctl --user -u power-profile-watcher.service -f
```

Check the current active profile:

```bash
powerprofilesctl get
```

Then test the actual behavior:

1. Plug in AC power and confirm the profile becomes `performance`.
2. Unplug AC power and confirm the profile becomes `power-saver`.
3. Manually change the profile in GNOME and confirm it stays changed until the next AC state transition.

## Update After Code Changes

After modifying the program:

```bash
cargo build --release
install -m 0755 target/release/power-profile-watcher ~/.local/bin/power-profile-watcher
systemctl --user restart power-profile-watcher.service
```

## Uninstall

Disable the service and remove installed files:

```bash
systemctl --user disable --now power-profile-watcher.service
rm -f ~/.config/systemd/user/power-profile-watcher.service
rm -f ~/.local/bin/power-profile-watcher
systemctl --user daemon-reload
```

## Notes

- This program is not GNOME-specific. It works anywhere `UPower` and `power-profiles-daemon` are available.
- It uses direct D-Bus calls instead of spawning `powerprofilesctl`.
- It is event-driven. It does not poll.
