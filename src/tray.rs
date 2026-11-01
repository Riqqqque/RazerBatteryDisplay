use std::{
    error::Error,
    mem, ptr,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT,
        WPARAM,
    },
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{
            CreateMutexW, GetCurrentProcess, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
            PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
            ProcessPowerThrottling, SetProcessInformation, SetProcessWorkingSetSize,
        },
    },
    UI::{
        Input::{
            Ime::ImmDisableIME, RAWINPUTDEVICE, RIDEV_INPUTSINK, RIDEV_REMOVE,
            RegisterRawInputDevices,
        },
        Shell::{
            NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
            Shell_NotifyIconW,
        },
        WindowsAndMessaging::{
            AppendMenuW, CREATESTRUCTW, CW_USEDEFAULT, CreateIcon, CreatePopupMenu,
            CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow,
            DispatchMessageW, GWLP_USERDATA, GetCursorPos, GetMessageW, GetWindowLongPtrW, HICON,
            HMENU, IDC_ARROW, IDI_APPLICATION, IMAGE_ICON, KillTimer, LR_DEFAULTSIZE, LoadCursorW,
            LoadImageW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG, PostMessageW, PostQuitMessage,
            RegisterClassW, RegisterWindowMessageW, SetCoalescableTimer, SetForegroundWindow,
            SetWindowLongPtrW, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TrackPopupMenu, TranslateMessage,
            WM_APP, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_INPUT, WM_LBUTTONDBLCLK,
            WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
        },
    },
};

use crate::{APP_NAME, WINDOW_CLASS, battery, install, win};

const TRAY_UID: u32 = 1;
const TRAY_CALLBACK: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const POLL_MS: u32 = 300_000;
const STARTUP_RETRY_MS: [u32; 4] = [10_000, 20_000, 30_000, 60_000];
const TIMER_TOLERANCE_DIVISOR: u32 = 20;
const MOTION_SETTLE_MS: u32 = 750;
const MOTION_RETRY_MS: u32 = 15_000;
const ICON_SIZE: usize = 32;
const ICON_TEXT: u32 = rgb(255, 255, 255);
const GLYPH_WIDTH: usize = 5;
const GLYPH_HEIGHT: usize = 7;
const GLYPH_QUESTION: usize = 10;
#[rustfmt::skip]
const GLYPHS: [[u8; GLYPH_HEIGHT]; 11] = [
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100],
];
const HID_USAGE_PAGE_GENERIC: u16 = 0x01;
const HID_USAGE_GENERIC_MOUSE: u16 = 0x02;
const MANUAL_REFRESH_COOLDOWN: Duration = Duration::from_secs(2);
const INSTANCE_MUTEX: &str = "Local\\RazerBatteryDisplayTray";
const TASKBAR_CREATED: &str = "TaskbarCreated";

const MENU_REFRESH: usize = 1001;
const MENU_STARTUP: usize = 1002;
const MENU_UNINSTALL: usize = 1003;
const MENU_QUIT: usize = 1004;

