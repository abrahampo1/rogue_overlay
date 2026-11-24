//! Everything that touches Win32: transparent layered window, keyboard/mouse hooks,
//! presenting with UpdateLayeredWindow, and screen capture for the mana %.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::*;

// ---------------------------------------------------------------- input

#[derive(Clone, Copy, Debug)]
pub enum Input {
    KeyDown { vk: u32, focused: bool },
    KeyUp { vk: u32 },
    Click { focused: bool },
}

// The hooks run on their own thread (see `spawn_hooks`), so this state is shared across threads.
static EVENTS: Mutex<Vec<Input>> = Mutex::new(Vec::new());
static GAME_TITLE: Mutex<Vec<u16>> = Mutex::new(Vec::new());
static EDITING: AtomicBool = AtomicBool::new(false);
static HOOK_THREAD: AtomicU32 = AtomicU32::new(0);
/// Cursor shown over the overlay in edit mode (see `set_cursor`).
static CURSOR: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CursorKind {
    Move,
    ResizeH,
    ResizeV,
    ResizeDiagDown,
    ResizeDiagUp,
}

pub fn set_cursor(kind: CursorKind) {
    CURSOR.store(kind as u8, Ordering::Relaxed);
}
/// Signalled by the hook thread once its hooks are uninstalled.
static HOOKS_DONE: Mutex<Option<std::sync::mpsc::Receiver<()>>> = Mutex::new(None);

/// Keys edit mode keeps for itself (arrows, PageUp/Down, Esc, Enter, R, Z) so they do not reach
/// the game.
const EDIT_KEYS: [u32; 10] = [0x25, 0x26, 0x27, 0x28, 0x21, 0x22, 0x1B, 0x0D, 0x52, 0x5A];

pub fn set_game_title(title: &str) {
    if let Ok(mut t) = GAME_TITLE.lock() {
        *t = title.encode_utf16().collect();
    }
}

pub fn take_events() -> Vec<Input> {
    EVENTS.lock().map(|mut e| std::mem::take(&mut *e)).unwrap_or_default()
}

fn push(ev: Input) {
    if let Ok(mut v) = EVENTS.lock() {
        v.push(ev);
    }
}

/// Is the foreground window the game?
pub fn game_focused() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return false;
        }
        let mut buf = [0u16; 64];
        let n = GetWindowTextW(fg, &mut buf).max(0) as usize;
        GAME_TITLE.lock().map(|t| buf[..n] == t[..]).unwrap_or(false)
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 {
        let k = &*(lp.0 as *const KBDLLHOOKSTRUCT);
        match wp.0 as u32 {
            WM_KEYDOWN | WM_SYSKEYDOWN => push(Input::KeyDown { vk: k.vkCode, focused: game_focused() }),
            WM_KEYUP | WM_SYSKEYUP => push(Input::KeyUp { vk: k.vkCode }),
            _ => {}
        }
        if EDITING.load(Ordering::Relaxed) && EDIT_KEYS.contains(&k.vkCode) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wp, lp)
}

unsafe extern "system" fn mouse_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code >= 0 && wp.0 as u32 == WM_LBUTTONDOWN {
        push(Input::Click { focused: game_focused() });
    }
    CallNextHookEx(None, code, wp, lp)
}

/// Global low-level hooks. They only observe (except edit keys while editing) and never inject input.
///
/// They live on a dedicated thread that does nothing else: Windows makes every mouse move and key
/// press in the whole system wait until the hook thread handles it, so it must never be busy
/// drawing or capturing the screen (that showed up as input lag in game).
pub fn spawn_hooks() -> windows::core::Result<()> {
    let (tx, rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    if let Ok(mut d) = HOOKS_DONE.lock() {
        *d = Some(done_rx);
    }
    std::thread::spawn(move || unsafe {
        let hooks = (|| -> windows::core::Result<[HHOOK; 2]> {
            let hmod = GetModuleHandleW(None)?;
            let kb = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), hmod, 0)?;
            let ms = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), hmod, 0)?;
            Ok([kb, ms])
        })();
        let hooks = match hooks {
            Ok(h) => {
                HOOK_THREAD.store(windows::Win32::System::Threading::GetCurrentThreadId(), Ordering::SeqCst);
                let _ = tx.send(Ok(()));
                h
            }
            Err(e) => {
                let _ = tx.send(Err(e));
                return;
            }
        };
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
        for h in hooks {
            let _ = UnhookWindowsHookEx(h);
        }
        let _ = done_tx.send(());
    });
    rx.recv().unwrap_or(Ok(()))
}

