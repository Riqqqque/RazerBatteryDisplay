use std::{
    env,
    error::Error,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_SUCCESS, WAIT_OBJECT_0},
    Storage::FileSystem::SYNCHRONIZE,
    System::{
        Registry::{
            HKEY, HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE, REG_DWORD, REG_SZ,
            RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegOpenKeyExW,
            RegQueryValueExW, RegSetValueExW,
        },
        Threading::{GetCurrentProcessId, OpenProcess, WaitForSingleObject},
    },
    UI::WindowsAndMessaging::{FindWindowW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE},
};

use crate::{APP_ID, APP_NAME, APP_VERSION, EXE_NAME, WINDOW_CLASS, win};

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const UNINSTALL_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\RazerBatteryDisplay";

pub fn install() -> Result<(), Box<dyn Error>> {
    install_inner(true)
}

pub fn install_quiet() -> Result<(), Box<dyn Error>> {
    install_inner(false)
}

fn install_inner(show_message: bool) -> Result<(), Box<dyn Error>> {
    if let Some(pid) = request_running_app_exit()
        && !wait_for_process_to_exit(pid, Duration::from_secs(8))
    {
        return Err("the running tray process did not exit before reinstall".into());
    }

    let source = env::current_exe()?;
    let target = installed_exe();
    let target_dir = target
        .parent()
        .ok_or("could not resolve install directory")?
        .to_path_buf();

    fs::create_dir_all(&target_dir)?;
    fs::copy(&source, &target)?;

    set_startup_enabled(true)?;
    write_uninstall_entry(&target, &target_dir)?;
    create_start_menu_shortcut(&target);

    Command::new(&target).arg("--run").spawn()?;
    if show_message {
        win::message_box(
            APP_NAME,
            "Installed. The battery icon is running in the tray and will start with Windows.",
        );
    }

    Ok(())
}

pub fn uninstall() -> Result<(), Box<dyn Error>> {
    request_running_app_exit();
    set_startup_enabled(false)?;
    delete_reg_tree(HKEY_CURRENT_USER, UNINSTALL_KEY);
    remove_start_menu_shortcut();

    let dir = install_dir();
    let current = env::current_exe()?;

    if current.starts_with(&dir) {
        let helper = env::temp_dir().join("RazerBatteryDisplayUninstall.exe");
        fs::copy(&current, &helper)?;
        Command::new(&helper)
            .arg("--finish-uninstall")
            .arg(unsafe { GetCurrentProcessId() }.to_string())
            .arg(dir)
            .spawn()?;
    } else if dir.exists() {
        fs::remove_dir_all(dir)?;
        win::message_box(APP_NAME, "Uninstalled.");
    }

    Ok(())
}

pub fn finish_uninstall(args: &[String]) -> Result<(), Box<dyn Error>> {
    if args.len() < 2 {
        return Err("missing uninstall helper arguments".into());
    }

    let pid: u32 = args[0].parse()?;
    let dir = validate_uninstall_dir(Path::new(&args[1]))?;
    wait_for_process_to_exit(pid, Duration::from_secs(8));

    let deadline = Instant::now() + Duration::from_secs(8);
    let mut last_error = None;
    while Instant::now() < deadline {
        match fs::remove_dir_all(&dir) {
            Ok(()) => {
                win::message_box(APP_NAME, "Uninstalled.");
                return Ok(());
            }
            Err(_) if !dir.exists() => {
                win::message_box(APP_NAME, "Uninstalled.");
                return Ok(());
            }
            Err(err) => {
                last_error = Some(err);
                std::thread::sleep(Duration::from_millis(350));
            }
        }
    }

    Err(format!(
        "uninstall cleaned up startup entries, but could not remove {}: {}",
        dir.display(),
        last_error
            .map(|err| err.to_string())
            .unwrap_or_else(|| "unknown error".to_string())
    )
    .into())
}

pub fn installed_exe() -> PathBuf {
    install_dir().join(EXE_NAME)
}

pub fn startup_enabled() -> bool {
    let expected = quote_arg(&installed_exe()) + " --run";
    read_reg_string(HKEY_CURRENT_USER, RUN_KEY, APP_ID)
        .map(|value| value == expected)
        .unwrap_or(false)
}

pub fn set_startup_enabled(enabled: bool) -> Result<(), Box<dyn Error>> {
    if enabled {
        let value = quote_arg(&installed_exe()) + " --run";
        set_reg_string(HKEY_CURRENT_USER, RUN_KEY, APP_ID, &value)?;
    } else {
        delete_reg_value(HKEY_CURRENT_USER, RUN_KEY, APP_ID);
    }

    Ok(())
}

