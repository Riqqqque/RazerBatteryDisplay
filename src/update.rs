use std::{
    env,
    error::Error,
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};

use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::{APP_NAME, APP_VERSION, win};

const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");
const SETUP_ASSET: &str = "RazerBatteryDisplaySetup.exe";

// Runs in its own short-lived process, so the tray app never loads any network code.
pub fn run() {
    let _ = fs::remove_file(downloaded_setup());

    if let Err(err) = check_and_install() {
        win::message_box_error(APP_NAME, &format!("Could not update.\n\n{err}"));
    }
}

fn check_and_install() -> Result<(), Box<dyn Error>> {
    let latest = latest_version()?;
    if !is_newer(&latest, APP_VERSION) {
        win::message_box(
            APP_NAME,
            &format!("You're up to date. Version {APP_VERSION} is the latest."),
        );
        return Ok(());
    }

    if !win::confirm(
        APP_NAME,
        &format!("Version {latest} is available. You have {APP_VERSION}.\n\nUpdate now?"),
    ) {
        return Ok(());
    }

    let setup = downloaded_setup();
    download_verified_setup(&latest, &setup)?;
    Command::new(&setup).arg("--install").spawn()?;
    Ok(())
}

pub fn downloaded_setup() -> PathBuf {
    env::temp_dir().join("RazerBatteryDisplayUpdate.exe")
}

fn latest_version() -> Result<String, Box<dyn Error>> {
    let output = system_tool("curl.exe")
        .args([
            "-sS",
            "--max-time",
            "20",
            "-o",
            "NUL",
            "-w",
            "%{redirect_url}",
        ])
        .arg(format!("{REPOSITORY}/releases/latest"))
        .output()?;
    let url = String::from_utf8_lossy(&output.stdout);
    version_from_release_url(&url)
        .map(str::to_string)
        .ok_or_else(|| "could not reach GitHub to find the latest version".into())
}

fn download_verified_setup(version: &str, target: &Path) -> Result<(), Box<dyn Error>> {
    let url = format!("{REPOSITORY}/releases/download/v{version}/{SETUP_ASSET}");

    let checksum = system_tool("curl.exe")
        .args(["-fsSL", "--max-time", "30"])
        .arg(format!("{url}.sha256"))
        .output()?;
    if !checksum.status.success() {
        return Err("could not download the update checksum".into());
    }

    let download = system_tool("curl.exe")
        .args(["-fsSL", "--max-time", "120", "-o"])
        .arg(target)
        .arg(&url)
        .status()?;
    if !download.success() {
        return Err("could not download the update".into());
    }

    let hashed = system_tool("certutil.exe")
        .arg("-hashfile")
        .arg(target)
        .arg("SHA256")
        .output()?;
    let expected = String::from_utf8_lossy(&checksum.stdout);
    let actual = String::from_utf8_lossy(&hashed.stdout);
    if !checksum_matches(&expected, &actual) {
        let _ = fs::remove_file(target);
        return Err("the download failed its checksum, so it was not installed".into());
    }

    Ok(())
}

fn system_tool(name: &str) -> Command {
    let root = env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let mut command = Command::new(Path::new(&root).join("System32").join(name));
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

fn version_from_release_url(url: &str) -> Option<&str> {
    let version = url.trim().rsplit_once("/releases/tag/v")?.1;
    parse_version(version).map(|_| version)
}

fn parse_version(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version.split('.').map(|part| part.parse().ok());
    let parsed = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(parsed)
}

fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(candidate), Some(current)) => candidate > current,
        _ => false,
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn checksum_matches(checksum_file: &str, certutil_output: &str) -> bool {
    let Some(expected) = checksum_file.split_whitespace().next() else {
        return false;
    };

    is_sha256(expected)
        && certutil_output
            .lines()
            .map(|line| line.replace(' ', ""))
            .find(|line| is_sha256(line))
            .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "3e386b3533e621d3bdb13badd0ac0e19bdefdc0ebcfde016bf729eed8188154e";

    #[test]
    fn version_is_read_from_the_release_redirect() {
        assert_eq!(
            version_from_release_url(
                "https://github.com/Riqqqque/RazerBatteryDisplay/releases/tag/v0.1.6\n"
            ),
            Some("0.1.6")
        );
        assert_eq!(
            version_from_release_url("https://github.com/Riqqqque/RazerBatteryDisplay/releases"),
            None
        );
        assert_eq!(version_from_release_url(""), None);
    }

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.6", "0.2.0"));
        assert!(!is_newer("0.2", "0.1.0"));
        assert!(!is_newer("0.2.0.1", "0.1.0"));
    }

    #[test]
    fn checksum_matches_certutil_output() {
        let checksum_file = format!("{HASH}  RazerBatteryDisplaySetup.exe\n");
        let certutil = format!(
            "SHA256 hash of setup.exe:\r\n{}\r\nCertUtil: -hashfile command completed successfully.\r\n",
            HASH.to_ascii_uppercase()
        );
        assert!(checksum_matches(&checksum_file, &certutil));
    }

    #[test]
    fn checksum_rejects_a_mismatch_or_failed_hash() {
        let checksum_file = format!("{HASH}  RazerBatteryDisplaySetup.exe\n");
        let other = HASH.replace("3e38", "0000");
        assert!(!checksum_matches(&checksum_file, &other));
        assert!(!checksum_matches(&checksum_file, "CertUtil: error"));
        assert!(!checksum_matches("", HASH));
        assert!(!checksum_matches("Not Found", "Not Found"));
    }

    #[test]
    #[ignore = "downloads the latest release"]
    fn downloads_and_verifies_the_latest_release() {
        let target = env::temp_dir().join("RazerBatteryDisplayUpdateTest.bin");
        let latest = latest_version().unwrap();
        download_verified_setup(&latest, &target).unwrap();
        assert!(fs::metadata(&target).unwrap().len() > 100_000);
        fs::remove_file(target).unwrap();
    }
}
