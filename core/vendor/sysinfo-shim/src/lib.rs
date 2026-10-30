//! librespot reports the OS and kernel version to Spotify; nothing else is used.

use std::ffi::CStr;

pub struct System;

fn sysctl_string(name: &CStr) -> Option<String> {
    let mut len: libc::size_t = 0;
    // SAFETY: a null buffer asks sysctlbyname for the value's length.
    let rc = unsafe { libc::sysctlbyname(name.as_ptr(), std::ptr::null_mut(), &mut len, std::ptr::null_mut(), 0) };
    if rc != 0 || len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: buf holds len bytes, as sysctlbyname reported.
    let rc = unsafe { libc::sysctlbyname(name.as_ptr(), buf.as_mut_ptr().cast(), &mut len, std::ptr::null_mut(), 0) };
    if rc != 0 {
        return None;
    }
    let s = CStr::from_bytes_until_nul(&buf).ok()?.to_str().ok()?.to_owned();
    Some(s)
}

impl System {
    pub fn os_version() -> Option<String> {
        sysctl_string(c"kern.osproductversion")
    }

    pub fn kernel_version() -> Option<String> {
        sysctl_string(c"kern.osrelease")
    }
}
