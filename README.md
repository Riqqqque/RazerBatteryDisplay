<p align="center">
  <img src="assets/icon.png" width="128" height="128" alt="Razer Battery Display icon">
</p>

<h1 align="center">Razer Battery Display</h1>

<p align="center">
  <strong>Your mouse battery percentage in the tray. No Synapse. No bloat.</strong>
</p>

<p align="center">
  <a href="https://github.com/Riqqqque/RazerBatteryDisplay/actions/workflows/windows.yml"><img src="https://github.com/Riqqqque/RazerBatteryDisplay/actions/workflows/windows.yml/badge.svg" alt="Windows build"></a>
</p>

A tiny Windows 11 tray app that does one job: it shows the battery percentage of a Razer Viper V4 Pro.

**296 KB exe. 1.4 MB of private memory. 1 thread. Effectively zero CPU.**

## Why This Exists

Razer Synapse wants to run services, background processes, an account login, lighting, profiles, and update prompts, all so you can see one number. If all you want is to know when to charge your mouse, that is a lot of software sitting on a gaming PC doing nothing for you.

So this app keeps the one useful part and throws away the rest. It reads the battery straight from the mouse, shows it in the tray, and gets out of the way. You don't need Synapse installed at all.

## What You Get

- The battery percentage right in the tray, as a live number you can read at a glance.
- Green, amber, and red colours, so a low battery stands out.
- The same percentage on hover.
- An automatic refresh every 5 minutes.
- A reading as soon as you touch the mouse. If it was asleep at boot, the first movement picks it up.
- Left-click to refresh on demand.
- Starts with Windows, and you can turn that off from the tray menu.
- A normal per-user install with a normal Apps & Features uninstall entry.

## What You Don't Get

| Razer Synapse | Razer Battery Display |
| --- | --- |
| Background services | None |
| Account login | None |
| Telemetry and online features | None. It never touches the network |
| Lighting and profile engine | None |
| Update prompts | None |
| Large install | One 296 KB exe |

## How Light Is It?

Measured on Windows 11 with the release build installed:

| Metric | Result |
| --- | ---: |
| Exe size | 296 KB |
| Private memory | about 1.4 MB |
| Working set after the first reading | about 0.4 MB |
| CPU time in the first minute | about 0.02 s |
| Settled threads | 1 |
| Handles | about 133 |
| Services installed | 0 |
| Network connections | 0 |

The working set grows back a little with use, and most of it is shared Windows DLL memory.

That number is the result of deliberate choices, not just using Rust:

- **Native Win32.** No webview, no runtime, no framework, and one dependency: Microsoft's Windows API bindings.
- **Asleep until needed.** The app waits on the Windows message loop and uses no CPU between refreshes. There is no busy loop.
- **Direct driver requests.** Battery reads go straight to the HID driver, so `hid.dll` and `setupapi.dll` are never loaded.
- **Built-in pixel font.** Tray digits are drawn from a tiny font inside the exe, so no font or text-shaping libraries load. The icon is only redrawn when the percentage changes.
- **Efficiency mode.** The app asks Windows to schedule it as a low-power process and lets its timer share wake-ups with other work.
- **Startup memory handed back.** After the first reading, memory used only during startup is returned to Windows.

## Safe While Gaming

The app never hooks your mouse and never reads pointer data. It only talks to the Viper V4 Pro's non-pointer HID feature interfaces (`mi_03` and `mi_04`), then caches the working path. It never sends feature reports to the mouse or keyboard input collections.

If Windows starts the app before the mouse is ready, it retries after 10, 20, 30, and 60 seconds. While it has no battery value, it also registers for a single Windows Raw Input notification. The first movement removes that registration straight away, so the app never receives a stream of mouse events, even at 8000 Hz. It reads the battery 750 ms later, which gives a sleeping mouse time to reconnect, and the contents of the event are ignored.

If that read fails, the listener stays off for 15 seconds, so using a different mouse can't cause constant reads. The first good read removes the listener and returns to the normal 5-minute refresh.

## Supported Mouse

- Razer Viper V4 Pro wired: `1532:00E5`
- Razer Viper V4 Pro wireless: `1532:00E6`

The battery query follows the same command path as current OpenRazer Viper V4 Pro support work. Windows does not expose this mouse's battery through the normal device battery property, so the app reads the Razer HID battery report directly.

## Install

Download `RazerBatteryDisplaySetup.exe` from [Releases](https://github.com/Riqqqque/RazerBatteryDisplay/releases/latest) and run it. It installs for the current user, starts the tray app, and enables startup with Windows.

To build the setup exe yourself:

```powershell
cargo build --release
New-Item -ItemType Directory -Force dist
Copy-Item target\release\razer-battery-display.exe dist\RazerBatteryDisplaySetup.exe -Force
dist\RazerBatteryDisplaySetup.exe
```

For a quiet reinstall from a fresh build:

```powershell
target\release\razer-battery-display.exe --install-quiet
```

## Tray Menu

Left-click refreshes the percentage. Right-click for:

- Refresh
- Start with Windows
- Uninstall
- Quit

## Build Checks

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --all-features --locked
cargo build --release --locked
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

The app only talks to the local HID device and the current-user Windows registry entries needed for install, startup, and uninstall. Startup uses the normal per-user `Run` entry, not a service or scheduled task. It sends no network requests, collects no telemetry, and stores no personal data.

## License

MIT. Not affiliated with or endorsed by Razer.
