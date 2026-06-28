# Razer Battery Display

[![Windows build](https://github.com/Riqqqque/RazerBatteryDisplay/actions/workflows/windows.yml/badge.svg)](https://github.com/Riqqqque/RazerBatteryDisplay/actions/workflows/windows.yml)

A tiny Windows 11 tray app that shows the battery percentage for a Razer Viper V4 Pro.

## Why

Razer Synapse does a lot, but that is exactly the problem if all you want is one number: mouse battery percentage. This app exists because the useful part was the battery readout, not a full background suite with profiles, lighting, services, account prompts, and extra resource use.

Razer Battery Display keeps it simple: start with Windows, sit in the tray, show the percentage, and stay out of the way.

## What It Does

- Shows the Viper V4 Pro battery percentage in the Windows tray.
- Shows the same percentage on hover.
- Refreshes automatically every 5 minutes.
- Lets you refresh manually from the tray icon.
- Installs per-user with a normal `.exe`.
- Adds a normal Apps & Features uninstall entry.
- Starts with Windows unless you turn that off in the tray menu.

## Why It Is Lightweight

- Built in Rust.
- Native Win32 tray app.
- Direct Windows HID calls for the mouse battery report.
- No Razer Synapse dependency.
- No webview.
- No background service.
- No telemetry.
- No account login.
- No RGB/profile system.
- No always-busy polling loop.

The app only queries the Viper V4 Pro non-pointer HID feature interfaces (`mi_03`/`mi_04`) for battery reports, then caches the working path. It does not hook mouse input, read pointer movement, or send feature reports to the mouse/keyboard input collections. After startup it only wakes up for the Windows tray message loop and a battery poll every 5 minutes. The tray icon is only redrawn when the visible percentage changes.

## Example Footprint

Measured on Windows 11 with the optimized build installed. Windows can trim or grow the shared working set over time, so private memory is the best quick read on what the app itself owns.

| Metric | Result |
| --- | ---: |
| Installed exe size | about 257 KB |
| Private memory | about 1.8 MB |
| Working set | about 7-11 MB |
| Idle CPU over 75 seconds | 0% |
| Settled threads | 1 |
| Handles | about 150-180 |

Most of the working set is normal shared Windows DLL/runtime memory.

## Supported Mouse

Currently targeted at:

- Razer Viper V4 Pro wired: `1532:00E5`
- Razer Viper V4 Pro wireless: `1532:00E6`

The battery query follows the same command path used by current OpenRazer Viper V4 Pro support work. On this machine, Windows did not expose the mouse battery through the normal device battery property, so the app reads the Razer HID battery report directly.

## Install

Build the setup exe:

```powershell
cargo build --release
New-Item -ItemType Directory -Force dist
Copy-Item target\release\razer-battery-display.exe dist\RazerBatteryDisplaySetup.exe -Force
```

Run:

```powershell
dist\RazerBatteryDisplaySetup.exe
```

It installs to the current user, starts the tray app, and enables startup with Windows.

For a quiet reinstall from a fresh build:

```powershell
target\release\razer-battery-display.exe --install-quiet
```

## Tray Menu

Right-click the tray icon for:

- Refresh
- Start with Windows
- Uninstall
- Quit

Left-click refreshes the battery percentage.

## Build Checks

```powershell
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build --release
```

Hardware probe:

```powershell
cargo run -- --probe
```

Verbose probe with full HID instance paths:

```powershell
cargo run -- --probe-verbose
```

## Privacy

The app only talks to the local HID device and Windows registry entries needed for install/startup/uninstall. It does not send network requests, collect telemetry, or store personal data.

## License

MIT. Not affiliated with or endorsed by Razer.
