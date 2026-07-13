use std::{ffi::OsStr, os::windows::ffi::OsStrExt, ptr};

use windows_sys::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW,
};

pub fn wide_null(value: &str) -> Vec<u16> {
    OsStr::new(value).encode_wide().chain(Some(0)).collect()
}

pub fn message_box(title: &str, body: &str) {
    message_box_with_style(title, body, MB_ICONINFORMATION);
}

pub fn message_box_error(title: &str, body: &str) {
    message_box_with_style(title, body, MB_ICONERROR);
}

fn message_box_with_style(title: &str, body: &str, style: u32) {
    let title = wide_null(title);
    let body = wide_null(body);

    unsafe {
        MessageBoxW(
            ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | style,
        );
    }
}
