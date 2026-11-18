//! Windows: the main screen as pictures (DXGI's desktop duplication, or
//! GDI where that is not available), this computer's pointer drawn in,
//! and the viewer's mouse and keys given to Windows as if typed here.
//!
//! It runs as the person signed in, so it sees and works only what they
//! can: the sign-in screen and administrator prompts are out of its reach
//! (docs/remote-connection.md, step 2).

use crate::picture::{Frame, Source};
use ironrdp_pdu::input::fast_path::SynchronizeFlags;
use ironrdp_server::{KeyboardEvent, MouseEvent};
use std::time::Duration;
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_UNKNOWN;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput1, IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, SRCCOPY,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
    VK_CAPITAL, VK_NUMLOCK, VK_SCROLL,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, GetSystemMetrics, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, HICON, ICONINFO,
    SM_CXSCREEN, SM_CYSCREEN,
};

/// Pixels, not the scaled points Windows would otherwise report to a
/// program that has not said it knows about scaling.
pub fn dpi_aware() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// The main screen: DXGI unless told `gdi`, falling back to GDI.
pub fn open(how: Option<&str>) -> anyhow::Result<Box<dyn Source>> {
    match how {
        Some("gdi") => Ok(Box::new(Gdi::new()?)),
        Some("dxgi") => Ok(Box::new(Dxgi::new()?)),
        _ => match Dxgi::new() {
            Ok(d) => Ok(Box::new(d)),
            Err(e) => {
                crate::say(&format!("DXGI is not available ({e:#}): GDI instead"));
                Ok(Box::new(Gdi::new()?))
            }
        },
    }
}

