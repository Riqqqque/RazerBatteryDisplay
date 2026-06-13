use std::{error::Error, ffi::c_void, fmt, mem, ptr, thread, time::Duration};

use windows_sys::{
    Win32::{
        Devices::{
            DeviceAndDriverInstallation::{
                DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
                SP_DEVICE_INTERFACE_DETAIL_DATA_W, SetupDiDestroyDeviceInfoList,
                SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW,
                SetupDiGetDeviceInterfaceDetailW,
            },
            HumanInterfaceDevice::{
                HIDD_ATTRIBUTES, HidD_GetAttributes, HidD_GetFeature, HidD_GetHidGuid,
                HidD_GetProductString, HidD_SetFeature,
            },
        },
        Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE},
        Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING},
    },
    core::GUID,
};

const RAZER_VID: u16 = 0x1532;
const VIPER_V4_PRO_WIRED_PID: u16 = 0x00E5;
const VIPER_V4_PRO_WIRELESS_PID: u16 = 0x00E6;

const RAZER_REPORT_LEN: usize = 90;
const HID_REPORT_LEN: usize = RAZER_REPORT_LEN + 1;
const STATUS_SUCCESS: u8 = 0x02;
const STATUS_BUSY: u8 = 0x01;
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
        if let Some(path) = self.cached_path.as_deref() {
            match read_path(path) {
                Ok(reading) => return Ok(reading),
                Err(_) => self.cached_path = None,
            }
        }

        let candidates = hid_candidates()?;
        if candidates.is_empty() {
            return Err(BatteryError::new(
                "Razer Viper V4 Pro was not found over USB.",
            ));
        }

        let mut errors = Vec::new();
        for candidate in candidates {
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
    product: String,
}

pub fn read_viper_v4() -> Result<BatteryReading, BatteryError> {
    BatteryReader::new().read()
}

