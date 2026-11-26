//! FFI to the native DAB receiver (native/dab_capi.cpp over nativedab's DabBridge).

use crate::dsp::C32;

#[cfg(feature = "dab")]
mod sys {
    use std::ffi::{c_char, c_int, c_void};
    unsafe extern "C" {
        pub fn rfa_dab_create() -> *mut c_void;
        pub fn rfa_dab_destroy(h: *mut c_void);
        pub fn rfa_dab_start(h: *mut c_void);
        pub fn rfa_dab_stop(h: *mut c_void);
        pub fn rfa_dab_push(h: *mut c_void, iq: *const f32, n: c_int) -> c_int;
        pub fn rfa_dab_select_sid(h: *mut c_void, sid: u32) -> c_int;
        pub fn rfa_dab_describe(h: *mut c_void, sid: u32) -> c_int;
        pub fn rfa_dab_poll_events(h: *mut c_void) -> *mut c_char;
        pub fn rfa_dab_free(p: *mut c_void);
        pub fn rfa_dab_poll_audio(h: *mut c_void, out: *mut i16, cap: c_int) -> c_int;
        pub fn rfa_dab_audio_rate(h: *mut c_void) -> c_int;
        pub fn rfa_dab_take_slide(h: *mut c_void, data: *mut *mut u8, size: *mut c_int, typ: *mut c_int, name: *mut c_char, name_cap: c_int) -> c_int;
        pub fn rfa_dab_poll_constellation(h: *mut c_void, out: *mut f32, max_points: c_int) -> c_int;
        pub fn rfa_dab_dropped(h: *mut c_void) -> u32;
    }
}

pub struct Native {
    #[cfg(feature = "dab")]
    h: *mut std::ffi::c_void,
}

unsafe impl Send for Native {}

#[cfg(feature = "dab")]
impl Native {
    pub fn create() -> Option<Native> {
        let h = unsafe { sys::rfa_dab_create() };
        if h.is_null() { None } else { Some(Native { h }) }
    }
    pub fn start(&self) {
        unsafe { sys::rfa_dab_start(self.h) }
    }
    pub fn stop(&self) {
        unsafe { sys::rfa_dab_stop(self.h) }
    }
    pub fn push(&self, s: &[C32]) -> i32 {
        unsafe { sys::rfa_dab_push(self.h, s.as_ptr() as *const f32, s.len() as i32) }
    }
    pub fn select(&self, sid: u32) -> bool {
        unsafe { sys::rfa_dab_select_sid(self.h, sid) != 0 }
    }
    pub fn describe(&self, sid: u32) -> bool {
        unsafe { sys::rfa_dab_describe(self.h, sid) != 0 }
    }
    pub fn poll_events(&self) -> Option<String> {
        unsafe {
            let p = sys::rfa_dab_poll_events(self.h);
            if p.is_null() {
                return None;
            }
            let s = std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned();
            sys::rfa_dab_free(p as *mut _);
            Some(s)
        }
    }
    pub fn poll_audio(&self, out: &mut [i16]) -> usize {
        unsafe { sys::rfa_dab_poll_audio(self.h, out.as_mut_ptr(), out.len() as i32).max(0) as usize }
    }
    pub fn audio_rate(&self) -> i32 {
        unsafe { sys::rfa_dab_audio_rate(self.h) }
    }
    pub fn take_slide(&self) -> Option<(Vec<u8>, String, i32)> {
        unsafe {
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut size = 0;
            let mut typ = 0;
            let mut name = [0 as std::ffi::c_char; 256];
            if sys::rfa_dab_take_slide(self.h, &mut data, &mut size, &mut typ, name.as_mut_ptr(), 256) == 0 {
                return None;
            }
            let v = std::slice::from_raw_parts(data, size.max(0) as usize).to_vec();
            sys::rfa_dab_free(data as *mut _);
            let n = std::ffi::CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned();
            Some((v, n, typ))
        }
    }
    pub fn poll_constellation(&self, max: usize) -> Vec<[f32; 2]> {
        let mut buf = vec![0f32; max * 2];
        let k = unsafe { sys::rfa_dab_poll_constellation(self.h, buf.as_mut_ptr(), max as i32) }.max(0) as usize;
        buf.chunks_exact(2).take(k).map(|c| [c[0], c[1]]).collect()
    }
    pub fn dropped(&self) -> u32 {
        unsafe { sys::rfa_dab_dropped(self.h) }
    }
}

#[cfg(feature = "dab")]
impl Drop for Native {
    fn drop(&mut self) {
        unsafe { sys::rfa_dab_destroy(self.h) }
    }
}

#[cfg(not(feature = "dab"))]
impl Native {
    pub fn create() -> Option<Native> {
        None
    }
    pub fn start(&self) {}
    pub fn stop(&self) {}
    pub fn push(&self, _s: &[C32]) -> i32 {
        0
    }
    pub fn select(&self, _sid: u32) -> bool {
        false
    }
    pub fn describe(&self, _sid: u32) -> bool {
        false
    }
    pub fn poll_events(&self) -> Option<String> {
        None
    }
    pub fn poll_audio(&self, _out: &mut [i16]) -> usize {
        0
    }
    pub fn audio_rate(&self) -> i32 {
        48000
    }
    pub fn take_slide(&self) -> Option<(Vec<u8>, String, i32)> {
        None
    }
    pub fn poll_constellation(&self, _max: usize) -> Vec<[f32; 2]> {
        Vec::new()
    }
    pub fn dropped(&self) -> u32 {
        0
    }
}
