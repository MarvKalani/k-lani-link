//! The notification-area icon (Windows) and menu-bar item (macOS): open the panel, pause or
//! resume the connection, quit. Its colour shows the connection state; the page uses the
//! same square as its icon. The menu logic is portable and tested; only the Win32 and
//! AppKit glue below is platform-specific.
// Elsewhere only the tests use the portable part.
#![cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]

use super::{Panel, State, connect, open_browser, snapshot, stop_client};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Open,
    Pause,
    Resume,
    Quit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Light {
    #[default]
    Idle,
    Waiting,
    Connected,
    Problem,
}

/// What the icon and its menu show, taken from the panel's status.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct View {
    pub light: Light,
    pub phase: Phase,
    /// Pause is offered for a pairing by code only: a WebRTC tunnel that ends needs a new
    /// code on both computers, so pausing it would cost the pairing.
    pub pausable: bool,
    pub running: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Phase {
    #[default]
    Unpaired,
    Pairing,
    Connecting,
    Connected,
    Paused,
    Problem,
}

impl View {
    pub fn from_status(status: &Value) -> Self {
        let running = status["running"] == true;
        let https = status["transport"] == "https";
        let (phase, light) = match status["phase"].as_str().unwrap_or_default() {
            "connected" => (Phase::Connected, Light::Connected),
            "joining" => (Phase::Pairing, Light::Waiting),
            "connecting" | "recovering" => (Phase::Connecting, Light::Waiting),
            "not_configured" => (Phase::Unpaired, Light::Idle),
            "disconnected" => (Phase::Paused, Light::Idle),
            _ => (Phase::Problem, Light::Problem),
        };
        View {
            light,
            phase,
            pausable: https && status["configured"] == true && status["joining"] != true,
            running,
        }
    }

    /// The menu entries: an id, the text and whether it can be chosen.
    pub fn menu(&self, german: bool) -> Vec<(Action, &'static str)> {
        let text = |de, en| if german { de } else { en };
        let mut items = vec![(
            Action::Open,
            text("Kontrollfeld öffnen", "Open control panel"),
        )];
        if self.pausable {
            items.push(if self.running {
                (
                    Action::Pause,
                    text("Verbindung pausieren", "Pause connection"),
                )
            } else {
                (
                    Action::Resume,
                    text("Verbindung fortsetzen", "Resume connection"),
                )
            });
        }
        items.push((
            Action::Quit,
            text("k-lani-link beenden", "Quit k-lani-link"),
        ));
        items
    }

    pub fn tooltip(&self, german: bool) -> String {
        let state = match (self.phase, german) {
            (Phase::Unpaired, true) => "nicht gekoppelt",
            (Phase::Unpaired, false) => "not paired",
            (Phase::Pairing, true) => "koppelt…",
            (Phase::Pairing, false) => "pairing…",
            (Phase::Connecting, true) => "verbindet…",
            (Phase::Connecting, false) => "connecting…",
            (Phase::Connected, true) => "verbunden",
            (Phase::Connected, false) => "connected",
            (Phase::Paused, true) => "pausiert",
            (Phase::Paused, false) => "paused",
            (Phase::Problem, true) => "nicht verbunden",
            (Phase::Problem, false) => "not connected",
        };
        format!("k-lani-link: {state}")
    }
}

/// Carries out a menu choice on the running panel.
pub(super) async fn act(panel: &Panel, address: &str, action: Action) {
    match action {
        Action::Open => {
            if let Ok(mut browser) = open_browser(address) {
                std::thread::spawn(move || {
                    let _ = browser.wait();
                });
            }
        }
        Action::Pause => {
            if View::from_status(&snapshot(panel).await).pausable {
                stop_client(panel).await;
            }
        }
        Action::Resume => {
            let _ = connect(State(panel.clone())).await;
        }
        Action::Quit => panel.stop.cancel(),
    }
}

