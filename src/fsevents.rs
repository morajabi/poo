//! FSEvents: the macOS file-change journal. Replaying it ("which folders changed since
//! event N?") lets a rescan re-measure only what changed, even across reboots.

#[cfg(target_os = "macos")]
mod imp {
    #![allow(non_upper_case_globals, non_snake_case, non_camel_case_types)]
    use std::ffi::{c_char, c_void, CStr, CString};
    use std::path::PathBuf;
    use std::sync::mpsc::Sender;
    use std::time::{Duration, Instant};

    type CFRef = *const c_void;
    type FSEventStreamRef = *mut c_void;

    #[repr(C)]
    struct FSEventStreamContext {
        version: isize,
        info: *mut c_void,
        retain: *const c_void,
        release: *const c_void,
        copy_description: *const c_void,
    }

    type Callback = extern "C" fn(FSEventStreamRef, *mut c_void, usize, *mut c_void, *const u32, *const u64);

    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn FSEventStreamCreate(alloc: CFRef, cb: Callback, ctx: *const FSEventStreamContext, paths: CFRef, since: u64, latency: f64, flags: u32) -> FSEventStreamRef;
        fn FSEventStreamScheduleWithRunLoop(s: FSEventStreamRef, rl: CFRef, mode: CFRef);
        fn FSEventStreamStart(s: FSEventStreamRef) -> u8;
        fn FSEventStreamStop(s: FSEventStreamRef);
        fn FSEventStreamInvalidate(s: FSEventStreamRef);
        fn FSEventStreamRelease(s: FSEventStreamRef);
        fn FSEventsGetCurrentEventId() -> u64;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopDefaultMode: CFRef;
        static kCFTypeArrayCallBacks: c_void;
        fn CFStringCreateWithCString(alloc: CFRef, s: *const c_char, enc: u32) -> CFRef;
        fn CFArrayCreate(alloc: CFRef, values: *const CFRef, n: isize, cb: *const c_void) -> CFRef;
        fn CFRunLoopGetCurrent() -> CFRef;
        fn CFRunLoopRunInMode(mode: CFRef, seconds: f64, return_after: u8) -> i32;
        fn CFRunLoopStop(rl: CFRef);
        fn CFRelease(cf: CFRef);
    }

    const UTF8: u32 = 0x0800_0100;
    const NO_DEFER: u32 = 0x02;
    const SINCE_NOW: u64 = u64::MAX;
    const MUST_SCAN_SUBDIRS: u32 = 0x01;
    const USER_DROPPED: u32 = 0x02;
    const KERNEL_DROPPED: u32 = 0x04;
    const IDS_WRAPPED: u32 = 0x08;
    const HISTORY_DONE: u32 = 0x10;
    const ROOT_CHANGED: u32 = 0x20;

    pub fn current_id() -> u64 {
        unsafe { FSEventsGetCurrentEventId() }
    }

    struct State {
        paths: Vec<PathBuf>,
        rescan_dirs: Vec<PathBuf>,
        lost: bool,
        done: bool,
        sender: Option<Sender<Vec<PathBuf>>>,
    }

    extern "C" fn callback(_s: FSEventStreamRef, info: *mut c_void, n: usize, paths: *mut c_void, flags: *const u32, _ids: *const u64) {
        let st = unsafe { &mut *(info as *mut State) };
        let paths = paths as *const *const c_char;
        let mut batch = Vec::with_capacity(n);
        for i in 0..n {
            let f = unsafe { *flags.add(i) };
            let p = unsafe { CStr::from_ptr(*paths.add(i)) }.to_string_lossy().to_string();
            let p = PathBuf::from(p.trim_end_matches('/'));
            if f & HISTORY_DONE != 0 {
                st.done = true;
                unsafe { CFRunLoopStop(CFRunLoopGetCurrent()) };
                continue;
            }
            if f & (USER_DROPPED | KERNEL_DROPPED | IDS_WRAPPED | ROOT_CHANGED) != 0 {
                st.lost = true;
            }
            if f & MUST_SCAN_SUBDIRS != 0 {
                st.rescan_dirs.push(p.clone());
            }
            batch.push(p);
        }
        if let Some(tx) = &st.sender {
            if !batch.is_empty() {
                let _ = tx.send(batch);
            }
        } else {
            st.paths.extend(batch);
        }
    }

    fn make_stream(roots: &[PathBuf], since: u64, latency: f64, st: *mut State) -> Option<FSEventStreamRef> {
        unsafe {
            let cstrs: Vec<CString> = roots.iter().filter_map(|r| CString::new(r.to_string_lossy().as_bytes()).ok()).collect();
            let cfs: Vec<CFRef> = cstrs.iter().map(|c| CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8)).collect();
            let arr = CFArrayCreate(std::ptr::null(), cfs.as_ptr(), cfs.len() as isize, &kCFTypeArrayCallBacks as *const c_void);
            for c in &cfs {
                CFRelease(*c);
            }
            let ctx = FSEventStreamContext {
                version: 0,
                info: st as *mut c_void,
                retain: std::ptr::null(),
                release: std::ptr::null(),
                copy_description: std::ptr::null(),
            };
            let s = FSEventStreamCreate(std::ptr::null(), callback, &ctx, arr, since, latency, NO_DEFER);
            CFRelease(arr);
            (!s.is_null()).then_some(s)
        }
    }

    pub struct Changes {
        /// Folders whose contents changed.
        pub paths: Vec<PathBuf>,
        /// Folders the journal says to rescan entirely.
        pub rescan_dirs: Vec<PathBuf>,
        /// The journal lost events (dropped, wrapped, root moved): do a full scan.
        pub lost: bool,
    }

    /// Changed folders under `roots` since event `since`. None when the replay does not
    /// finish in time.
    pub fn changes_since(roots: &[PathBuf], since: u64, timeout: Duration) -> Option<Changes> {
        let mut st = State { paths: vec![], rescan_dirs: vec![], lost: false, done: false, sender: None };
        let s = make_stream(roots, since, 0.0, &mut st as *mut State)?;
        unsafe {
            FSEventStreamScheduleWithRunLoop(s, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
            FSEventStreamStart(s);
            let start = Instant::now();
            while !st.done && start.elapsed() < timeout {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.25, 0);
            }
            FSEventStreamStop(s);
            FSEventStreamInvalidate(s);
            FSEventStreamRelease(s);
        }
        if !st.done {
            return None;
        }
        st.paths.sort();
        st.paths.dedup();
        Some(Changes { paths: st.paths, rescan_dirs: st.rescan_dirs, lost: st.lost })
    }

    /// Streams batches of changed folders forever (until the process exits).
    pub fn watch(roots: &[PathBuf], latency: f64, tx: Sender<Vec<PathBuf>>) -> bool {
        let st = Box::into_raw(Box::new(State { paths: vec![], rescan_dirs: vec![], lost: false, done: false, sender: Some(tx) }));
        let Some(s) = make_stream(roots, SINCE_NOW, latency, st) else { return false };
        unsafe {
            FSEventStreamScheduleWithRunLoop(s, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);
            FSEventStreamStart(s);
            loop {
                CFRunLoopRunInMode(kCFRunLoopDefaultMode, 60.0, 0);
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub use imp::{changes_since, current_id, watch, Changes};

#[cfg(not(target_os = "macos"))]
pub struct Changes {
    pub paths: Vec<std::path::PathBuf>,
    pub rescan_dirs: Vec<std::path::PathBuf>,
    pub lost: bool,
}

#[cfg(not(target_os = "macos"))]
pub fn current_id() -> u64 {
    0
}

#[cfg(not(target_os = "macos"))]
pub fn changes_since(_: &[std::path::PathBuf], _: u64, _: std::time::Duration) -> Option<Changes> {
    None
}

#[cfg(not(target_os = "macos"))]
pub fn watch(_: &[std::path::PathBuf], _: f64, _: std::sync::mpsc::Sender<Vec<std::path::PathBuf>>) -> bool {
    false
}
