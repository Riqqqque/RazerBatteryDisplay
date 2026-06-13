use std::{error::Error, mem, ptr};

use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateBitmap, CreateCompatibleBitmap,
        CreateCompatibleDC, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH,
        DT_CENTER, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW, FF_DONTCARE,
        FW_BOLD, FillRect, HBRUSH, HGDIOBJ, OUT_DEFAULT_PRECIS, ReleaseDC, SelectObject, SetBkMode,
        SetTextColor, TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Shell::{
            NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
            Shell_NotifyIconW,
        },
        WindowsAndMessaging::{
            AppendMenuW, CREATESTRUCTW, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateIconIndirect,
            CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu,
            DestroyWindow, DispatchMessageW, FindWindowW, GWLP_USERDATA, GetCursorPos, GetMessageW,
            GetWindowLongPtrW, HICON, HMENU, ICONINFO, IDC_ARROW, IDI_APPLICATION, IMAGE_ICON,
            LR_DEFAULTSIZE, LoadCursorW, LoadImageW, MF_CHECKED, MF_SEPARATOR, MF_STRING, MSG,
            PostQuitMessage, RegisterClassW, SetForegroundWindow, SetTimer, SetWindowLongPtrW,
            TPM_BOTTOMALIGN, TPM_LEFTALIGN, TrackPopupMenu, TranslateMessage, WM_APP, WM_CLOSE,
            WM_COMMAND, WM_CREATE, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP, WM_RBUTTONUP,
            WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
        },
    },
};

use crate::{APP_NAME, WINDOW_CLASS, battery, install, win};

const TRAY_UID: u32 = 1;
const TRAY_CALLBACK: u32 = WM_APP + 1;
const TIMER_ID: usize = 1;
const POLL_MS: u32 = 300_000;

const MENU_REFRESH: usize = 1001;
const MENU_STARTUP: usize = 1002;
const MENU_UNINSTALL: usize = 1003;
const MENU_QUIT: usize = 1004;

pub fn run() -> Result<(), Box<dyn Error>> {
    if another_instance_running() {
        return Ok(());
    }

    let class_name = win::wide_null(WINDOW_CLASS);
    let instance = unsafe { GetModuleHandleW(ptr::null()) };

    let wc = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
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

    let mut state = Box::new(AppState::new());
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

    unsafe {
        SetTimer(hwnd, TIMER_ID, POLL_MS, None);
    }

    let mut msg: MSG = unsafe { mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, ptr::null_mut(), 0, 0) } > 0 {
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    state.remove_icon();
    Ok(())
}

struct AppState {
    hwnd: HWND,
    icon: HICON,
    reader: battery::BatteryReader,
    reading: Option<battery::BatteryReading>,
    error: Option<String>,
    display_key: DisplayKey,
    startup_enabled: bool,
}

#[derive(Clone, PartialEq, Eq)]
enum DisplayKey {
    Unknown,
    Battery(u8),
    Error,
}

impl AppState {
    fn new() -> Self {
        Self {
            hwnd: ptr::null_mut(),
            icon: ptr::null_mut(),
            reader: battery::BatteryReader::new(),
            reading: None,
            error: None,
            display_key: DisplayKey::Unknown,
            startup_enabled: install::startup_enabled(),
        }
    }

    fn refresh(&mut self) {
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
        if next_key != self.display_key {
            self.display_key = next_key;
            let _ = self.add_or_update_icon(false);
        }
    }

    fn add_or_update_icon(&mut self, add: bool) -> Result<(), Box<dyn Error>> {
        let icon = make_icon(self.reading.as_ref());
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
        for (slot, value) in nid.szTip.iter_mut().zip(tip.iter()) {
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
            DestroyMenu(menu);
        }
    }

    fn toggle_startup(&mut self) {
        let next = !self.startup_enabled;
        if install::set_startup_enabled(next).is_ok() {
            self.startup_enabled = next;
        }
    }

    fn uninstall(&self) {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).arg("--uninstall").spawn();
        }
        unsafe {
            DestroyWindow(self.hwnd);
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
    let state = unsafe { state_ptr.as_mut() };

    match message {
        TRAY_CALLBACK => {
            if let Some(state) = state {
                match lparam as u32 {
                    WM_RBUTTONUP => state.show_menu(),
                    WM_LBUTTONUP => state.refresh(),
                    WM_LBUTTONDBLCLK => win::message_box(APP_NAME, &state.tooltip()),
                    _ => {}
                }
            }
            0
        }
        WM_COMMAND => {
            if let Some(state) = state {
                match wparam & 0xffff {
                    MENU_REFRESH => state.refresh(),
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
                PostQuitMessage(0);
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn another_instance_running() -> bool {
    let class = win::wide_null(WINDOW_CLASS);
    let hwnd = unsafe { FindWindowW(class.as_ptr(), ptr::null()) };
    !hwnd.is_null()
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

fn make_icon(reading: Option<&battery::BatteryReading>) -> HICON {
    let label = reading
        .map(|reading| reading.percent.to_string())
        .unwrap_or_else(|| "?".to_string());
    let percent = reading.map(|reading| reading.percent);
    let bg = icon_color(percent);
    let label = win::wide_null(&label);
    let font_name = win::wide_null("Segoe UI");

    unsafe {
        let screen_dc = windows_sys::Win32::Graphics::Gdi::GetDC(ptr::null_mut());
        let dc = CreateCompatibleDC(screen_dc);
        let bitmap = CreateCompatibleBitmap(screen_dc, 32, 32);
        let old_bitmap = SelectObject(dc, bitmap as HGDIOBJ);

        let brush = CreateSolidBrush(bg);
        let rect = RECT {
            left: 0,
            top: 0,
            right: 32,
            bottom: 32,
        };
        FillRect(dc, &rect, brush as HBRUSH);
        DeleteObject(brush as HGDIOBJ);

        let font = CreateFontW(
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
        );
        let old_font = if font.is_null() {
            ptr::null_mut()
        } else {
            SelectObject(dc, font as HGDIOBJ)
        };

        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, rgb(255, 255, 255));
        let mut text_rect = RECT {
            left: 0,
            top: 0,
            right: 32,
            bottom: 31,
        };
        DrawTextW(
            dc,
            label.as_ptr(),
            -1,
            &mut text_rect,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );

        if !old_font.is_null() {
            SelectObject(dc, old_font);
        }
        if !font.is_null() {
            DeleteObject(font as HGDIOBJ);
        }
        SelectObject(dc, old_bitmap);

        let mask = CreateBitmap(32, 32, 1, 1, ptr::null());
        let info = ICONINFO {
            fIcon: 1,
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: bitmap,
        };

        let icon = CreateIconIndirect(&info);
        DeleteObject(mask as HGDIOBJ);
        DeleteObject(bitmap as HGDIOBJ);
        DeleteDC(dc);
        ReleaseDC(ptr::null_mut(), screen_dc);

        if icon.is_null() {
            LoadImageW(
                ptr::null_mut(),
                IDI_APPLICATION,
                IMAGE_ICON,
                32,
                32,
                LR_DEFAULTSIZE,
            ) as HICON
        } else {
            icon
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
