#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod battery;
mod install;
mod tray;
mod update;
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
        win::message_box_error(APP_NAME, &err.to_string());

        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.len() > 1 && args.first().is_none_or(|arg| arg != "--finish-uninstall") {
        return Err("too many command-line arguments".into());
    }

    match args.first().map(String::as_str) {
        Some("--probe") => {
            println!("{}", battery::probe_report());
            return Ok(());
        }
        Some("--probe-verbose") => {
            println!("{}", battery::probe_report_verbose());
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
        Some("--repair-startup") => {
            install::repair_startup()?;
            return Ok(());
        }
        Some("--uninstall") => {
            install::uninstall()?;
            return Ok(());
        }
        Some("--uninstall-quiet") => {
            install::uninstall_quiet()?;
            return Ok(());
        }
        Some("--finish-uninstall") => {
            install::finish_uninstall(&args[1..])?;
            return Ok(());
        }
        Some("--update") => {
            update::run();
            return Ok(());
        }
        Some("--run") => {
            tray::run()?;
            return Ok(());
        }
        Some(option) => return Err(format!("unknown option: {option}").into()),
        None => {}
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
