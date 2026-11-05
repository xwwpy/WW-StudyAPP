//! Best-effort user-enabled sound, never a system notification or focus change.
pub fn play() {
    #[cfg(all(windows, not(test)))]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBeep(kind: u32) -> i32;
        }
        unsafe {
            let _ = MessageBeep(0);
        }
    }
    #[cfg(all(target_os = "macos", not(test)))]
    {
        #[link(name = "AppKit", kind = "framework")]
        unsafe extern "C" {
            fn NSBeep();
        }
        unsafe {
            NSBeep();
        }
    }
    #[cfg(all(target_os = "linux", not(test)))]
    if let Some(display) = gtk::gdk::Display::default() {
        display.beep();
    }
}