/// Uninstalls the hooks and waits (up to 1 s) until they are gone. Exiting the process with the
/// hooks still registered can stall keyboard/mouse input system-wide (the game included) until
/// Windows times them out.
pub fn stop_hooks() {
    let id = HOOK_THREAD.swap(0, Ordering::SeqCst);
    if id != 0 {
        unsafe {
            let _ = PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        if let Some(rx) = HOOKS_DONE.lock().ok().and_then(|mut d| d.take()) {
            let _ = rx.recv_timeout(std::time::Duration::from_secs(1));
        }
    }
}

/// Key name → virtual key code.
pub fn parse_key(name: &str) -> Option<u32> {
    let n = name.trim().to_ascii_uppercase();
    let b = n.as_bytes();
    if b.len() == 1 && (b[0].is_ascii_digit() || b[0].is_ascii_uppercase()) {
        return Some(b[0] as u32);
    }
    if let Some(num) = n.strip_prefix('F').and_then(|s| s.parse::<u32>().ok()) {
        if (1..=24).contains(&num) {
            return Some(0x70 + num - 1);
        }
    }
    if let Some(num) = n.strip_prefix("NUMPAD").and_then(|s| s.parse::<u32>().ok()) {
        if num <= 9 {
            return Some(0x60 + num);
        }
    }
    Some(match n.as_str() {
        "-" | "MINUS" => 0xBD,
        "=" | "PLUS" | "EQUALS" => 0xBB,
        "," => 0xBC,
        "." => 0xBE,
        "SPACE" => 0x20,
        "TAB" => 0x09,
        "ESC" | "ESCAPE" => 0x1B,
        "PAGEUP" => 0x21,
        "PAGEDOWN" => 0x22,
        "END" => 0x23,
        "HOME" => 0x24,
        "INSERT" => 0x2D,
        "DELETE" => 0x2E,
        "SHIFT" => 0x10,
        "CTRL" | "CONTROL" => 0x11,
        "ALT" => 0x12,
        _ => return None,
    })
}

// ---------------------------------------------------------------- game window

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GameRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// Client area of the game window in screen coordinates.
pub fn find_game(title: &str) -> Option<GameRect> {
    let wide: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let hwnd = FindWindowW(None, PCWSTR(wide.as_ptr())).ok()?;
        if hwnd.is_invalid() || IsIconic(hwnd).as_bool() || !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        let mut rc = RECT::default();
        GetClientRect(hwnd, &mut rc).ok()?;
        let mut origin = POINT { x: 0, y: 0 };
        if !ClientToScreen(hwnd, &mut origin).as_bool() {
            return None;
        }
        let (w, h) = (rc.right - rc.left, rc.bottom - rc.top);
        (w > 0 && h > 0).then_some(GameRect { x: origin.x, y: origin.y, w, h })
    }
}

// ---------------------------------------------------------------- overlay

pub fn enable_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// Window messages the app cares about.
#[derive(Clone, Copy, Debug)]
pub enum Msg {
    Tick,
    MouseDown { x: i32, y: i32 },
    MouseMove { x: i32, y: i32 },
    MouseUp,
    Wheel { x: i32, y: i32, delta: i32, ctrl: bool },
}

pub type Handler = fn(Msg);

thread_local! {
    static HANDLER: RefCell<Option<Handler>> = const { RefCell::new(None) };
}

fn dispatch(m: Msg) {
    if let Some(f) = HANDLER.with(|t| *t.borrow()) {
        f(m);
    }
}

fn lp_xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as u16 as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            dispatch(Msg::Tick);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let (x, y) = lp_xy(lp);
            SetCapture(hwnd);
            dispatch(Msg::MouseDown { x, y });
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let (x, y) = lp_xy(lp);
            dispatch(Msg::MouseMove { x, y });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            dispatch(Msg::MouseUp);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            // Screen → client coordinates.
            let (sx, sy) = lp_xy(lp);
            let mut p = POINT { x: sx, y: sy };
            let _ = ScreenToClient(hwnd, &mut p);
            let delta = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            let ctrl = wp.0 & 0x0008 != 0;
            dispatch(Msg::Wheel { x: p.x, y: p.y, delta, ctrl });
            LRESULT(0)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_SETCURSOR => {
            let id = match CURSOR.load(Ordering::Relaxed) {
                1 => IDC_SIZEWE,
                2 => IDC_SIZENS,
                3 => IDC_SIZENWSE,
                4 => IDC_SIZENESW,
                _ => IDC_SIZEALL,
            };
            if let Ok(c) = LoadCursorW(None, id) {
                SetCursor(c);
            }
            LRESULT(1)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Layered window: per-pixel transparent, always on top, click-through, never takes focus,
/// and not shown in the taskbar.
pub fn create_overlay() -> windows::core::Result<HWND> {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("RogueOverlayWnd");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassW(&wc);
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Rogue Overlay HUD"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            hinst,
            None,
        )
    }
}

