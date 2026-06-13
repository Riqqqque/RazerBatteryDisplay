#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod battery;
mod install;
mod tray;
mod win;

use std::{env, error::Error, path::Path};

pub const APP_ID: &str = "RazerBatteryDisplay";
pub const APP_NAME: &str = "Razer Battery Display";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const EXE_NAME: &str = "RazerBatteryDisplay.exe";
pub const WINDOW_CLASS: &str = "RazerBatteryDisplayTrayWindow";

fn main() {
    if let Err(err) = run() {
        #[cfg(debug_assertions)]
        eprintln!("{err}");

        #[cfg(windows)]
        win::message_box(APP_NAME, &err.to_string());
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("--probe") => {
            println!("{}", battery::probe_report());
            return Ok(());
        }
        Some("--install") => {
            install::install()?;
            return Ok(());
        }
        Some("--install-quiet") => {
            install::install_quiet()?;
            return Ok(());
        }
        Some("--uninstall") => {
            install::uninstall()?;
            return Ok(());
        }
        Some("--finish-uninstall") => {
            install::finish_uninstall(&args[1..])?;
            return Ok(());
        }
        Some("--run") => {
            tray::run()?;
            return Ok(());
        }
        _ => {}
    }

    if running_as_setup()? {
        install::install()?;
    } else {
        tray::run()?;
    }

    Ok(())
}

fn running_as_setup() -> Result<bool, Box<dyn Error>> {
    let exe = env::current_exe()?;
    let name = exe
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    Ok(name.contains("setup") || name.contains("install") || is_outside_install_dir(&exe))
}

fn is_outside_install_dir(exe: &Path) -> bool {
    let installed = install::installed_exe();
    match (exe.canonicalize(), installed.canonicalize()) {
        (Ok(current), Ok(installed)) => current != installed,
        _ => true,
    }
}
