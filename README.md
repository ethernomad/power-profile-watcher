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

Build the binary in this repository:

```bash
cargo build --release
```

Or install it into your Cargo bin directory:

```bash
cargo install --path .
```

The service installer command uses the path of the currently running executable, so it works both when you run the binary from `target/release/` and when you run the `cargo install` copy from `~/.cargo/bin/`.

## Run Manually

You can test it directly before creating a service:

```bash
target/release/power-profile-watcher
```

You should see log lines when it starts and when it changes profiles.

Stop it with `Ctrl+C`.

If you installed it with `cargo install --path .`, run `power-profile-watcher` instead.

## Install As A systemd User Service

Use the built-in installer command:

```bash
target/release/power-profile-watcher install-service
```

If you installed it with `cargo install --path .`, run:

```bash
power-profile-watcher install-service
```

This writes `~/.config/systemd/user/power-profile-watcher.service`, reloads the user manager, and runs `systemctl --user enable --now power-profile-watcher.service`.

The generated unit sets `Environment=RUST_LOG=info`, so it emits normal log output by default.

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
target/release/power-profile-watcher install-service
```

## Uninstall

Remove the user service with the built-in command:

```bash
target/release/power-profile-watcher uninstall-service
```

If you installed it with `cargo install --path .`, run:

```bash
power-profile-watcher uninstall-service
```

This disables the service, removes `~/.config/systemd/user/power-profile-watcher.service`, and reloads the user manager.

## Notes

- This program is not GNOME-specific. It works anywhere `UPower` and `power-profiles-daemon` are available.
- It uses direct D-Bus calls instead of spawning `powerprofilesctl`.
- It is event-driven. It does not poll.