/// Keeps `view` in step with the panel (calling `changed` after each change) and carries
/// out the actions sent to the returned channel, until the panel stops.
pub(super) fn follow(
    panel: Panel,
    address: String,
    view: std::sync::Arc<std::sync::Mutex<View>>,
    changed: impl Fn() + Send + 'static,
) -> tokio::sync::mpsc::UnboundedSender<Action> {
    let (actions, mut chosen) = tokio::sync::mpsc::unbounded_channel();
    let worker = panel.clone();
    tokio::spawn(async move {
        while let Some(action) = chosen.recv().await {
            act(&worker, &address, action).await;
        }
    });
    tokio::spawn(async move {
        loop {
            let next = View::from_status(&snapshot(&panel).await);
            let previous = {
                let mut current = view.lock().unwrap_or_else(|e| e.into_inner());
                std::mem::replace(&mut *current, next)
            };
            if previous != next {
                changed();
            }
            tokio::select! {
                _ = panel.stop.cancelled() => break,
                _ = tokio::time::sleep(std::time::Duration::from_millis(1500)) => {}
            }
        }
    });
    actions
}

/// A PNG of the square icon (stored, uncompressed), for the page and the macOS menu bar.
pub(super) fn png(size: usize, light: Light) -> Vec<u8> {
    fn crc(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    0xedb8_8320 ^ (crc >> 1)
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        let start = out.len();
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        let sum = crc(&out[start..]);
        out.extend_from_slice(&sum.to_be_bytes());
    }
    let mut raw = Vec::with_capacity(size * (size * 4 + 1));
    for row in pixels(size, light).chunks(size) {
        raw.push(0);
        for pixel in row {
            let [a, r, g, b] = pixel.to_be_bytes();
            raw.extend_from_slice(&[r, g, b, a]);
        }
    }
    // zlib with stored deflate blocks of at most 65535 bytes, then the Adler-32 sum.
    let mut zlib = vec![0x78, 0x01];
    let blocks = raw.chunks(65535).count();
    for (index, block) in raw.chunks(65535).enumerate() {
        zlib.push(u8::from(index + 1 == blocks));
        let length = block.len() as u16;
        zlib.extend_from_slice(&length.to_le_bytes());
        zlib.extend_from_slice(&(!length).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for byte in &raw {
        a = (a + u32::from(*byte)) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend_from_slice(&((b << 16) | a).to_be_bytes());
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(size as u32).to_be_bytes());
    header.extend_from_slice(&(size as u32).to_be_bytes());
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// The square icon in the k-lani colours: a paper frame around night, with a centre
/// square in the state colour. Pixels are BGRA, top row first.
pub(super) fn pixels(size: usize, light: Light) -> Vec<u32> {
    const PAPER: u32 = 0xfffd_f8ec;
    const NIGHT: u32 = 0xff0a_0a18;
    let centre = match light {
        Light::Idle => 0xff6b_6b8f,
        Light::Waiting => 0xffff_e500,
        Light::Connected => 0xff22_e6ff,
        Light::Problem => 0xffff_2e88,
    };
    let frame = (size / 8).max(1);
    let inset = size / 4;
    (0..size * size)
        .map(|i| {
            let (x, y) = (i % size, i / size);
            let edge = x.min(y).min(size - 1 - x).min(size - 1 - y);
            if edge < frame {
                PAPER
            } else if edge >= inset {
                centre
            } else {
                NIGHT
            }
        })
        .collect()
}

#[cfg(windows)]
pub(super) use win::start;

#[cfg(windows)]
mod win {
    use super::{Action, Light, View, follow, pixels};
    use crate::ui::Panel;
    use std::cell::RefCell;
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };
    use windows_sys::Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        Globalization::GetUserDefaultUILanguage,
        Graphics::Gdi::{CreateBitmap, DeleteObject},
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Shell::{
                NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
                Shell_NotifyIconW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreateIconIndirect, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
                DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos,
                GetMessageW, GetSystemMetrics, HICON, ICONINFO, MF_SEPARATOR, MF_STRING, MSG,
                PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SM_CXSMICON,
                SetForegroundWindow, SetMenuDefaultItem, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON,
                TrackPopupMenu, TranslateMessage, WM_APP, WM_CLOSE, WM_COMMAND, WM_CONTEXTMENU,
                WM_DESTROY, WM_LBUTTONUP, WM_NULL, WM_RBUTTONUP, WNDCLASSW,
            },
        },
    };

    const CALLBACK: u32 = WM_APP + 1;
    const REFRESH: u32 = WM_APP + 2;

    /// The icon's thread and window; `close` removes the icon and waits for the thread.
    pub struct Tray {
        window: Arc<AtomicUsize>,
        closing: Arc<AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    }

    impl Tray {
        pub async fn close(mut self) {
            self.closing.store(true, Ordering::SeqCst);
            post(&self.window, WM_CLOSE);
            if let Some(thread) = self.thread.take() {
                let _ = tokio::task::spawn_blocking(move || thread.join()).await;
            }
        }
    }

    fn post(window: &AtomicUsize, message: u32) {
        let hwnd = window.load(Ordering::SeqCst);
        if hwnd != 0 {
            unsafe { PostMessageW(hwnd as HWND, message, 0, 0) };
        }
    }

    struct Shared {
        view: Arc<Mutex<View>>,
        actions: tokio::sync::mpsc::UnboundedSender<Action>,
        german: bool,
        icon: HICON,
        taskbar_created: u32,
    }

    thread_local! {
        static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) };
    }

    /// Starts the icon on a thread of its own and keeps it in step with the panel.
    pub fn start(panel: Panel, address: String) -> Option<Tray> {
        let view = Arc::new(Mutex::new(View::default()));
        let window = Arc::new(AtomicUsize::new(0));
        let closing = Arc::new(AtomicBool::new(false));
        let refresh = window.clone();
        let actions = follow(panel, address, view.clone(), move || {
            post(&refresh, REFRESH)
        });
        let thread = {
            let window = window.clone();
            let closing = closing.clone();
            std::thread::Builder::new()
                .name("tray".into())
                .spawn(move || run(view, window, closing, actions))
                .ok()?
        };
        Some(Tray {
            window,
            closing,
            thread: Some(thread),
        })
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(Some(0)).collect()
    }

    fn run(
        view: Arc<Mutex<View>>,
        window: Arc<AtomicUsize>,
        closing: Arc<AtomicBool>,
        actions: tokio::sync::mpsc::UnboundedSender<Action>,
    ) {
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = wide("k-lani-link-tray");
            let mut definition: WNDCLASSW = std::mem::zeroed();
            definition.lpfnWndProc = Some(procedure);
            definition.hInstance = instance;
            definition.lpszClassName = class.as_ptr();
            RegisterClassW(&definition);
            // A hidden top-level window (not message-only) also receives TaskbarCreated,
            // so the icon comes back after Explorer restarts.
            let hwnd = CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return;
            }
            let german = GetUserDefaultUILanguage() & 0x3ff == 0x07;
            SHARED.with(|shared| {
                *shared.borrow_mut() = Some(Shared {
                    view,
                    actions,
                    german,
                    icon: std::ptr::null_mut(),
                    taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
                })
            });
            show(hwnd, NIM_ADD);
            window.store(hwnd as usize, Ordering::SeqCst);
            if closing.load(Ordering::SeqCst) {
                DestroyWindow(hwnd);
            }
            let mut message: MSG = std::mem::zeroed();
            while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            window.store(0, Ordering::SeqCst);
        }
    }

    fn icon(light: Light) -> HICON {
        unsafe {
            let size = GetSystemMetrics(SM_CXSMICON).clamp(16, 64) as usize;
            let color = pixels(size, light);
            let mask = vec![0u8; size.div_ceil(16) * 2 * size];
            let color_map = CreateBitmap(size as i32, size as i32, 1, 32, color.as_ptr().cast());
            let mask_map = CreateBitmap(size as i32, size as i32, 1, 1, mask.as_ptr().cast());
            let info = ICONINFO {
                fIcon: 1,
                xHotspot: 0,
                yHotspot: 0,
                hbmMask: mask_map,
                hbmColor: color_map,
            };
            let icon = CreateIconIndirect(&info);
            DeleteObject(color_map);
            DeleteObject(mask_map);
            icon
        }
    }

    /// Adds or updates the icon from the current view.
    unsafe fn show(hwnd: HWND, operation: u32) {
        SHARED.with(|shared| {
            let mut shared = shared.borrow_mut();
            let Some(shared) = shared.as_mut() else {
                return;
            };
            let view = *shared.view.lock().unwrap_or_else(|e| e.into_inner());
            let fresh = icon(view.light);
            let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
            data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
            data.hWnd = hwnd;
            data.uID = 1;
            data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
            data.uCallbackMessage = CALLBACK;
            data.hIcon = fresh;
            let tip: Vec<u16> = view
                .tooltip(shared.german)
                .encode_utf16()
                .take(127)
                .collect();
            data.szTip[..tip.len()].copy_from_slice(&tip);
            unsafe { Shell_NotifyIconW(operation, &data) };
            if !shared.icon.is_null() {
                unsafe { DestroyIcon(shared.icon) };
            }
            shared.icon = fresh;
        });
    }

    unsafe fn menu(hwnd: HWND) {
        let items = SHARED
            .with(|shared| {
                let shared = shared.borrow();
                let shared = shared.as_ref()?;
                let view = *shared.view.lock().unwrap_or_else(|e| e.into_inner());
                Some(view.menu(shared.german))
            })
            .unwrap_or_default();
        unsafe {
            let popup = CreatePopupMenu();
            for (index, (action, text)) in items.iter().enumerate() {
                if *action == Action::Quit && index > 0 {
                    AppendMenuW(popup, MF_SEPARATOR, 0, std::ptr::null());
                }
                AppendMenuW(popup, MF_STRING, *action as usize + 1, wide(text).as_ptr());
            }
            SetMenuDefaultItem(popup, Action::Open as u32 + 1, 0);
            let mut point = POINT { x: 0, y: 0 };
            GetCursorPos(&mut point);
            // Without the foreground switch the menu stays open after a click elsewhere.
            SetForegroundWindow(hwnd);
            TrackPopupMenu(
                popup,
                TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                point.x,
                point.y,
                0,
                hwnd,
                std::ptr::null(),
            );
            PostMessageW(hwnd, WM_NULL, 0, 0);
            DestroyMenu(popup);
        }
    }

    fn choose(action: Action) {
        SHARED.with(|shared| {
            if let Some(shared) = shared.borrow().as_ref() {
                let _ = shared.actions.send(action);
            }
        });
    }

    unsafe extern "system" fn procedure(
        hwnd: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let recreated = SHARED.with(|shared| {
            shared
                .borrow()
                .as_ref()
                .is_some_and(|shared| shared.taskbar_created == message)
        });
        unsafe {
            match message {
                _ if recreated => show(hwnd, NIM_ADD),
                CALLBACK => match lparam as u32 {
                    WM_LBUTTONUP => choose(Action::Open),
                    WM_RBUTTONUP | WM_CONTEXTMENU => menu(hwnd),
                    _ => {}
                },
                REFRESH => show(hwnd, NIM_MODIFY),
                WM_COMMAND => match (wparam & 0xffff) as i32 - 1 {
                    0 => choose(Action::Open),
                    1 => choose(Action::Pause),
                    2 => choose(Action::Resume),
                    3 => choose(Action::Quit),
                    _ => {}
                },
                WM_CLOSE => {
                    DestroyWindow(hwnd);
                }
                WM_DESTROY => {
                    let mut data: NOTIFYICONDATAW = std::mem::zeroed();
                    data.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
                    data.hWnd = hwnd;
                    data.uID = 1;
                    Shell_NotifyIconW(NIM_DELETE, &data);
                    SHARED.with(|shared| {
                        if let Some(shared) = shared.borrow_mut().take()
                            && !shared.icon.is_null()
                        {
                            DestroyIcon(shared.icon);
                        }
                    });
                    PostQuitMessage(0);
                }
                _ => return DefWindowProcW(hwnd, message, wparam, lparam),
            }
        }
        0
    }
}