/// A picture GDI can draw on: the pointer goes in here.
struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl Canvas {
    fn new(width: i32, height: i32) -> anyhow::Result<Canvas> {
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Negative: rows from the top down.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let made = CreateDIBSection(Some(screen), &info, DIB_RGB_COLORS, &mut bits, None, 0);
            ReleaseDC(None, screen);
            let bitmap = match made {
                Ok(b) => b,
                Err(e) => {
                    let _ = DeleteDC(dc);
                    return Err(e.into());
                }
            };
            let old = SelectObject(dc, bitmap.into());
            Ok(Canvas { dc, bitmap, old, bits: bits as *mut u8, width, height })
        }
    }

    fn bytes(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.width * self.height * 4) as usize) }
    }

    fn frame(&mut self) -> Frame {
        let (width, height) = (self.width as u16, self.height as u16);
        Frame { width, height, bgra: self.bytes().to_vec() }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// Where the pointer is and what it looks like, to see when it changed.
#[derive(PartialEq, Clone, Copy, Default)]
struct Pointer {
    shown: bool,
    shape: usize,
    x: i32,
    y: i32,
}

fn pointer_now() -> Pointer {
    unsafe {
        let mut info = CURSORINFO { cbSize: std::mem::size_of::<CURSORINFO>() as u32, ..Default::default() };
        if GetCursorInfo(&mut info).is_err() {
            return Pointer::default();
        }
        Pointer {
            shown: info.flags.0 & CURSOR_SHOWING.0 != 0,
            shape: info.hCursor.0 as usize,
            x: info.ptScreenPos.x,
            y: info.ptScreenPos.y,
        }
    }
}

/// This computer's pointer, drawn where it is: the viewer sees the person
/// here pointing.
fn draw_pointer(dc: HDC) -> Pointer {
    let now = pointer_now();
    if !now.shown || now.shape == 0 {
        return now;
    }
    unsafe {
        let icon = HICON(now.shape as *mut _);
        let mut parts = ICONINFO::default();
        let (mut hx, mut hy) = (0, 0);
        if GetIconInfo(icon, &mut parts).is_ok() {
            hx = parts.xHotspot as i32;
            hy = parts.yHotspot as i32;
            if !parts.hbmMask.is_invalid() {
                let _ = DeleteObject(parts.hbmMask.into());
            }
            if !parts.hbmColor.is_invalid() {
                let _ = DeleteObject(parts.hbmColor.into());
            }
        }
        let _ = DrawIconEx(dc, now.x - hx, now.y - hy, icon, 0, 0, 0, None, DI_NORMAL);
    }
    now
}

/// The main screen's size in pixels.
fn screen_size() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// The screen through GDI: a copy each time, sent when it differs.
struct Gdi {
    canvas: Canvas,
    last: Vec<u8>,
}

// Used only on the thread that takes the pictures.
unsafe impl Send for Gdi {}

impl Gdi {
    fn new() -> anyhow::Result<Gdi> {
        let (w, h) = screen_size();
        anyhow::ensure!(w > 0 && h > 0, "no screen");
        Ok(Gdi { canvas: Canvas::new(w, h)?, last: Vec::new() })
    }
}

impl Source for Gdi {
    fn name(&self) -> String {
        format!("the main screen through GDI, {}x{}", self.canvas.width, self.canvas.height)
    }

    fn next(&mut self, _wait: Duration) -> anyhow::Result<Option<Frame>> {
        let (w, h) = screen_size();
        if (w, h) != (self.canvas.width, self.canvas.height) {
            anyhow::bail!("the screen's size changed");
        }
        unsafe {
            let screen = GetDC(None);
            let copied = BitBlt(self.canvas.dc, 0, 0, w, h, Some(screen), 0, 0, SRCCOPY | CAPTUREBLT);
            ReleaseDC(None, screen);
            copied?;
        }
        draw_pointer(self.canvas.dc);
        let now = self.canvas.bytes();
        if now == self.last.as_slice() {
            return Ok(None);
        }
        self.last.clear();
        self.last.extend_from_slice(now);
        Ok(Some(self.canvas.frame()))
    }
}

/// The screen through DXGI: Windows says when it changed and hands over
/// the picture in the graphics card's memory.
struct Dxgi {
    context: ID3D11DeviceContext,
    duplication: IDXGIOutputDuplication,
    staging: Option<ID3D11Texture2D>,
    _device: ID3D11Device,
    canvas: Canvas,
    /// The latest picture without the pointer.
    clean: Vec<u8>,
    pointer: Pointer,
}

unsafe impl Send for Dxgi {}

impl Dxgi {
    fn new() -> anyhow::Result<Dxgi> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
            let mut i = 0;
            while let Ok(adapter) = factory.EnumAdapters1(i) {
                i += 1;
                let mut j = 0;
                while let Ok(output) = adapter.EnumOutputs(j) {
                    j += 1;
                    let desc = output.GetDesc()?;
                    let r = desc.DesktopCoordinates;
                    // The main screen is the one at the desktop's origin.
                    if !desc.AttachedToDesktop.as_bool() || r.left != 0 || r.top != 0 {
                        continue;
                    }
                    let mut device = None;
                    let mut context = None;
                    D3D11CreateDevice(
                        &adapter,
                        D3D_DRIVER_TYPE_UNKNOWN,
                        HMODULE::default(),
                        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                        None,
                        D3D11_SDK_VERSION,
                        Some(&mut device),
                        None,
                        Some(&mut context),
                    )?;
                    let device = device.ok_or_else(|| anyhow::anyhow!("no Direct3D device"))?;
                    let context = context.ok_or_else(|| anyhow::anyhow!("no Direct3D context"))?;
                    let output1: IDXGIOutput1 = output.cast()?;
                    let duplication = output1.DuplicateOutput(&device)?;
                    let (w, h) = (r.right - r.left, r.bottom - r.top);
                    return Ok(Dxgi {
                        context,
                        duplication,
                        staging: None,
                        _device: device,
                        canvas: Canvas::new(w, h)?,
                        clean: vec![0; (w * h * 4) as usize],
                        pointer: Pointer::default(),
                    });
                }
            }
            anyhow::bail!("no main screen found")
        }
    }

    /// The picture Windows handed over, into `clean`.
    fn copy(&mut self, resource: IDXGIResource) -> anyhow::Result<()> {
        unsafe {
            let texture: ID3D11Texture2D = resource.cast()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if (desc.Width as i32, desc.Height as i32) != (self.canvas.width, self.canvas.height) {
                anyhow::bail!("the screen's size changed");
            }
            if self.staging.is_none() {
                desc.Usage = D3D11_USAGE_STAGING;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                desc.BindFlags = 0;
                desc.MiscFlags = 0;
                desc.MipLevels = 1;
                desc.ArraySize = 1;
                let mut made = None;
                self._device.CreateTexture2D(&desc, None, Some(&mut made))?;
                self.staging = made;
            }
            let staging = self.staging.as_ref().ok_or_else(|| anyhow::anyhow!("no staging picture"))?;
            self.context.CopyResource(staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
            let row = self.canvas.width as usize * 4;
            let pitch = mapped.RowPitch as usize;
            let from = mapped.pData as *const u8;
            for y in 0..self.canvas.height as usize {
                let src = std::slice::from_raw_parts(from.add(y * pitch), row);
                self.clean[y * row..(y + 1) * row].copy_from_slice(src);
            }
            self.context.Unmap(staging, 0);
        }
        Ok(())
    }
}