pub fn install_dir() -> PathBuf {
    env_path("LOCALAPPDATA")
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
        .join("Programs")
        .join(APP_ID)
}

fn write_uninstall_entry(exe: &Path, dir: &Path) -> Result<(), Box<dyn Error>> {
    set_reg_string(HKEY_CURRENT_USER, UNINSTALL_KEY, "DisplayName", APP_NAME)?;
    set_reg_string(
        HKEY_CURRENT_USER,
        UNINSTALL_KEY,
        "DisplayVersion",
        APP_VERSION,
    )?;
    set_reg_string(HKEY_CURRENT_USER, UNINSTALL_KEY, "Publisher", "Rique")?;
    set_reg_string(
        HKEY_CURRENT_USER,
        UNINSTALL_KEY,
        "InstallLocation",
        &dir.display().to_string(),
    )?;
    set_reg_string(
        HKEY_CURRENT_USER,
        UNINSTALL_KEY,
        "DisplayIcon",
        &format!("{},0", exe.display()),
    )?;
    set_reg_string(
        HKEY_CURRENT_USER,
        UNINSTALL_KEY,
        "UninstallString",
        &(quote_arg(exe) + " --uninstall"),
    )?;
    set_reg_string(
        HKEY_CURRENT_USER,
        UNINSTALL_KEY,
        "QuietUninstallString",
        &(quote_arg(exe) + " --uninstall"),
    )?;
    set_reg_dword(HKEY_CURRENT_USER, UNINSTALL_KEY, "NoModify", 1)?;
    set_reg_dword(HKEY_CURRENT_USER, UNINSTALL_KEY, "NoRepair", 1)?;
    Ok(())
}

fn create_start_menu_shortcut(exe: &Path) {
    let Some(programs) = start_menu_programs_dir() else {
        return;
    };

    let _ = fs::create_dir_all(&programs);
    let lnk = programs.join(format!("{APP_NAME}.lnk"));

    if create_lnk_with_powershell(exe, &lnk).is_ok() {
        return;
    }

    let url = programs.join(format!("{APP_NAME}.url"));
    let file_url = format!("file:///{}", exe.display().to_string().replace('\\', "/"));
    let body = format!(
        "[InternetShortcut]\nURL={file_url}\nIconFile={}\nIconIndex=0\n",
        exe.display()
    );
    let _ = fs::write(url, body);
}

fn create_lnk_with_powershell(exe: &Path, lnk: &Path) -> Result<(), Box<dyn Error>> {
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut('{}');$s.TargetPath='{}';$s.Arguments='--run';$s.WorkingDirectory='{}';$s.IconLocation='{},0';$s.Save()",
        ps_quote(&lnk.display().to_string()),
        ps_quote(&exe.display().to_string()),
        ps_quote(
            &exe.parent()
                .map(|path| path.display().to_string())
                .unwrap_or_default()
        ),
        ps_quote(&exe.display().to_string()),
    );

    let status = Command::new("powershell")
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-Command")
        .arg(script)
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err("PowerShell shortcut creation failed".into())
    }
}

fn remove_start_menu_shortcut() {
    let Some(programs) = start_menu_programs_dir() else {
        return;
    };

    let _ = fs::remove_file(programs.join(format!("{APP_NAME}.lnk")));
    let _ = fs::remove_file(programs.join(format!("{APP_NAME}.url")));
}

fn start_menu_programs_dir() -> Option<PathBuf> {
    env_path("APPDATA").map(|path| {
        path.join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs")
    })
}

fn request_running_app_exit() -> Option<u32> {
    let class = win::wide_null(WINDOW_CLASS);
    let hwnd = unsafe { FindWindowW(class.as_ptr(), std::ptr::null()) };
    if hwnd.is_null() {
        return None;
    }

    let mut pid = 0_u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, &mut pid);
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
    }

    (pid != 0).then_some(pid)
}

fn wait_for_process_to_exit(pid: u32, duration: Duration) -> bool {
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return true;
    }

    let result =
        unsafe { WaitForSingleObject(handle, duration.as_millis().min(u32::MAX as u128) as u32) };
    unsafe {
        CloseHandle(handle);
    }

    result == WAIT_OBJECT_0
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).map(PathBuf::from)
}