pub fn probe_report() -> String {
    let mut lines = vec!["Razer Battery Display probe".to_string()];

    match hid_candidates() {
        Ok(candidates) if candidates.is_empty() => {
            lines.push("no Viper V4 Pro HID devices found".to_string());
        }
        Ok(candidates) => {
            for candidate in &candidates {
                lines.push(format!(
                    "found PID {:04X}, product {}, path {}",
                    candidate.pid, candidate.product, candidate.path
                ));
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
    let mut hid_guid: GUID = unsafe { mem::zeroed() };
    unsafe {
        HidD_GetHidGuid(&mut hid_guid);
    }

    let devices = unsafe {
        SetupDiGetClassDevsW(
            &hid_guid,
            ptr::null(),
            ptr::null_mut(),
            DIGCF_PRESENT | DIGCF_DEVICEINTERFACE,
        )
    };

    if devices == INVALID_HANDLE_VALUE as HDEVINFO {
        return Err(BatteryError::new(format!(
            "SetupDiGetClassDevsW failed: {}",
            std::io::Error::last_os_error()
        )));
    }

    let _guard = DeviceInfoSet(devices);
    let mut candidates = Vec::new();
    let mut index = 0;

    loop {
        let mut interface_data = SP_DEVICE_INTERFACE_DATA {
            cbSize: mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };

        let ok = unsafe {
            SetupDiEnumDeviceInterfaces(devices, ptr::null(), &hid_guid, index, &mut interface_data)
        };
        if ok == 0 {
            break;
        }

        if let Some(path) = device_path(devices, &interface_data) {
            let lower_path = path.to_ascii_lowercase();
            if is_viper_path(&lower_path)
                && let Some(candidate) = candidate_from_path(path)
            {
                candidates.push(candidate);
            }
        }

        index += 1;
    }

    candidates.sort_by_key(|candidate| {
        let lower = candidate.path.to_ascii_lowercase();
        (
            !lower.contains("mi_03"),
            candidate.pid != VIPER_V4_PRO_WIRELESS_PID,
            candidate.path.clone(),
        )
    });

    Ok(candidates)
}

fn device_path(devices: HDEVINFO, interface_data: &SP_DEVICE_INTERFACE_DATA) -> Option<String> {
    let mut required_size = 0_u32;
    unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            devices,
            interface_data,
            ptr::null_mut(),
            0,
            &mut required_size,
            ptr::null_mut(),
        );
    }

    if required_size == 0 {
        return None;
    }

    let mut buffer = vec![0_u8; required_size as usize];
    let detail = buffer.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
    unsafe {
        (*detail).cbSize = mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
    }

    let ok = unsafe {
        SetupDiGetDeviceInterfaceDetailW(
            devices,
            interface_data,
            detail,
            required_size,
            ptr::null_mut(),
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return None;
    }

    let path_ptr = unsafe { (*detail).DevicePath.as_ptr() };
    let path_len = unsafe {
        let mut len = 0;
        while *path_ptr.add(len) != 0 {
            len += 1;
        }
        len
    };

    String::from_utf16(unsafe { std::slice::from_raw_parts(path_ptr, path_len) }).ok()
}

fn candidate_from_path(path: String) -> Option<HidCandidate> {
    let handle = DeviceHandle::open(&path).ok()?;
    let mut attributes = HIDD_ATTRIBUTES {
        Size: mem::size_of::<HIDD_ATTRIBUTES>() as u32,
        ..Default::default()
    };

    if unsafe { HidD_GetAttributes(handle.0, &mut attributes) } {
        if attributes.VendorID != RAZER_VID || !is_viper_pid(attributes.ProductID) {
            return None;
        }

        return Some(HidCandidate {
            path,
            pid: attributes.ProductID,
            product: product_string(handle.0),
        });
    }

    None
}

fn read_candidate(candidate: &HidCandidate) -> Result<BatteryReading, BatteryError> {
    read_path(&candidate.path)
}

fn read_path(path: &str) -> Result<BatteryReading, BatteryError> {
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
        let request = build_report();
        let sent = unsafe {
            HidD_SetFeature(
                handle,
                request.as_ptr() as *const c_void,
                request.len() as u32,
            )
        };
        if !sent {
            last_error = Some(format!(
                "HidD_SetFeature failed: {}",
                std::io::Error::last_os_error()
            ));
            thread::sleep(Duration::from_millis(50));
            continue;
        }

        thread::sleep(Duration::from_millis(65));

        let mut response = [0_u8; HID_REPORT_LEN];
        let received = unsafe {
            HidD_GetFeature(
                handle,
                response.as_mut_ptr() as *mut c_void,
                response.len() as u32,
            )
        };
        if !received {
            last_error = Some(format!(
                "HidD_GetFeature failed: {}",
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
    if !matches!(response[1], STATUS_SUCCESS | STATUS_BUSY) {
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

    Ok(response[10])
}

fn product_string(handle: HANDLE) -> String {
    let mut buffer = [0_u16; 128];
    let ok = unsafe {
        HidD_GetProductString(
            handle,
            buffer.as_mut_ptr() as *mut c_void,
            (buffer.len() * size_of::<u16>()) as u32,
        )
    };

    if ok {
        let end = buffer
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(buffer.len());
        if let Ok(value) = String::from_utf16(&buffer[..end])
            && !value.trim().is_empty()
        {
            return value;
        }
    }

    "Razer Viper V4 Pro".to_string()
}

fn is_viper_path(path: &str) -> bool {
    path.contains("vid_1532") && (path.contains("pid_00e5") || path.contains("pid_00e6"))
}

fn is_viper_pid(pid: u16) -> bool {
    matches!(pid, VIPER_V4_PRO_WIRED_PID | VIPER_V4_PRO_WIRELESS_PID)
}

fn raw_to_percent(raw: u8) -> u8 {
    (((raw as u16) * 100 + 127) / 255).min(100) as u8
}

struct DeviceInfoSet(HDEVINFO);

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        unsafe {
            SetupDiDestroyDeviceInfoList(self.0);
        }
    }
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