impl Source for Dxgi {
    fn name(&self) -> String {
        format!("the main screen through DXGI, {}x{}", self.canvas.width, self.canvas.height)
    }

    fn next(&mut self, wait: Duration) -> anyhow::Result<Option<Frame>> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource: Option<IDXGIResource> = None;
        let got = unsafe { self.duplication.AcquireNextFrame(wait.as_millis() as u32, &mut info, &mut resource) };
        let mut changed = false;
        match got {
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => {}
            // Lost (a change of mode, the secure desktop): opened again.
            Err(e) => return Err(e.into()),
            Ok(()) => {
                let copied = match resource {
                    Some(r) if info.LastPresentTime != 0 => self.copy(r).map(|_| true),
                    _ => Ok(false),
                };
                unsafe {
                    let _ = self.duplication.ReleaseFrame();
                }
                changed = copied?;
            }
        }
        if !changed && pointer_now() == self.pointer {
            return Ok(None);
        }
        let clean = std::mem::take(&mut self.clean);
        self.canvas.bytes().copy_from_slice(&clean);
        self.clean = clean;
        self.pointer = draw_pointer(self.canvas.dc);
        Ok(Some(self.canvas.frame()))
    }
}

fn send(inputs: &[INPUT]) {
    unsafe {
        SendInput(inputs, std::mem::size_of::<INPUT>() as i32);
    }
}

fn keyboard(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

fn mouse_input(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as _, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    }
}

/// A key from the viewer, by its place on the keyboard (as Remote Desktop
/// sends it), so that this computer's own layout decides the letter.
pub fn key(event: KeyboardEvent) {
    let none = VIRTUAL_KEY(0);
    let input = match event {
        KeyboardEvent::Pressed { code, extended } => {
            keyboard(none, code as u16, KEYEVENTF_SCANCODE | if extended { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) })
        }
        KeyboardEvent::Released { code, extended } => keyboard(
            none,
            code as u16,
            KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP | if extended { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) },
        ),
        KeyboardEvent::UnicodePressed(unit) => keyboard(none, unit, KEYEVENTF_UNICODE),
        KeyboardEvent::UnicodeReleased(unit) => keyboard(none, unit, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
        KeyboardEvent::Synchronize(flags) => return locks(flags),
    };
    send(&[input]);
}

/// Caps Lock, Num Lock and Scroll Lock as the viewer's keyboard has them.
fn locks(flags: SynchronizeFlags) {
    for (vk, on, extended) in [
        (VK_CAPITAL, flags.contains(SynchronizeFlags::CAPS_LOCK), false),
        (VK_NUMLOCK, flags.contains(SynchronizeFlags::NUM_LOCK), true),
        (VK_SCROLL, flags.contains(SynchronizeFlags::SCROLL_LOCK), false),
    ] {
        let now = unsafe { GetKeyState(vk.0 as i32) } & 1 != 0;
        if now != on {
            let ext = if extended { KEYEVENTF_EXTENDEDKEY } else { KEYBD_EVENT_FLAGS(0) };
            send(&[keyboard(vk, 0, ext), keyboard(vk, 0, ext | KEYEVENTF_KEYUP)]);
        }
    }
}

