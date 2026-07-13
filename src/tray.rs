use std::{
    error::Error,
    mem, ptr,
    time::{Duration, Instant},
};

use windows_sys::Win32::{
    Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, POINT,
        RECT, WPARAM,
    },
    Graphics::Gdi::{
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateBitmap, CreateCompatibleBitmap,
        CreateCompatibleDC, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH,
        DT_CENTER, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FF_DONTCARE,
        FW_BOLD, FillRect, GetDC, HBRUSH, HDC, HGDIOBJ, OUT_DEFAULT_PRECIS, ReleaseDC,
        SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    },
    System::{LibraryLoader::GetModuleHandleW, Threading::CreateMutexW},
    UI::{
        Shell::{
            NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
            Shell_NotifyIconW,
        },
        WindowsAndMessaging::{
            AppendMenuW, CREATESTRUCTW, CW_USEDEFAULT, CreateIconIndirect, CreatePopupMenu,
            CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow,
            DispatchMessageW, GWLP_USERDATA, GetCursorPos, GetMessageW, GetWindowLongPtrW, HICON,
            HMENU, ICONINFO, IDC_ARROW, IDI_APPLICATION, IMAGE_ICON, KillTimer, LR_DEFAULTSIZE,
            LoadCursorW, LoadImageW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG, PostMessageW,
            PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow, SetTimer,
            SetWindowLongPtrW, TPM_BOTTOMALIGN, TPM_LEFTALIGN, TrackPopupMenu, TranslateMessage,
            WM_APP, WM_CLOSE, WM_COMMAND, WM_CREATE, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP,
            WM_NULL, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
        },
    },
};

use crate::{APP_NAME, WINDOW_CLASS, battery, install, win};

const TRAY_UID: u32 = 1;
const TRAY_CALLBACK: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const POLL_MS: u32 = 300_000;
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
    state.refresh();
    state.add_or_update_icon(true)?;

    if unsafe { SetTimer(hwnd, TIMER_ID, POLL_MS, None) } == 0 {
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
            startup_enabled: install::startup_enabled(),
            taskbar_created_message,
            last_refresh: None,
        }
    }

    fn refresh(&mut self) {
        self.last_refresh = Some(Instant::now());
        match self.reader.read() {
            Ok(reading) => {
                self.reading = Some(reading);
                self.error = None;
            }
            Err(err) => {
                self.reading = None;
                self.error = Some(err.to_string());
            }
        }

        let next_key = self.current_display_key();
        if !self.icon.is_null() && !self.icon_registered {
            if self.add_or_update_icon(true).is_ok() {
                self.display_key = next_key;
            }
            return;
        }

        if next_key != self.display_key
            && (self.icon.is_null() || self.add_or_update_icon(false).is_ok())
        {
            self.display_key = next_key;
        }
    }

    fn refresh_manual(&mut self) {
        if self
            .last_refresh
            .is_some_and(|last| last.elapsed() < MANUAL_REFRESH_COOLDOWN)
        {
            return;
        }
        self.refresh();
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
                state.refresh();
            }
            0
        }
        WM_CLOSE => {
            unsafe {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_DESTROY => {
            if let Some(state) = state {
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
    let label = reading
        .map(|reading| reading.percent.to_string())
        .unwrap_or_else(|| "?".to_string());
    let percent = reading.map(|reading| reading.percent);
    let bg = icon_color(percent);
    let label = win::wide_null(&label);
    let font_name = win::wide_null("Segoe UI");

    unsafe {
        let screen_dc = ScreenDc(GetDC(ptr::null_mut()));
        if screen_dc.0.is_null() {
            return load_fallback_icon();
        }

        let dc = MemoryDc(CreateCompatibleDC(screen_dc.0));
        if dc.0.is_null() {
            return load_fallback_icon();
        }

        let bitmap = GdiObject(CreateCompatibleBitmap(screen_dc.0, 32, 32) as HGDIOBJ);
        if bitmap.0.is_null() {
            return load_fallback_icon();
        }
        let old_bitmap = SelectObject(dc.0, bitmap.0);
        if old_bitmap.is_null() {
            return load_fallback_icon();
        }
        let _bitmap_selection = SelectedObject {
            dc: dc.0,
            previous: old_bitmap,
        };

        let brush = GdiObject(CreateSolidBrush(bg) as HGDIOBJ);
        if brush.0.is_null() {
            return load_fallback_icon();
        }
        let rect = RECT {
            left: 0,
            top: 0,
            right: 32,
            bottom: 32,
        };
        if FillRect(dc.0, &rect, brush.0 as HBRUSH) == 0 {
            return load_fallback_icon();
        }

        let font = GdiObject(CreateFontW(
            -16,
            0,
            0,
            0,
            FW_BOLD as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            font_name.as_ptr(),
        ) as HGDIOBJ);
        let _font_selection = if font.0.is_null() {
            None
        } else {
            let previous = SelectObject(dc.0, font.0);
            (!previous.is_null()).then_some(SelectedObject { dc: dc.0, previous })
        };

        SetBkMode(dc.0, TRANSPARENT as i32);
        SetTextColor(dc.0, rgb(255, 255, 255));
        let mut text_rect = RECT {
            left: 0,
            top: 0,
            right: 32,
            bottom: 31,
        };
        DrawTextW(
            dc.0,
            label.as_ptr(),
            -1,
            &mut text_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );

        let mask = GdiObject(CreateBitmap(32, 32, 1, 1, ptr::null()) as HGDIOBJ);
        if mask.0.is_null() {
            return load_fallback_icon();
        }
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask.0,
            hbmColor: bitmap.0,
        };

        let icon = CreateIconIndirect(&info);
        if icon.is_null() {
            load_fallback_icon()
        } else {
            Ok(icon)
        }
    }
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

struct ScreenDc(HDC);

impl Drop for ScreenDc {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                ReleaseDC(ptr::null_mut(), self.0);
            }
        }
    }
}

struct MemoryDc(HDC);

impl Drop for MemoryDc {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DeleteDC(self.0);
            }
        }
    }
}

struct GdiObject(HGDIOBJ);

impl Drop for GdiObject {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                DeleteObject(self.0);
            }
        }
    }
}

struct SelectedObject {
    dc: HDC,
    previous: HGDIOBJ,
}

impl Drop for SelectedObject {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.previous);
        }
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

fn rgb(r: u8, g: u8, b: u8) -> u32 {
    r as u32 | ((g as u32) << 8) | ((b as u32) << 16)
}