#[cfg(target_os = "macos")]
pub(super) use mac::{main_thread, start};

/// macOS: AppKit runs on the main thread, so the panel's runtime moves to a thread of its
/// own and the main thread runs the application loop with a menu-bar item.
#[cfg(target_os = "macos")]
mod mac {
    use super::{Action, View, follow, png};
    use crate::ui::Panel;
    use objc2::{
        AnyThread, MainThreadMarker, MainThreadOnly, define_class, msg_send, rc::Retained, sel,
    };
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSImage, NSMenu, NSMenuItem, NSStatusBar,
        NSStatusItem,
    };
    use objc2_foundation::{NSData, NSLocale, NSObject, NSSize, NSString, NSTimer};
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex, OnceLock};

    struct Bridge {
        view: Arc<Mutex<View>>,
        actions: tokio::sync::mpsc::UnboundedSender<Action>,
    }

    static BRIDGE: Mutex<Option<Bridge>> = Mutex::new(None);
    static DONE: OnceLock<bool> = OnceLock::new();

    struct Shown {
        item: Retained<NSStatusItem>,
        menu: Retained<NSMenu>,
        view: Option<View>,
    }

    thread_local! {
        static SHOWN: RefCell<Option<Shown>> = const { RefCell::new(None) };
    }

    define_class!(
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        #[name = "KLaniLinkMenuBar"]
        struct Handler;

        impl Handler {
            #[unsafe(method(choose:))]
            fn choose(&self, sender: &NSMenuItem) {
                let action = match sender.tag() {
                    0 => Action::Open,
                    1 => Action::Pause,
                    2 => Action::Resume,
                    _ => Action::Quit,
                };
                if let Some(bridge) = BRIDGE.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                    let _ = bridge.actions.send(action);
                }
            }

            #[unsafe(method(tick:))]
            fn tick(&self, _timer: &NSTimer) {
                tick(self);
            }
        }
    );

    impl Handler {
        fn new(mtm: MainThreadMarker) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(());
            unsafe { msg_send![super(this), init] }
        }
    }

    /// Ties the menu-bar item to the panel until `close`.
    pub struct Tray;

    impl Tray {
        pub async fn close(self) {
            *BRIDGE.lock().unwrap_or_else(|e| e.into_inner()) = None;
        }
    }

    pub fn start(panel: Panel, address: String) -> Option<Tray> {
        let view = Arc::new(Mutex::new(View::default()));
        let actions = follow(panel, address, view.clone(), || {});
        *BRIDGE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Bridge { view, actions });
        Some(Tray)
    }

    /// Runs `work` on another thread while the main thread shows the menu-bar item; exits
    /// the process with `work`'s outcome once it returns.
    pub fn main_thread(work: impl FnOnce() -> bool + Send + 'static) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return work();
        };
        let started = std::thread::Builder::new()
            .name("panel".into())
            .spawn(move || {
                let _ = DONE.set(work());
            });
        if started.is_err() {
            return false;
        }
        let app = NSApplication::sharedApplication(mtm);
        // A menu-bar item only: no Dock icon and no menu bar of its own.
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
        let handler = Handler::new(mtm);
        unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.3,
                &handler,
                sel!(tick:),
                None,
                true,
            );
        }
        app.run();
        DONE.get().copied().unwrap_or(false)
    }

    fn german() -> bool {
        NSLocale::preferredLanguages()
            .firstObject()
            .is_some_and(|language| language.to_string().starts_with("de"))
    }

    fn tick(handler: &Handler) {
        if let Some(ok) = DONE.get() {
            std::process::exit(if *ok { 0 } else { 1 });
        }
        let view = BRIDGE
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|bridge| *bridge.view.lock().unwrap_or_else(|e| e.into_inner()));
        let mtm = handler.mtm();
        SHOWN.with(|shown| {
            let mut shown = shown.borrow_mut();
            let Some(view) = view else {
                if let Some(old) = shown.take() {
                    NSStatusBar::systemStatusBar().removeStatusItem(&old.item);
                }
                return;
            };
            let current = shown.get_or_insert_with(|| {
                // -2 is NSSquareStatusItemLength: as wide as the menu bar is high.
                let item = NSStatusBar::systemStatusBar().statusItemWithLength(-2.0);
                let menu = NSMenu::new(mtm);
                item.setMenu(Some(&menu));
                Shown {
                    item,
                    menu,
                    view: None,
                }
            });
            if current.view == Some(view) {
                return;
            }
            current.view = Some(view);
            let german = german();
            if let Some(button) = current.item.button(mtm) {
                let data = NSData::with_bytes(&png(36, view.light));
                if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
                    image.setSize(NSSize::new(18.0, 18.0));
                    button.setImage(Some(&image));
                }
                button.setToolTip(Some(&NSString::from_str(&view.tooltip(german))));
            }
            current.menu.removeAllItems();
            for (action, text) in view.menu(german) {
                if action == Action::Quit {
                    current.menu.addItem(&NSMenuItem::separatorItem(mtm));
                }
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(text),
                        Some(sel!(choose:)),
                        &NSString::from_str(""),
                    )
                };
                unsafe { item.setTarget(Some(handler)) };
                item.setTag(action as isize);
                current.menu.addItem(&item);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn pause_is_offered_for_a_running_pairing_by_code_only() {
        let https = View::from_status(&json!({"phase": "connected", "running": true,
            "configured": true, "transport": "https", "joining": false}));
        assert_eq!(https.light, Light::Connected);
        let actions: Vec<_> = https.menu(false).iter().map(|item| item.0).collect();
        assert_eq!(actions, [Action::Open, Action::Pause, Action::Quit]);

        let paused = View::from_status(&json!({"phase": "disconnected", "running": false,
            "configured": true, "transport": "https", "joining": false}));
        assert_eq!(paused.phase, Phase::Paused);
        assert_eq!(
            paused.menu(true)[1],
            (Action::Resume, "Verbindung fortsetzen")
        );
        assert_eq!(paused.tooltip(true), "k-lani-link: pausiert");

        // Ending a WebRTC tunnel would cost the pairing code, so it is never paused here.
        let webrtc = View::from_status(&json!({"phase": "connected", "running": true,
            "configured": true, "transport": "webrtc", "joining": false}));
        let actions: Vec<_> = webrtc.menu(false).iter().map(|item| item.0).collect();
        assert_eq!(actions, [Action::Open, Action::Quit]);

        let unpaired = View::from_status(&json!({"phase": "not_configured", "running": false,
            "configured": false, "joining": false}));
        assert_eq!(unpaired.menu(false).len(), 2);
        assert_eq!(unpaired.tooltip(false), "k-lani-link: not paired");
    }

    #[test]
    fn the_icon_png_is_well_formed() {
        let image = png(36, Light::Connected);
        assert_eq!(&image[..8], b"\x89PNG\r\n\x1a\n");
        // Signature, IHDR, the IDAT header and the end chunk around 36 rows of 1 + 144 bytes.
        assert_eq!(image.len(), 8 + 25 + 12 + 2 + 5 + 36 * 145 + 4 + 12);
        assert_eq!(&image[image.len() - 8..], b"IEND\xae\x42\x60\x82");
    }

    #[test]
    fn the_icon_has_a_frame_and_a_centre_in_the_state_colour() {
        let icon = pixels(16, Light::Problem);
        assert_eq!(icon.len(), 256);
        assert_eq!(icon[0], 0xfffd_f8ec);
        assert_eq!(icon[16 * 3 + 3], 0xff0a_0a18);
        assert_eq!(icon[16 * 8 + 8], 0xffff_2e88);
    }
}
