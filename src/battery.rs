use std::{error::Error, fmt, ptr, thread, time::Duration};

use windows_sys::{
    Win32::{
        Devices::DeviceAndDriverInstallation::{
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_Get_Device_Interface_List_SizeW,
            CM_Get_Device_Interface_ListW, CR_BUFFER_SMALL, CR_SUCCESS,
        },
        Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
        System::IO::DeviceIoControl,
    },
    core::GUID,
};

// GUID_DEVINTERFACE_HID
const HID_INTERFACE_GUID: GUID = GUID::from_u128(0x4d1e55b2_f16f_11cf_88cb_001111000030);
const IOCTL_HID_SET_FEATURE: u32 = 0x000B_0191;
const IOCTL_HID_GET_FEATURE: u32 = 0x000B_0192;

const VIPER_V4_PRO_WIRED_PID: u16 = 0x00E5;
const VIPER_V4_PRO_WIRELESS_PID: u16 = 0x00E6;
const SAFE_FEATURE_INTERFACE_HINTS: [&str; 2] = ["mi_03", "mi_04"];

const RAZER_REPORT_LEN: usize = 90;
const HID_REPORT_LEN: usize = RAZER_REPORT_LEN + 1;
const STATUS_SUCCESS: u8 = 0x02;
const TRANSACTION_ID: u8 = 0x1f;
const CMD_CLASS_MISC: u8 = 0x07;
const CMD_GET_BATTERY: u8 = 0x80;
const BATTERY_DATA_SIZE: u8 = 0x02;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryReading {
    pub percent: u8,
    pub raw: u8,
}

#[derive(Debug, Default)]
pub struct BatteryReader {
    cached_path: Option<String>,
}

impl BatteryReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn read(&mut self) -> Result<BatteryReading, BatteryError> {
        let mut errors = Vec::new();
        let failed_path = self.cached_path.take();
        if let Some(path) = failed_path.as_deref() {
            match read_path(path) {
                Ok(reading) => {
                    self.cached_path = failed_path;
                    return Ok(reading);
                }
                Err(err) => errors.push(err.to_string()),
            }
        }

        let candidates = hid_candidates()?;
        if candidates.is_empty() {
            return Err(BatteryError::new(
                "Razer Viper V4 Pro battery interface was not found over USB.",
            ));
        }

        for candidate in candidates {
            if failed_path.as_deref() == Some(candidate.path.as_str()) {
                continue;
            }

            match read_candidate(&candidate) {
                Ok(reading) => {
                    self.cached_path = Some(candidate.path);
                    return Ok(reading);
                }
                Err(err) => errors.push(format!("PID {:04X}: {err}", candidate.pid)),
            }
        }

        Err(BatteryError::new(format!(
            "Found the mouse, but could not read battery. {}",
            errors.join(" | ")
        )))
    }
}

#[derive(Debug, Clone)]
pub struct BatteryError {
    message: String,
}

impl BatteryError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for BatteryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for BatteryError {}

#[derive(Debug, Clone)]
struct HidCandidate {
    path: String,
    pid: u16,
}

pub fn read_viper_v4() -> Result<BatteryReading, BatteryError> {
    BatteryReader::new().read()
}

pub fn probe_report() -> String {
    probe_report_inner(false)
}

pub fn probe_report_verbose() -> String {
    probe_report_inner(true)
}

fn probe_report_inner(verbose: bool) -> String {
    let mut lines = vec!["Razer Battery Display probe".to_string()];

    match hid_candidates() {
        Ok(candidates) if candidates.is_empty() => {
            lines.push("no Viper V4 Pro HID devices found".to_string());
        }
        Ok(candidates) => {
            for candidate in &candidates {
                if verbose {
                    lines.push(format!(
                        "found PID {:04X}, product Razer Viper V4 Pro, path {}",
                        candidate.pid, candidate.path
                    ));
                } else {
                    lines.push(format!(
                        "found PID {:04X}, product Razer Viper V4 Pro, interface {}",
                        candidate.pid,
                        path_interface_hint(&candidate.path)
                    ));
                }
            }
        }
        Err(err) => lines.push(format!("HID enumeration failed: {err}")),
    }

    match read_viper_v4() {
        Ok(reading) => lines.push(format!("battery: {}% raw={}", reading.percent, reading.raw)),
        Err(err) => lines.push(format!("battery read failed: {err}")),
    }

    lines.join("\n")
}

