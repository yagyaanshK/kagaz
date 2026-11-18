//! This computer's screen on Windows, for mirroring to a TV: the main
//! screen through Windows.Graphics.Capture (Windows 10 1903 or newer),
//! pointer included. Windows marks the screen as being captured while this
//! runs. Kagaz takes the newest picture when the TV is ready for one and
//! scales it to the TV's size.
//!
//! Not yet run on Windows: written to Microsoft's documentation and checked
//! by building for Windows from Linux.

use super::picture::{scale, Layout};
use std::time::{Duration, Instant};
use windows::core::Interface;
use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::{HMODULE, POINT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_CPU_ACCESS_READ,
    D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAPPED_SUBRESOURCE, D3D11_MAP_READ, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};

fn err(what: &str) -> impl Fn(windows::core::Error) -> String + '_ {
    move |e| format!("{what}: {}", e.message())
}

/// The main screen being captured. Dropping it ends the capture.
pub struct Screen {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
    scaled: Vec<u8>,
    /// Pictures that arrived from Windows.
    pub arrived: u64,
    /// The screen's size.
    pub width: u32,
    pub height: u32,
}

// The capture objects are free-threaded (the frame pool is made so).
unsafe impl Send for Screen {}

impl Screen {
    /// Starts capturing the main screen.
    pub fn main() -> Result<Screen, String> {
        unsafe {
            // Already set up on this thread is fine.
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
            if !GraphicsCaptureSession::IsSupported().unwrap_or(false) {
                return Err("this Windows cannot capture the screen for apps (Windows 10 1903 or newer is needed)".into());
            }
            let (mut device, mut context) = (None, None);
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut context),
            )
            .map_err(err("no Direct3D device"))?;
            let device: ID3D11Device = device.ok_or("no Direct3D device")?;
            let context = context.ok_or("no Direct3D context")?;
            let dxgi: IDXGIDevice = device.cast().map_err(err("no DXGI device"))?;
            let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)
                .and_then(|i| i.cast())
                .map_err(err("no capture device"))?;
            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()
                .map_err(err("screen capture is not available"))?;
            let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let item: GraphicsCaptureItem = interop.CreateForMonitor(monitor).map_err(err("the main screen"))?;
            let size = item.Size().map_err(err("the screen's size"))?;
            let pool =
                Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 2, size)
                    .map_err(err("the capture's pictures"))?;
            let session = pool.CreateCaptureSession(&item).map_err(err("the capture"))?;
            let _ = session.SetIsCursorCaptureEnabled(true);
            // Windows 11 can leave the yellow frame off; Windows 10 cannot.
            let _ = session.SetIsBorderRequired(false);
            session.StartCapture().map_err(err("starting the capture"))?;
            Ok(Screen {
                device,
                context,
                pool,
                session,
                staging: None,
                scaled: Vec::new(),
                arrived: 0,
                width: size.Width as u32,
                height: size.Height as u32,
            })
        }
    }

    /// Waits up to `wait` for the first picture; the screen's size.
    pub fn first_picture(&mut self, wait: Duration) -> Option<(usize, usize)> {
        let end = Instant::now() + wait;
        while Instant::now() < end {
            if let Some(f) = self.newest() {
                let _ = self.take(&f, 16, 16);
                return Some((self.width as usize, self.height as usize));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    /// The newest picture waiting, if any (older ones are let go).
    fn newest(&mut self) -> Option<Direct3D11CaptureFrame> {
        let mut last = None;
        while let Ok(f) = self.pool.TryGetNextFrame() {
            self.arrived += 1;
            last = Some(f);
        }
        last
    }

    /// The newest picture scaled to `width` x `height` RGBA, with black
    /// bars where the shapes differ; the last one again when nothing new
    /// came.
    pub fn picture(&mut self, width: usize, height: usize) -> &[u8] {
        if self.scaled.len() != width * height * 4 {
            self.scaled = vec![0; width * height * 4];
            for px in self.scaled.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }
        if let Some(f) = self.newest() {
            if let Err(e) = self.take(&f, width, height) {
                eprintln!("kagaz: a picture of the screen could not be read: {e}");
            }
        }
        &self.scaled
    }

    /// Copies a picture out of graphics memory and scales it.
    fn take(&mut self, frame: &Direct3D11CaptureFrame, width: usize, height: usize) -> Result<(), String> {
        unsafe {
            let surface = frame.Surface().map_err(err("the picture"))?;
            let access: IDirect3DDxgiInterfaceAccess = surface.cast().map_err(err("the picture"))?;
            let texture: ID3D11Texture2D = access.GetInterface().map_err(err("the picture"))?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            texture.GetDesc(&mut desc);
            if self.staging.as_ref().is_none_or(|s| (s.1, s.2) != (desc.Width, desc.Height)) {
                desc.Usage = D3D11_USAGE_STAGING;
                desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                desc.BindFlags = 0;
                desc.MiscFlags = 0;
                desc.MipLevels = 1;
                desc.ArraySize = 1;
                let mut made = None;
                self.device.CreateTexture2D(&desc, None, Some(&mut made)).map_err(err("a copy of the picture"))?;
                self.staging = made.map(|t| (t, desc.Width, desc.Height));
            }
            let (staging, w, h) = self.staging.as_ref().ok_or("no copy of the picture")?;
            self.context.CopyResource(staging, &texture);
            let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).map_err(err("reading the picture"))?;
            let pitch = mapped.RowPitch as usize;
            let src = std::slice::from_raw_parts(mapped.pData as *const u8, pitch * *h as usize);
            if self.scaled.len() == width * height * 4 {
                scale(src, *w as usize, *h as usize, pitch, Layout::Bgrx, &mut self.scaled, width, height);
            }
            self.context.Unmap(staging, 0);
        }
        Ok(())
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = self.session.Close();
        let _ = self.pool.Close();
    }
}