/// Runs the message loop; `handler` gets a `Msg::Tick` every `interval_ms`.
pub fn run(hwnd: HWND, interval_ms: u32, handler: Handler) {
    HANDLER.with(|t| *t.borrow_mut() = Some(handler));
    unsafe {
        SetTimer(hwnd, 1, interval_ms.max(10), None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

pub fn set_interval(hwnd: HWND, interval_ms: u32) {
    unsafe {
        SetTimer(hwnd, 1, interval_ms.max(10), None);
    }
}

/// Edit mode: the window stops being click-through and receives clicks/drags.
pub fn set_editing(hwnd: HWND, editing: bool) {
    EDITING.store(editing, Ordering::Relaxed);
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let t = WS_EX_TRANSPARENT.0 as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, if editing { ex & !t } else { ex | t });
    }
}

/// Cursor position in screen coordinates.
pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

/// Excludes the overlay from screen captures (including our own when reading the mana, which
/// would otherwise read itself). Trade-off: OBS / Discord will not see it either. Windows 10 2004+.
pub fn exclude_from_capture(hwnd: HWND, exclude: bool) {
    unsafe {
        let _ = SetWindowDisplayAffinity(hwnd, if exclude { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE });
    }
}

pub fn shift_down() -> bool {
    unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(0x10) < 0 }
}

pub fn destroy(hwnd: HWND) {
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

pub fn show(hwnd: HWND, visible: bool) {
    unsafe {
        let _ = ShowWindow(hwnd, if visible { SW_SHOWNOACTIVATE } else { SW_HIDE });
    }
}

pub fn bring_to_top(hwnd: HWND) {
    unsafe {
        let _ = SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER,
        );
    }
}

/// Alert sound: "" = none, "system" = Windows sound, anything else = path to a .wav.
pub fn play_sound(spec: &str) {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_FILENAME, SND_NODEFAULT};
    let spec = spec.trim();
    if spec.is_empty() {
        return;
    }
    unsafe {
        if spec.eq_ignore_ascii_case("system") {
            let _ = PlaySoundW(w!("SystemExclamation"), None, SND_ALIAS | SND_ASYNC);
        } else {
            let mut p = std::path::PathBuf::from(spec);
            if p.is_relative() {
                p = crate::config::exe_dir().join(p);
            }
            let wide: Vec<u16> = p.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
            let _ = PlaySoundW(PCWSTR(wide.as_ptr()), None, SND_FILENAME | SND_ASYNC | SND_NODEFAULT);
        }
    }
}

pub fn message_box(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    unsafe {
        MessageBoxW(None, PCWSTR(wide.as_ptr()), w!("Rogue Overlay"), MB_OK | MB_ICONWARNING | MB_TOPMOST);
    }
}

/// 32-bit DIB (premultiplied BGRA, top-down) selected into a memory DC.
pub struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    pub w: i32,
    pub h: i32,
}