fn hid_candidates() -> Result<Vec<HidCandidate>, BatteryError> {
    let paths = hid_interface_paths()?;
    let mut candidates: Vec<HidCandidate> = paths
        .split(|word| *word == 0)
        .filter(|path| !path.is_empty())
        .filter_map(|path| {
            let path = String::from_utf16(path).ok()?;
            let lower_path = path.to_ascii_lowercase();
            candidate_from_path(path, &lower_path)
        })
        .collect();

    candidates.sort_by_cached_key(|candidate| {
        let lower = candidate.path.to_ascii_lowercase();
        (
            interface_preference(&lower),
            candidate.pid != VIPER_V4_PRO_WIRELESS_PID,
            candidate.path.clone(),
        )
    });

    Ok(candidates)
}

fn hid_interface_paths() -> Result<Vec<u16>, BatteryError> {
    for _ in 0..3 {
        let mut len = 0_u32;
        let status = unsafe {
            CM_Get_Device_Interface_List_SizeW(
                &mut len,
                &HID_INTERFACE_GUID,
                ptr::null(),
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        if status != CR_SUCCESS {
            return Err(BatteryError::new(format!(
                "could not size the HID device list: CONFIGRET {status}"
            )));
        }

        let mut paths = vec![0_u16; len as usize];
        let status = unsafe {
            CM_Get_Device_Interface_ListW(
                &HID_INTERFACE_GUID,
                ptr::null(),
                paths.as_mut_ptr(),
                len,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        match status {
            CR_SUCCESS => return Ok(paths),
            CR_BUFFER_SMALL => continue,
            _ => {
                return Err(BatteryError::new(format!(
                    "could not list HID devices: CONFIGRET {status}"
                )));
            }
        }
    }

    Err(BatteryError::new("the HID device list kept changing"))
}

fn candidate_from_path(path: String, lower_path: &str) -> Option<HidCandidate> {
    if !is_supported_battery_path(lower_path) {
        return None;
    }

    Some(HidCandidate {
        path,
        pid: pid_from_path(lower_path)?,
    })
}

fn read_candidate(candidate: &HidCandidate) -> Result<BatteryReading, BatteryError> {
    read_path(&candidate.path)
}

fn read_path(path: &str) -> Result<BatteryReading, BatteryError> {
    if !is_supported_battery_path(&path.to_ascii_lowercase()) {
        return Err(BatteryError::new(
            "refusing to query a non-battery HID interface",
        ));
    }

    let handle = DeviceHandle::open(path)?;
    let raw = read_battery_byte(handle.0)?;

    Ok(BatteryReading {
        percent: raw_to_percent(raw),
        raw,
    })
}

fn read_battery_byte(handle: HANDLE) -> Result<u8, BatteryError> {
    let mut last_error = None;

    for _ in 0..2 {
        let mut request = build_report();
        if !hid_feature_ioctl(handle, IOCTL_HID_SET_FEATURE, &mut request) {
            last_error = Some(format!(
                "set feature failed: {}",
                std::io::Error::last_os_error()
            ));
            thread::sleep(Duration::from_millis(50));
            continue;
        }

        thread::sleep(Duration::from_millis(65));

        let mut response = [0_u8; HID_REPORT_LEN];
        if !hid_feature_ioctl(handle, IOCTL_HID_GET_FEATURE, &mut response) {
            last_error = Some(format!(
                "get feature failed: {}",
                std::io::Error::last_os_error()
            ));
            thread::sleep(Duration::from_millis(75));
            continue;
        }

        match validate_response(&response) {
            Ok(value) => return Ok(value),
            Err(err) => last_error = Some(err),
        }

        thread::sleep(Duration::from_millis(75));
    }

    Err(BatteryError::new(last_error.unwrap_or_else(|| {
        "device did not return a usable response".to_string()
    })))
}

// Same driver request as HidD_SetFeature/HidD_GetFeature, without loading hid.dll.
fn hid_feature_ioctl(handle: HANDLE, code: u32, report: &mut [u8; HID_REPORT_LEN]) -> bool {
    let mut returned = 0_u32;
    let buffer = report.as_mut_ptr().cast();
    let len = report.len() as u32;
    unsafe {
        DeviceIoControl(
            handle,
            code,
            buffer,
            len,
            buffer,
            len,
            &mut returned,
            ptr::null_mut(),
        ) != 0
    }
}

fn build_report() -> [u8; HID_REPORT_LEN] {
    let mut report = [0_u8; HID_REPORT_LEN];
    report[2] = TRANSACTION_ID;
    report[6] = BATTERY_DATA_SIZE;
    report[7] = CMD_CLASS_MISC;
    report[8] = CMD_GET_BATTERY;
    report[89] = calculate_crc(&report);
    report
}

fn calculate_crc(report: &[u8; HID_REPORT_LEN]) -> u8 {
    report[3..89].iter().fold(0_u8, |crc, byte| crc ^ byte)
}

fn validate_response(response: &[u8; HID_REPORT_LEN]) -> Result<u8, String> {
    if response[1] != STATUS_SUCCESS {
        return Err(format!("unexpected status 0x{:02X}", response[1]));
    }

    if response[2] != TRANSACTION_ID {
        return Err(format!("unexpected transaction id 0x{:02X}", response[2]));
    }

    if response[6] != BATTERY_DATA_SIZE
        || response[7] != CMD_CLASS_MISC
        || response[8] != CMD_GET_BATTERY
    {
        return Err(format!(
            "unexpected response header size=0x{:02X} class=0x{:02X} command=0x{:02X}",
            response[6], response[7], response[8]
        ));
    }

    // A sleeping mouse can leave the dongle answering with an empty value.
    if response[10] == 0 {
        return Err("device returned no battery value".to_string());
    }

    Ok(response[10])
}

fn is_supported_battery_path(path: &str) -> bool {
    has_path_component(path, "vid_1532")
        && (has_path_component(path, "pid_00e5") || has_path_component(path, "pid_00e6"))
        && !path_components(path).any(|component| component.starts_with("col"))
        && SAFE_FEATURE_INTERFACE_HINTS
            .iter()
            .any(|hint| has_path_component(path, hint))
}

fn pid_from_path(path: &str) -> Option<u16> {
    if has_path_component(path, "pid_00e6") {
        Some(VIPER_V4_PRO_WIRELESS_PID)
    } else if has_path_component(path, "pid_00e5") {
        Some(VIPER_V4_PRO_WIRED_PID)
    } else {
        None
    }
}

fn interface_preference(path: &str) -> usize {
    SAFE_FEATURE_INTERFACE_HINTS
        .iter()
        .position(|hint| has_path_component(path, hint))
        .unwrap_or(SAFE_FEATURE_INTERFACE_HINTS.len())
}

fn path_interface_hint(path: &str) -> String {
    let lower = path.to_ascii_lowercase();
    for prefix in ["mi_00", "mi_01", "mi_02", "mi_03", "mi_04"] {
        if has_path_component(&lower, prefix) {
            return prefix.to_string();
        }
    }

    "unknown".to_string()
}

fn path_components(path: &str) -> impl Iterator<Item = &str> {
    path.split(['#', '&'])
}

fn has_path_component(path: &str, expected: &str) -> bool {
    path_components(path).any(|component| component == expected)
}

fn raw_to_percent(raw: u8) -> u8 {
    (((raw as u16) * 100 + 127) / 255).min(100) as u8
}

struct DeviceHandle(HANDLE);

impl DeviceHandle {
    fn open(path: &str) -> Result<Self, BatteryError> {
        let path = crate::win::wide_null(path);
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                ptr::null(),
                OPEN_EXISTING,
                0,
                ptr::null_mut(),
            )
        };

        if handle == INVALID_HANDLE_VALUE {
            Err(BatteryError::new(format!(
                "open failed: {}",
                std::io::Error::last_os_error()
            )))
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for DeviceHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_has_expected_razer_battery_request_layout() {
        let report = build_report();

        assert_eq!(report.len(), HID_REPORT_LEN);
        assert_eq!(report[0], 0);
        assert_eq!(report[1], 0);
        assert_eq!(report[2], TRANSACTION_ID);
        assert_eq!(report[6], BATTERY_DATA_SIZE);
        assert_eq!(report[7], CMD_CLASS_MISC);
        assert_eq!(report[8], CMD_GET_BATTERY);
        assert_eq!(report[89], calculate_crc(&report));
        assert_eq!(report[90], 0);
    }

    #[test]
    fn validates_battery_response() {
        let mut response = [0_u8; HID_REPORT_LEN];
        response[1] = STATUS_SUCCESS;
        response[2] = TRANSACTION_ID;
        response[6] = BATTERY_DATA_SIZE;
        response[7] = CMD_CLASS_MISC;
        response[8] = CMD_GET_BATTERY;
        response[10] = 128;

        assert_eq!(validate_response(&response).unwrap(), 128);
    }

    #[test]
    fn rejects_unexpected_response_headers() {
        let mut response = [0_u8; HID_REPORT_LEN];
        response[1] = 0x03;
        response[2] = TRANSACTION_ID;
        response[6] = BATTERY_DATA_SIZE;
        response[7] = CMD_CLASS_MISC;
        response[8] = CMD_GET_BATTERY;

        assert!(validate_response(&response).is_err());
    }

    #[test]
    fn rejects_busy_and_empty_battery_responses() {
        let mut response = [0_u8; HID_REPORT_LEN];
        response[1] = 0x01;
        response[2] = TRANSACTION_ID;
        response[6] = BATTERY_DATA_SIZE;
        response[7] = CMD_CLASS_MISC;
        response[8] = CMD_GET_BATTERY;
        response[10] = 128;
        assert!(validate_response(&response).is_err());

        response[1] = STATUS_SUCCESS;
        response[10] = 0;
        assert!(validate_response(&response).is_err());
    }

    #[test]
    fn raw_battery_values_map_to_percentages() {
        assert_eq!(raw_to_percent(0), 0);
        assert_eq!(raw_to_percent(128), 50);
        assert_eq!(raw_to_percent(255), 100);
    }

    #[test]
    fn probe_interface_hint_avoids_instance_id() {
        let hint = path_interface_hint(r"\\?\hid#vid_1532&pid_00e6&mi_03#b&1d7ae382&0&0000#...");
        assert_eq!(hint, "mi_03");
    }

    #[test]
    fn only_safe_feature_interfaces_are_supported() {
        assert!(is_supported_battery_path(
            r"\\?\hid#vid_1532&pid_00e6&mi_03#b&1d7ae382&0&0000#{...}"
        ));
        assert!(is_supported_battery_path(
            r"\\?\hid#vid_1532&pid_00e5&mi_04#b&1d7ae382&0&0000#{...}"
        ));
        assert!(!is_supported_battery_path(
            r"\\?\hid#vid_1532&pid_00e6&mi_01&col02#b&1d7ae382&0&0001#{...}"
        ));
        assert!(!is_supported_battery_path(
            r"\\?\hid#vid_1532&pid_00e5&mi_00#b&1d7ae382&0&0000#{...}"
        ));
        assert!(!is_supported_battery_path(
            r"\\?\hid#vid_1532&pid_0099&mi_03#b&1d7ae382&0&0000#{...}"
        ));
        assert!(!is_supported_battery_path(
            r"\\?\hid#vid_15320&pid_00e6&mi_03#b&1d7ae382&0&0000#{...}"
        ));
        assert!(!is_supported_battery_path(
            r"\\?\hid#vid_1532&pid_00e60&mi_03#b&1d7ae382&0&0000#{...}"
        ));
        assert!(!is_supported_battery_path(
            r"\\?\hid#vid_1532&pid_00e6&mi_030#b&1d7ae382&0&0000#{...}"
        ));
    }

    #[test]
    fn pid_is_read_from_supported_path() {
        assert_eq!(
            pid_from_path(r"\\?\hid#vid_1532&pid_00e6&mi_03#..."),
            Some(VIPER_V4_PRO_WIRELESS_PID)
        );
        assert_eq!(
            pid_from_path(r"\\?\hid#vid_1532&pid_00e5&mi_04#..."),
            Some(VIPER_V4_PRO_WIRED_PID)
        );
        assert_eq!(pid_from_path(r"\\?\hid#vid_1532&pid_0099&mi_03#..."), None);
    }
}