fn quote_arg(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

fn ps_quote(value: &str) -> String {
    value.replace('\'', "''")
}

fn open_reg_key(root: HKEY, subkey: &str) -> Result<HKEY, Box<dyn Error>> {
    let mut key: HKEY = std::ptr::null_mut();
    let subkey = win::wide_null(subkey);
    let result = unsafe {
        RegCreateKeyExW(
            root,
            subkey.as_ptr(),
            0,
            std::ptr::null_mut(),
            0,
            KEY_SET_VALUE,
            std::ptr::null(),
            &mut key,
            std::ptr::null_mut(),
        )
    };

    if result == ERROR_SUCCESS {
        Ok(key)
    } else {
        Err(format!("registry open failed: {result}").into())
    }
}

fn set_reg_string(root: HKEY, subkey: &str, name: &str, value: &str) -> Result<(), Box<dyn Error>> {
    let key = open_reg_key(root, subkey)?;
    let name = win::wide_null(name);
    let value = win::wide_null(value);
    let bytes = unsafe {
        std::slice::from_raw_parts(value.as_ptr() as *const u8, value.len() * size_of::<u16>())
    };

    let result = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_SZ,
            bytes.as_ptr(),
            bytes.len() as u32,
        )
    };
    unsafe {
        RegCloseKey(key);
    }

    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("registry write failed: {result}").into())
    }
}

fn set_reg_dword(root: HKEY, subkey: &str, name: &str, value: u32) -> Result<(), Box<dyn Error>> {
    let key = open_reg_key(root, subkey)?;
    let name = win::wide_null(name);
    let bytes = value.to_le_bytes();
    let result = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            REG_DWORD,
            bytes.as_ptr(),
            bytes.len() as u32,
        )
    };
    unsafe {
        RegCloseKey(key);
    }

    if result == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("registry write failed: {result}").into())
    }
}

fn read_reg_string(root: HKEY, subkey: &str, name: &str) -> Option<String> {
    let subkey = win::wide_null(subkey);
    let name = win::wide_null(name);
    let mut key: HKEY = std::ptr::null_mut();
    let opened = unsafe { RegOpenKeyExW(root, subkey.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) };
    if opened != ERROR_SUCCESS {
        return None;
    }

    let mut value_type = 0;
    let mut byte_len = 0_u32;
    let size_result = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            &mut value_type,
            std::ptr::null_mut(),
            &mut byte_len,
        )
    };
    if size_result != ERROR_SUCCESS || value_type != REG_SZ || byte_len < 2 {
        unsafe {
            RegCloseKey(key);
        }
        return None;
    }

    let mut bytes = vec![0_u8; byte_len as usize];
    let read_result = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            std::ptr::null(),
            &mut value_type,
            bytes.as_mut_ptr(),
            &mut byte_len,
        )
    };
    unsafe {
        RegCloseKey(key);
    }

    if read_result != ERROR_SUCCESS || value_type != REG_SZ {
        return None;
    }

    let words =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const u16, byte_len as usize / 2) };
    let end = words
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(words.len());
    String::from_utf16(&words[..end]).ok()
}

fn delete_reg_value(root: HKEY, subkey: &str, name: &str) {
    if let Ok(key) = open_reg_key(root, subkey) {
        let name = win::wide_null(name);
        unsafe {
            RegDeleteValueW(key, name.as_ptr());
            RegCloseKey(key);
        }
    }
}

fn delete_reg_tree(root: HKEY, subkey: &str) {
    let subkey = win::wide_null(subkey);
    unsafe {
        RegDeleteTreeW(root, subkey.as_ptr());
    }
}

fn validate_uninstall_dir(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let actual = normalized_absolute_path(path)?;
    let expected = normalized_absolute_path(&install_dir())?;

    if same_path(&actual, &expected) {
        Ok(expected)
    } else {
        Err(format!(
            "refusing to remove {}, expected {}",
            actual.display(),
            expected.display()
        )
        .into())
    }
}

fn normalized_absolute_path(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir()?.join(path)
    };

    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            _ => normalized.push(component.as_os_str()),
        }
    }

    Ok(normalized)
}

fn same_path(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };

    normalize(left) == normalize(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninstall_dir_validation_accepts_install_dir() {
        assert!(validate_uninstall_dir(&install_dir()).is_ok());
    }

    #[test]
    fn uninstall_dir_validation_rejects_other_paths() {
        let other = env::temp_dir().join("RazerBatteryDisplay-should-not-delete");
        assert!(validate_uninstall_dir(&other).is_err());
    }
}