impl Surface {
    pub fn new(w: i32, h: i32) -> windows::core::Result<Self> {
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut c_void = std::ptr::null_mut();
            let bmp = match CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(b) => b,
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(e);
                }
            };
            let old = SelectObject(dc, bmp);
            Ok(Self { dc, bmp, old, bits: bits as *mut u8, w, h })
        }
    }

    pub fn bits(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h * 4) as usize) }
    }

    /// Copies a premultiplied RGBA buffer (tiny-skia) and presents it on the layered window.
    /// With `dirty`, only that rectangle (x, y, w, h) is copied and recomposited by Windows.
    pub fn present(&mut self, hwnd: HWND, rgba: &[u8], x: i32, y: i32, dirty: Option<(i32, i32, i32, i32)>) {
        let (sw, sh) = (self.w, self.h);
        let (dx, dy, dw, dh) = match dirty {
            Some((rx, ry, rw, rh)) => {
                let x0 = rx.clamp(0, sw);
                let y0 = ry.clamp(0, sh);
                let x1 = (rx + rw).clamp(0, sw);
                let y1 = (ry + rh).clamp(0, sh);
                (x0, y0, x1 - x0, y1 - y0)
            }
            None => (0, 0, sw, sh),
        };
        if dw <= 0 || dh <= 0 {
            return;
        }
        let dst = self.bits();
        for row in dy..dy + dh {
            let o = ((row * sw + dx) * 4) as usize;
            let n = (dw * 4) as usize;
            for (d, s) in dst[o..o + n].chunks_exact_mut(4).zip(rgba[o..o + n].chunks_exact(4)) {
                d[0] = s[2];
                d[1] = s[1];
                d[2] = s[0];
                d[3] = s[3];
            }
        }
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let pos = POINT { x, y };
        let size = SIZE { cx: sw, cy: sh };
        let src = POINT { x: 0, y: 0 };
        let rc = RECT { left: dx, top: dy, right: dx + dw, bottom: dy + dh };
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: HDC::default(),
            pptDst: &pos,
            psize: &size,
            hdcSrc: self.dc,
            pptSrc: &src,
            crKey: COLORREF(0),
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: if dirty.is_some() { &rc } else { std::ptr::null() },
        };
        unsafe {
            let _ = UpdateLayeredWindowIndirect(hwnd, &info);
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp);
            let _ = DeleteDC(self.dc);
        }
    }
}

thread_local! {
    /// Capture DIBs reused between calls, keyed by size.
    static CAPTURE_SURFACES: RefCell<HashMap<(i32, i32), Surface>> = RefCell::new(HashMap::new());
}

/// Captures a screen rectangle (screen coordinates) as BGRA. The overlay itself is kept out
/// of it with `exclude_from_capture`.
pub fn capture(x: i32, y: i32, w: i32, h: i32) -> Option<Vec<u8>> {
    if w <= 0 || h <= 0 {
        return None;
    }
    CAPTURE_SURFACES.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 8 && !c.contains_key(&(w, h)) {
            c.clear();
        }
        let surf = match c.entry((w, h)) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => e.insert(Surface::new(w, h).ok()?),
        };
        unsafe {
            let screen = GetDC(None);
            let ok = BitBlt(surf.dc, 0, 0, w, h, screen, x, y, SRCCOPY).is_ok();
            ReleaseDC(None, screen);
            let _ = GdiFlush();
            ok.then(|| surf.bits().to_vec())
        }
    })
}

// ---------------------------------------------------------------- WinRT: OCR and HTTP

/// Initialises WinRT on the calling (worker) thread. Safe to call more than once.
pub fn winrt_init() {
    unsafe {
        let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
    }
}

/// Reads the text in a BGRA image with the OCR engine built into Windows (Windows 10+), using
/// the user's language. Returns the recognised lines joined with spaces.
pub fn ocr_bgra(engine: &windows::Media::Ocr::OcrEngine, bgra: &[u8], w: i32, h: i32) -> windows::core::Result<String> {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Security::Cryptography::CryptographicBuffer;
    let buf = CryptographicBuffer::CreateFromByteArray(bgra)?;
    let bmp = SoftwareBitmap::CreateCopyFromBuffer(&buf, BitmapPixelFormat::Bgra8, w, h)?;
    let result = engine.RecognizeAsync(&bmp)?.get()?;
    let mut out = Vec::new();
    for line in result.Lines()? {
        out.push(line.Text()?.to_string_lossy());
    }
    Ok(out.join(" "))
}

pub fn ocr_engine() -> windows::core::Result<windows::Media::Ocr::OcrEngine> {
    windows::Media::Ocr::OcrEngine::TryCreateFromUserProfileLanguages()
}

/// Simple HTTPS GET with the WinRT HTTP client (no extra dependencies). Blocking: call it from a
/// worker thread.
pub fn http_get(url: &str) -> Result<String, String> {
    let run = || -> windows::core::Result<String> {
        winrt_init();
        let client = windows::Web::Http::HttpClient::new()?;
        let uri = windows::Foundation::Uri::CreateUri(&windows::core::HSTRING::from(url))?;
        Ok(client.GetStringAsync(&uri)?.get()?.to_string_lossy())
    };
    run().map_err(|e| format!("Download failed: {e}"))
}
