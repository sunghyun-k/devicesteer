//! The Mac's keyboard input source, for forwarding Control-Space (PITFALLS #46).
//!
//! Control-Space is the Mac's "Select the previous input source" shortcut. macOS takes it before
//! the window does, so the page sees Control go down and up and never the Space. What does reach
//! us is the input source changing: the shell watches for that and tells the page
//! (`input-source-changed`), which decides whether it was the shortcut pressed while forwarding.
//! If so, the page sends the device its Space and asks for the Mac's previous input source back
//! ([`restore`]), so only the device switches.

/// The page event sent when the Mac's input source changes.
pub const CHANGED: &str = "input-source-changed";

#[cfg(target_os = "macos")]
mod mac {
    use std::{
        ffi::c_void,
        sync::{Mutex, OnceLock},
    };

    use tauri::{AppHandle, Emitter};

    type CFTypeRef = *const c_void;
    type CFStringRef = CFTypeRef;
    type TISInputSourceRef = CFTypeRef;
    type Callback = extern "C" fn(CFTypeRef, *const c_void, CFStringRef, *const c_void, CFTypeRef);

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFNotificationCenterGetDistributedCenter() -> CFTypeRef;
        fn CFNotificationCenterAddObserver(
            center: CFTypeRef,
            observer: *const c_void,
            callback: Callback,
            name: CFStringRef,
            object: *const c_void,
            suspension: isize,
        );
        fn CFEqual(a: CFTypeRef, b: CFTypeRef) -> u8;
        fn CFRelease(cf: CFTypeRef);
    }

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        static kTISNotifySelectedKeyboardInputSourceChanged: CFStringRef;
        static kTISPropertyInputSourceID: CFStringRef;
        fn TISCopyCurrentKeyboardInputSource() -> TISInputSourceRef;
        fn TISSelectInputSource(source: TISInputSourceRef) -> i32;
        fn TISGetInputSourceProperty(source: TISInputSourceRef, key: CFStringRef) -> CFTypeRef;
    }

    /// `CFNotificationSuspensionBehaviorDeliverImmediately`: the change matters while the key is
    /// still held.
    const DELIVER_IMMEDIATELY: isize = 4;

    /// An input source we hold a reference to.
    struct Source(TISInputSourceRef);
    // Only touched on the main thread (the observer runs there; `restore` hops there), behind the
    // mutex.
    unsafe impl Send for Source {}
    impl Drop for Source {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) }
        }
    }
    impl Source {
        fn current() -> Option<Self> {
            let s = unsafe { TISCopyCurrentKeyboardInputSource() };
            (!s.is_null()).then_some(Self(s))
        }
        fn same(&self, other: &Self) -> bool {
            unsafe {
                let (a, b) = (
                    TISGetInputSourceProperty(self.0, kTISPropertyInputSourceID),
                    TISGetInputSourceProperty(other.0, kTISPropertyInputSourceID),
                );
                !a.is_null() && !b.is_null() && CFEqual(a, b) != 0
            }
        }
    }

    #[derive(Default)]
    struct State {
        current: Option<Source>,
        previous: Option<Source>,
        /// Selected by [`restore`]: its own change notification is not news for the page.
        restoring: Option<Source>,
    }

    static STATE: Mutex<State> = Mutex::new(State { current: None, previous: None, restoring: None });
    static APP: OnceLock<AppHandle> = OnceLock::new();

    extern "C" fn changed(_: CFTypeRef, _: *const c_void, _: CFStringRef, _: *const c_void, _: CFTypeRef) {
        let Some(now) = Source::current() else { return };
        let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if s.current.as_ref().is_some_and(|c| c.same(&now)) {
            return;
        }
        let ours = s.restoring.take().is_some_and(|r| r.same(&now));
        s.previous = s.current.replace(now);
        drop(s);
        if !ours && let Some(app) = APP.get() {
            let _ = app.emit(super::CHANGED, ());
        }
    }

    /// Starts watching. Call on the main thread (Tauri's `setup`), whose run loop delivers the
    /// notifications.
    pub fn watch(app: AppHandle) {
        if APP.set(app).is_err() {
            return;
        }
        STATE.lock().unwrap_or_else(|e| e.into_inner()).current = Source::current();
        unsafe {
            CFNotificationCenterAddObserver(
                CFNotificationCenterGetDistributedCenter(),
                std::ptr::null(),
                changed,
                kTISNotifySelectedKeyboardInputSourceChanged,
                std::ptr::null(),
                DELIVER_IMMEDIATELY,
            );
        }
    }

    pub fn restore(app: &AppHandle) {
        let _ = app.run_on_main_thread(|| {
            let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
            let Some(prev) = s.previous.take() else { return };
            if unsafe { TISSelectInputSource(prev.0) } == 0 {
                s.restoring = Some(prev);
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub use mac::watch;

#[cfg(not(target_os = "macos"))]
pub fn watch(_app: tauri::AppHandle) {}

/// Selects the Mac's input source from before the last change the page heard of. Nothing if
/// there was none, or on other platforms.
#[tauri::command]
pub fn restore_input_source(app: tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    mac::restore(&app);
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}