/// The viewer's mouse, on the main screen of `size` pixels.
pub fn mouse(event: MouseEvent, size: (u16, u16)) {
    let (w, h) = (size.0.max(2) as i32, size.1.max(2) as i32);
    let at = |x: u16, y: u16| ((x as i32 * 65535 + (w - 1) / 2) / (w - 1), (y as i32 * 65535 + (h - 1) / 2) / (h - 1));
    let none = MOUSE_EVENT_FLAGS(0);
    let one = |flags: MOUSE_EVENT_FLAGS, data: i32| mouse_input(flags, 0, 0, data);
    match event {
        MouseEvent::Move { x, y } => {
            let (dx, dy) = at(x, y);
            send(&[mouse_input(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE, dx, dy, 0)]);
        }
        MouseEvent::RelMove { x, y } => send(&[mouse_input(MOUSEEVENTF_MOVE, x, y, 0)]),
        MouseEvent::LeftPressed => send(&[one(MOUSEEVENTF_LEFTDOWN, 0)]),
        MouseEvent::LeftReleased => send(&[one(MOUSEEVENTF_LEFTUP, 0)]),
        MouseEvent::RightPressed => send(&[one(MOUSEEVENTF_RIGHTDOWN, 0)]),
        MouseEvent::RightReleased => send(&[one(MOUSEEVENTF_RIGHTUP, 0)]),
        MouseEvent::MiddlePressed => send(&[one(MOUSEEVENTF_MIDDLEDOWN, 0)]),
        MouseEvent::MiddleReleased => send(&[one(MOUSEEVENTF_MIDDLEUP, 0)]),
        MouseEvent::Button4Pressed => send(&[one(MOUSEEVENTF_XDOWN, 1)]),
        MouseEvent::Button4Released => send(&[one(MOUSEEVENTF_XUP, 1)]),
        MouseEvent::Button5Pressed => send(&[one(MOUSEEVENTF_XDOWN, 2)]),
        MouseEvent::Button5Released => send(&[one(MOUSEEVENTF_XUP, 2)]),
        MouseEvent::VerticalScroll { value } => send(&[one(MOUSEEVENTF_WHEEL, value as i32)]),
        MouseEvent::Scroll { x, y } => {
            if y != 0 {
                send(&[one(MOUSEEVENTF_WHEEL, y)]);
            }
            if x != 0 {
                send(&[one(MOUSEEVENTF_HWHEEL, x)]);
            }
        }
    }
    let _ = none;
}

/// Mouse and key presses the person here made (not those a viewer sent,
/// which Windows marks as injected), counted by low-level hooks on a
/// thread of their own, started the first time it is asked.
static LOCAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static HOOKED: std::sync::Once = std::sync::Once::new();

unsafe extern "system" fn mouse_hook(code: i32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, LLMHF_INJECTED, MSLLHOOKSTRUCT};
    if code >= 0 {
        let info = &*(l.0 as *const MSLLHOOKSTRUCT);
        if info.flags & LLMHF_INJECTED == 0 {
            LOCAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    CallNextHookEx(None, code, w, l)
}

unsafe extern "system" fn key_hook(code: i32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{CallNextHookEx, KBDLLHOOKSTRUCT, LLKHF_INJECTED};
    if code >= 0 {
        let info = &*(l.0 as *const KBDLLHOOKSTRUCT);
        if info.flags.0 & LLKHF_INJECTED.0 == 0 {
            LOCAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    CallNextHookEx(None, code, w, l)
}

pub fn local_input() -> u64 {
    HOOKED.call_once(|| {
        std::thread::spawn(|| unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, SetWindowsHookExW, MSG, WH_KEYBOARD_LL, WH_MOUSE_LL};
            let module = windows::Win32::System::LibraryLoader::GetModuleHandleW(None).ok();
            let _m = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module.map(|m| m.into()), 0);
            let _k = SetWindowsHookExW(WH_KEYBOARD_LL, Some(key_hook), module.map(|m| m.into()), 0);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        });
    });
    LOCAL.load(std::sync::atomic::Ordering::Relaxed)
}

/// Locks Windows, as Windows key + L does.
pub fn lock() {
    unsafe {
        let _ = windows::Win32::System::Shutdown::LockWorkStation();
    }
}