pub fn run() -> Result<(), Box<dyn Error>> {
    let Some(_instance_guard) = SingleInstance::acquire()? else {
        return Ok(());
    };

    request_efficiency_mode();
    // A tray icon takes no text input, so skip loading the input method libraries.
    unsafe {
        ImmDisableIME(0);
    }

    let class_name = win::wide_null(WINDOW_CLASS);
    let instance = unsafe { GetModuleHandleW(ptr::null()) };
    let taskbar_created = win::wide_null(TASKBAR_CREATED);
    let taskbar_created_message = unsafe { RegisterWindowMessageW(taskbar_created.as_ptr()) };
    if taskbar_created_message == 0 {
        return Err("could not register the taskbar recovery message".into());
    }

    let wc = WNDCLASSW {
        lpfnWndProc: Some(window_proc),
        hInstance: instance,
        lpszClassName: class_name.as_ptr(),
        hCursor: unsafe { LoadCursorW(ptr::null_mut(), IDC_ARROW) },
        ..unsafe { mem::zeroed() }
    };

    let atom = unsafe { RegisterClassW(&wc) };
    if atom == 0 {
        return Err("could not register tray window class".into());
    }

    let mut state = Box::new(AppState::new(taskbar_created_message));
    let hwnd = unsafe {
        CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            WS_OVERLAPPED,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            state.as_mut() as *mut AppState as *mut _,
        )
    };

    if hwnd.is_null() {
        return Err("could not create tray window".into());
    }

    state.hwnd = hwnd;
    let initial_read_succeeded = state.refresh();

    if state
        .schedule_after_refresh(initial_read_succeeded)
        .is_err()
    {
        state.remove_icon();
        unsafe {
            DestroyWindow(hwnd);
        }
        return Err("could not start the battery refresh timer".into());
    }

    let mut msg: MSG = unsafe { mem::zeroed() };
    loop {
        let result = unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) };
        if result == 0 {
            break;
        }
        if result == -1 {
            state.remove_icon();
            return Err(format!(
                "tray message loop failed: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }

        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    state.remove_icon();
    Ok(())
}

struct SingleInstance(HANDLE);

impl SingleInstance {
    fn acquire() -> Result<Option<Self>, Box<dyn Error>> {
        let name = win::wide_null(INSTANCE_MUTEX);
        let handle = unsafe { CreateMutexW(ptr::null(), 1, name.as_ptr()) };
        if handle.is_null() {
            return Err(format!(
                "could not create single-instance mutex: {}",
                std::io::Error::last_os_error()
            )
            .into());
        }

        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            unsafe {
                CloseHandle(handle);
            }
            return Ok(None);
        }

        Ok(Some(Self(handle)))
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

struct AppState {
    hwnd: HWND,
    icon: HICON,
    icon_registered: bool,
    reader: battery::BatteryReader,
    reading: Option<battery::BatteryReading>,
    error: Option<String>,
    display_key: DisplayKey,
    startup_enabled: bool,
    taskbar_created_message: u32,
    last_refresh: Option<Instant>,
    startup_retries: StartupRetrySchedule,
    motion_listening: bool,
    motion_read_pending: bool,
    working_set_trimmed: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum DisplayKey {
    Unknown,
    Battery(u8),
    Error,
}

impl AppState {
    fn new(taskbar_created_message: u32) -> Self {
        Self {
            hwnd: ptr::null_mut(),
            icon: ptr::null_mut(),
            icon_registered: false,
            reader: battery::BatteryReader::new(),
            reading: None,
            error: None,
            display_key: DisplayKey::Unknown,
            startup_enabled: false,
            taskbar_created_message,
            last_refresh: None,
            startup_retries: StartupRetrySchedule::new(),
            motion_listening: false,
            motion_read_pending: false,
            working_set_trimmed: false,
        }
    }

    fn refresh(&mut self) -> bool {
        self.last_refresh = Some(Instant::now());
        let succeeded = match self.reader.read() {
            Ok(reading) => {
                self.reading = Some(reading);
                self.error = None;
                true
            }
            Err(err) => {
                self.reading = None;
                self.error = Some(err.to_string());
                false
            }
        };

        let next_key = self.current_display_key();
        if !self.icon_registered {
            if self.add_or_update_icon(true).is_ok() {
                self.display_key = next_key;
            }
            return succeeded;
        }

        if next_key != self.display_key && self.add_or_update_icon(false).is_ok() {
            self.display_key = next_key;
        }

        succeeded
    }

    fn schedule_after_refresh(&mut self, succeeded: bool) -> Result<(), Box<dyn Error>> {
        self.motion_read_pending = false;
        if succeeded {
            self.stop_motion_listener();
            if !self.working_set_trimmed {
                self.working_set_trimmed = true;
                trim_working_set();
            }
        } else {
            self.start_motion_listener();
        }

        let interval = self.startup_retries.next_interval(succeeded);
        self.set_refresh_timer(interval)
    }

    fn set_refresh_timer(&self, interval: u32) -> Result<(), Box<dyn Error>> {
        let tolerance = interval / TIMER_TOLERANCE_DIVISOR;
        if unsafe { SetCoalescableTimer(self.hwnd, TIMER_ID, interval, None, tolerance) } == 0 {
            Err("could not start the battery refresh timer".into())
        } else {
            Ok(())
        }
    }

    fn refresh_from_timer(&mut self) {
        let motion_read = mem::take(&mut self.motion_read_pending);
        let succeeded = self.refresh();
        let _ = if motion_read && !succeeded {
            // The movement came from something else, or the mouse needs longer.
            // Stay disarmed for a while so another mouse cannot trigger constant reads.
            self.set_refresh_timer(MOTION_RETRY_MS)
        } else {
            self.schedule_after_refresh(succeeded)
        };
    }

    // Raw input stays registered only until the first event, so the app never
    // receives a stream of mouse messages. The event contents are not read.
    fn start_motion_listener(&mut self) {
        if !self.motion_listening {
            self.motion_listening = register_mouse_input(RIDEV_INPUTSINK, self.hwnd);
        }
    }

    fn stop_motion_listener(&mut self) {
        if self.motion_listening {
            register_mouse_input(RIDEV_REMOVE, ptr::null_mut());
            self.motion_listening = false;
        }
    }

    fn on_mouse_motion(&mut self) {
        if !self.motion_listening {
            return;
        }

        self.stop_motion_listener();
        self.motion_read_pending = true;
        let _ = self.set_refresh_timer(MOTION_SETTLE_MS);
    }

    fn refresh_manual(&mut self) {
        if self
            .last_refresh
            .is_some_and(|last| last.elapsed() < MANUAL_REFRESH_COOLDOWN)
        {
            return;
        }
        let succeeded = self.refresh();
        if succeeded || !self.startup_retries.is_active() {
            let _ = self.schedule_after_refresh(succeeded);
        }
    }

    fn restore_icon(&mut self) {
        self.icon_registered = false;
        let _ = self.add_or_update_icon(true);
    }

    fn add_or_update_icon(&mut self, add: bool) -> Result<(), Box<dyn Error>> {
        let icon = make_icon(self.reading.as_ref())?;
        let nid = self.notify_data(icon);
        let op = if add { NIM_ADD } else { NIM_MODIFY };
        let ok = unsafe { Shell_NotifyIconW(op, &nid) };
        if ok == 0 {
            unsafe {
                DestroyIcon(icon);
            }
            return Err("could not update tray icon".into());
        }

        if !self.icon.is_null() {
            unsafe {
                DestroyIcon(self.icon);
            }
        }
        self.icon = icon;
        self.icon_registered = true;
        Ok(())
    }

    fn remove_icon(&mut self) {
        let nid = self.notify_data(self.icon);
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &nid);
        }

        if !self.icon.is_null() {
            unsafe {
                DestroyIcon(self.icon);
            }
            self.icon = ptr::null_mut();
        }
        self.icon_registered = false;
    }

    fn notify_data(&self, icon: HICON) -> NOTIFYICONDATAW {
        let mut nid: NOTIFYICONDATAW = unsafe { mem::zeroed() };
        nid.cbSize = mem::size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = self.hwnd;
        nid.uID = TRAY_UID;
        nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        nid.uCallbackMessage = TRAY_CALLBACK;
        nid.hIcon = icon;

        let tip = self.tooltip();
        let tip = win::wide_null(&tip);
        for slot in nid.szTip.iter_mut() {
            *slot = 0;
        }
        let copy_len = tip
            .len()
            .saturating_sub(1)
            .min(nid.szTip.len().saturating_sub(1));
        for (slot, value) in nid.szTip.iter_mut().take(copy_len).zip(tip.iter()) {
            *slot = *value;
        }

        nid
    }

    fn tooltip(&self) -> String {
        if let Some(reading) = &self.reading {
            return format!("Razer Viper V4 Pro: {}%", reading.percent);
        }

        format!(
            "{}: {}",
            APP_NAME,
            self.error.as_deref().unwrap_or("battery unknown")
        )
    }

    fn show_menu(&mut self) {
        self.startup_enabled = install::startup_enabled();
        let menu = unsafe { CreatePopupMenu() };
        if menu.is_null() {
            return;
        }

        append_menu(menu, MENU_REFRESH, "Refresh");
        append_menu_checked(
            menu,
            MENU_STARTUP,
            "Start with Windows",
            self.startup_enabled,
        );
        unsafe {
            AppendMenuW(menu, MF_SEPARATOR, 0, ptr::null());
        }
        append_menu(menu, MENU_UNINSTALL, "Uninstall");
        append_menu(menu, MENU_QUIT, "Quit");

        let mut point: POINT = unsafe { mem::zeroed() };
        unsafe {
            GetCursorPos(&mut point);
            SetForegroundWindow(self.hwnd);
            TrackPopupMenu(
                menu,
                TPM_LEFTALIGN | TPM_BOTTOMALIGN,
                point.x,
                point.y,
                0,
                self.hwnd,
                ptr::null(),
            );
            PostMessageW(self.hwnd, WM_NULL, 0, 0);
            DestroyMenu(menu);
        }
    }

    fn toggle_startup(&mut self) {
        let next = !self.startup_enabled;
        match install::set_startup_enabled(next) {
            Ok(()) => self.startup_enabled = install::startup_enabled(),
            Err(err) => win::message_box(
                APP_NAME,
                &format!("Could not change the startup setting.\n\n{err}"),
            ),
        }
    }

    fn uninstall(&self) {
        let result = std::env::current_exe().and_then(|exe| {
            std::process::Command::new(exe)
                .arg("--uninstall")
                .spawn()
                .map(|_| ())
        });
        match result {
            Ok(()) => unsafe {
                DestroyWindow(self.hwnd);
            },
            Err(err) => win::message_box_error(
                APP_NAME,
                &format!("Could not start the uninstaller.\n\n{err}"),
            ),
        }
    }

    fn current_display_key(&self) -> DisplayKey {
        if let Some(reading) = self.reading {
            DisplayKey::Battery(reading.percent)
        } else if self.error.is_some() {
            DisplayKey::Error
        } else {
            DisplayKey::Unknown
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_CREATE {
        let createstruct = lparam as *const CREATESTRUCTW;
        if !createstruct.is_null() {
            let state = unsafe { (*createstruct).lpCreateParams as *mut AppState };
            unsafe {
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
            }
        }
    }

    let state_ptr = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut AppState };
    if !state_ptr.is_null() && message == unsafe { (*state_ptr).taskbar_created_message } {
        unsafe {
            (*state_ptr).restore_icon();
        }
        return 0;
    }

    let state = unsafe { state_ptr.as_mut() };

    match message {
        TRAY_CALLBACK => {
            if let Some(state) = state {
                match lparam as u32 {
                    WM_RBUTTONUP => state.show_menu(),
                    WM_LBUTTONUP => state.refresh_manual(),
                    WM_LBUTTONDBLCLK => win::message_box(APP_NAME, &state.tooltip()),
                    _ => {}
                }
            }
            0
        }
        WM_COMMAND => {
            if let Some(state) = state {
                match wparam & 0xffff {
                    MENU_REFRESH => state.refresh_manual(),
                    MENU_STARTUP => state.toggle_startup(),
                    MENU_UNINSTALL => state.uninstall(),
                    MENU_QUIT => unsafe {
                        DestroyWindow(hwnd);
                    },
                    _ => {}
                }
            }
            0
        }
        WM_TIMER => {
            if wparam == TIMER_ID
                && let Some(state) = state
            {
                state.refresh_from_timer();
            }
            0
        }
        WM_INPUT => {
            if let Some(state) = state {
                state.on_mouse_motion();
            }
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }
        WM_CLOSE => {
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            if let Some(state) = state {
                state.stop_motion_listener();
                state.remove_icon();
            }
            unsafe {
                KillTimer(hwnd, TIMER_ID);
            }
            unsafe {
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

struct StartupRetrySchedule {
    next_retry: usize,
}

impl StartupRetrySchedule {
    fn new() -> Self {
        Self { next_retry: 0 }
    }

    fn is_active(&self) -> bool {
        self.next_retry <= STARTUP_RETRY_MS.len()
    }

    fn next_interval(&mut self, succeeded: bool) -> u32 {
        if succeeded {
            self.next_retry = STARTUP_RETRY_MS.len() + 1;
            return POLL_MS;
        }

        if let Some(interval) = STARTUP_RETRY_MS.get(self.next_retry).copied() {
            self.next_retry += 1;
            interval
        } else {
            self.next_retry = STARTUP_RETRY_MS.len() + 1;
            POLL_MS
        }
    }
}

// Asks Windows to run this process on its most efficient scheduling. Best effort only.
fn request_efficiency_mode() {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
    };
    unsafe {
        SetProcessInformation(
            GetCurrentProcess(),
            ProcessPowerThrottling,
            &state as *const PROCESS_POWER_THROTTLING_STATE as *const _,
            mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
        );
    }
}

// Startup-only pages are never used again, so hand them back once after the first reading.
fn trim_working_set() {
    unsafe {
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

fn register_mouse_input(flags: u32, target: HWND) -> bool {
    let device = RAWINPUTDEVICE {
        usUsagePage: HID_USAGE_PAGE_GENERIC,
        usUsage: HID_USAGE_GENERIC_MOUSE,
        dwFlags: flags,
        hwndTarget: target,
    };
    unsafe { RegisterRawInputDevices(&device, 1, mem::size_of::<RAWINPUTDEVICE>() as u32) != 0 }
}

fn append_menu(menu: HMENU, id: usize, label: &str) {
    let label = win::wide_null(label);
    unsafe {
        AppendMenuW(menu, MF_STRING, id, label.as_ptr());
    }
}

fn append_menu_checked(menu: HMENU, id: usize, label: &str, checked: bool) {
    let label = win::wide_null(label);
    let flags = if checked {
        MF_STRING | MF_CHECKED
    } else {
        MF_STRING
    };
    unsafe {
        AppendMenuW(menu, flags, id, label.as_ptr());
    }
}

fn make_icon(reading: Option<&battery::BatteryReading>) -> Result<HICON, Box<dyn Error>> {
    let percent = reading.map(|reading| reading.percent);
    let pixels = render_icon(percent);
    let mask = [0_u8; ICON_SIZE * ICON_SIZE / 8];
    let icon = unsafe {
        CreateIcon(
            ptr::null_mut(),
            ICON_SIZE as i32,
            ICON_SIZE as i32,
            1,
            32,
            mask.as_ptr(),
            pixels.as_ptr().cast(),
        )
    };

    if icon.is_null() {
        load_fallback_icon()
    } else {
        Ok(icon)
    }
}

fn render_icon(percent: Option<u8>) -> [u32; ICON_SIZE * ICON_SIZE] {
    let mut pixels = [icon_color(percent); ICON_SIZE * ICON_SIZE];
    let mut glyphs = [GLYPH_QUESTION; 3];
    let count = match percent {
        Some(value @ 100..) => {
            glyphs = [
                (value / 100) as usize,
                (value / 10 % 10) as usize,
                (value % 10) as usize,
            ];
            3
        }
        Some(value @ 10..) => {
            glyphs[0] = (value / 10) as usize;
            glyphs[1] = (value % 10) as usize;
            2
        }
        Some(value) => {
            glyphs[0] = value as usize;
            1
        }
        None => 1,
    };

    let scale = if count == 3 { 2 } else { 3 };
    let gap = scale - 1;
    let width = count * GLYPH_WIDTH * scale + (count - 1) * gap;
    let left = (ICON_SIZE - width) / 2;
    let top = (ICON_SIZE - GLYPH_HEIGHT * scale) / 2;

    for (index, glyph) in glyphs[..count].iter().enumerate() {
        let origin = left + index * (GLYPH_WIDTH * scale + gap);
        for (row, bits) in GLYPHS[*glyph].iter().enumerate() {
            for column in 0..GLYPH_WIDTH {
                if bits & (1 << (GLYPH_WIDTH - 1 - column)) == 0 {
                    continue;
                }
                for y in 0..scale {
                    for x in 0..scale {
                        pixels[(top + row * scale + y) * ICON_SIZE + origin + column * scale + x] =
                            ICON_TEXT;
                    }
                }
            }
        }
    }

    pixels
}

fn load_fallback_icon() -> Result<HICON, Box<dyn Error>> {
    let icon = unsafe {
        LoadImageW(
            ptr::null_mut(),
            IDI_APPLICATION,
            IMAGE_ICON,
            32,
            32,
            LR_DEFAULTSIZE,
        ) as HICON
    };
    if icon.is_null() {
        Err("could not create or load a tray icon".into())
    } else {
        Ok(icon)
    }
}

fn icon_color(percent: Option<u8>) -> u32 {
    match percent {
        None => rgb(64, 68, 72),
        Some(0..=15) => rgb(198, 52, 45),
        Some(16..=35) => rgb(198, 130, 35),
        Some(_) => rgb(35, 145, 82),
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    0xFF00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_text_stays_inside_the_canvas_for_every_percentage() {
        assert!(render_icon(None).contains(&ICON_TEXT));
        for percent in 0..=100 {
            assert!(render_icon(Some(percent)).contains(&ICON_TEXT));
        }
    }

    #[test]
    fn three_digit_icon_uses_the_full_width_without_overlap() {
        let pixels = render_icon(Some(100));
        let columns_with_text = (0..ICON_SIZE)
            .filter(|x| (0..ICON_SIZE).any(|y| pixels[y * ICON_SIZE + x] == ICON_TEXT))
            .count();
        assert!(columns_with_text > 20);
    }

    #[test]
    fn startup_failures_use_bounded_backoff_then_normal_polling() {
        let mut schedule = StartupRetrySchedule::new();

        assert_eq!(schedule.next_interval(false), 10_000);
        assert_eq!(schedule.next_interval(false), 20_000);
        assert_eq!(schedule.next_interval(false), 30_000);
        assert_eq!(schedule.next_interval(false), 60_000);
        assert_eq!(schedule.next_interval(false), POLL_MS);
        assert_eq!(schedule.next_interval(false), POLL_MS);
        assert!(!schedule.is_active());
    }

    #[test]
    fn startup_success_immediately_restores_normal_polling() {
        let mut schedule = StartupRetrySchedule::new();

        assert_eq!(schedule.next_interval(false), 10_000);
        assert!(schedule.is_active());
        assert_eq!(schedule.next_interval(true), POLL_MS);
        assert!(!schedule.is_active());
        assert_eq!(schedule.next_interval(false), POLL_MS);
    }
}
