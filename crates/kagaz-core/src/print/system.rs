//! Printing through the operating system's own print queues, and so through
//! whatever driver is installed for the printer, exactly as that driver
//! behaves. Kagaz only passes the per-job choices (copies, sides, paper,
//! colour, pages); it never changes a queue's or driver's settings.
//!
//! - Linux and macOS: CUPS, through its `lpstat` and `lp` tools.
//! - Windows: the print spooler. Kagaz renders each page itself and draws it
//!   through the driver with GDI, so PDFs print without a PDF reader.
//! - Android: the app hands the document to Android's print dialog (that
//!   lives in the desktop crate's mobile layer); nothing here.

use super::{Fit, PrintError, PrintRequest};
use crate::scan::Page;
use crate::Device;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One print queue the operating system knows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Queue {
    /// The name the system uses for it (CUPS queue or Windows printer name).
    pub name: String,
    /// A friendlier description when the system has one.
    pub description: String,
    pub is_default: bool,
    /// Where it sends jobs: the CUPS device URI or the Windows port name.
    pub target: String,
}

impl Queue {
    /// Whether this queue looks like it prints to `device`: its address,
    /// host name or model appears in the queue's name or target.
    pub fn matches(&self, device: &Device) -> bool {
        let hay = format!("{} {} {}", self.name, self.description, self.target).to_lowercase();
        let hay_words = hay.replace(['_', '-'], " ");
        device
            .addresses
            .iter()
            .any(|a| hay.contains(&a.to_string()))
            || device
                .hostname
                .as_deref()
                .map(|h| h.trim_end_matches('.').to_lowercase())
                .is_some_and(|h| !h.is_empty() && hay.contains(&h))
            || device
                .model
                .as_deref()
                .map(|m| m.to_lowercase().replace(['_', '-'], " "))
                .is_some_and(|m| m.len() >= 4 && hay_words.contains(&m))
    }
}

/// The queue for `device`, if one of `queues` matches it.
pub fn queue_for<'a>(queues: &'a [Queue], device: &Device) -> Option<&'a Queue> {
    queues.iter().find(|q| q.matches(device))
}

/// The print queues this computer has, the default first.
pub fn queues() -> Result<Vec<Queue>, PrintError> {
    let mut list = platform::queues()?;
    list.sort_by_key(|q| !q.is_default);
    Ok(list)
}

/// Print a PDF, JPEG or PNG through `queue`. Returns the system's job label.
pub fn print_file(queue: &str, path: &Path, req: &PrintRequest) -> Result<String, PrintError> {
    platform::print_file(queue, path, req)
}

/// Print scanned pages through `queue` (the copy half of a photocopy).
pub fn print_scans(
    queue: &str,
    pages: &[Page],
    req: &PrintRequest,
    fit: Fit,
) -> Result<String, PrintError> {
    if pages.is_empty() {
        return Err(PrintError::Unsupported(
            "there are no pages to print".into(),
        ));
    }
    platform::print_scans(queue, pages, req, fit)
}

/// Whether this platform prints through system queues from the core.
pub fn available() -> bool {
    platform::AVAILABLE
}

#[cfg(all(unix, not(any(target_os = "android", target_os = "ios"))))]
mod platform {
    //! CUPS. `lpstat` answers in the user's language unless told otherwise,
    //! so every call runs with LC_ALL=C.
    use super::*;
    use std::process::Command;

    pub const AVAILABLE: bool = true;

    fn run(program: &str, args: &[&str]) -> Result<String, PrintError> {
        let out = Command::new(program)
            .args(args)
            .env("LC_ALL", "C")
            .output()
            .map_err(|e| {
                PrintError::Unsupported(format!("cannot run {program} ({e}); is CUPS installed?"))
            })?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(PrintError::Unsupported(if err.is_empty() {
                format!("{program} failed")
            } else {
                err
            }));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn queues() -> Result<Vec<Queue>, PrintError> {
        // No queues at all makes lpstat exit non-zero; that is just "none".
        let names = match run("lpstat", &["-e"]) {
            Ok(text) => text,
            Err(PrintError::Unsupported(msg)) if msg.contains("cannot run") => {
                return Err(PrintError::Unsupported(msg))
            }
            Err(_) => return Ok(Vec::new()),
        };
        let default = run("lpstat", &["-d"]).unwrap_or_default();
        let default = default
            .split_once(':')
            .map(|(_, d)| d.trim().to_string())
            .unwrap_or_default();
        let targets = run("lpstat", &["-v"]).unwrap_or_default();
        Ok(parse_queues(&names, &default, &targets))
    }

    pub(super) fn parse_queues(names: &str, default: &str, targets: &str) -> Vec<Queue> {
        names
            .lines()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|name| {
                let target = targets
                    .lines()
                    .find_map(|l| {
                        let rest = l.strip_prefix("device for ")?;
                        let (q, uri) = rest.split_once(':')?;
                        (q == name).then(|| uri.trim().to_string())
                    })
                    .unwrap_or_default();
                Queue {
                    name: name.to_string(),
                    description: name.replace('_', " "),
                    is_default: name == default,
                    target,
                }
            })
            .collect()
    }

    pub(super) fn lp_args(queue: &str, req: &PrintRequest, extra: &[String]) -> Vec<String> {
        let mut args = vec!["-d".to_string(), queue.to_string()];
        if req.copies > 1 {
            args.extend(["-n".to_string(), req.copies.to_string()]);
        }
        args.extend(["-o".to_string(), format!("sides={}", req.sides.keyword())]);
        if let Some(media) = &req.media {
            args.extend(["-o".to_string(), format!("media={media}")]);
        }
        if !req.color {
            args.extend(["-o".to_string(), "print-color-mode=monochrome".to_string()]);
        }
        if let Some(pages) = req.pages.as_deref().filter(|p| !p.trim().is_empty()) {
            args.extend(["-P".to_string(), pages.replace(' ', "")]);
        }
        if let Some(name) = &req.job_name {
            args.extend(["-t".to_string(), name.clone()]);
        }
        args.extend(extra.iter().cloned());
        args
    }

    fn lp(args: Vec<String>, path: &Path) -> Result<String, PrintError> {
        let mut all: Vec<&str> = args.iter().map(String::as_str).collect();
        all.push("--");
        let p = path.to_str().ok_or_else(|| {
            PrintError::Unsupported(format!("{} is not a usable path", path.display()))
        })?;
        all.push(p);
        let out = run("lp", &all)?;
        // "request id is Brother-12 (1 file(s))"
        Ok(out
            .split_whitespace()
            .skip_while(|w| *w != "is")
            .nth(1)
            .unwrap_or("sent")
            .to_string())
    }

    pub fn print_file(queue: &str, path: &Path, req: &PrintRequest) -> Result<String, PrintError> {
        lp(lp_args(queue, req, &[]), path)
    }

    pub fn print_scans(
        queue: &str,
        pages: &[Page],
        req: &PrintRequest,
        fit: Fit,
    ) -> Result<String, PrintError> {
        // A PDF keeps every page at its scanned physical size; CUPS lays it
        // on the paper ("auto" shrinks only what does not fit).
        let dir = std::env::temp_dir().join(format!("kagaz-copy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).map_err(|e| PrintError::Render(e.to_string()))?;
        let pdf = dir.join("copy.pdf");
        let color = if req.color {
            crate::scan::ColorMode::Color
        } else {
            crate::scan::ColorMode::Gray
        };
        let result = crate::output::write(
            pages,
            &crate::output::OutputOptions {
                format: crate::output::Format::Pdf,
                color,
                quality: Some(90),
                max_bytes: None,
            },
            &pdf,
        )
        .map_err(|e| PrintError::Render(e.to_string()))
        .and_then(|_| {
            let scaling = match fit {
                Fit::ActualSize => "print-scaling=auto",
                Fit::FitToPage => "print-scaling=fit",
            };
            let mut req = req.clone();
            req.pages = None;
            lp(
                lp_args(queue, &req, &["-o".to_string(), scaling.to_string()]),
                &pdf,
            )
        });
        // lp has handed the file to the spooler by the time it returns.
        let _ = std::fs::remove_dir_all(&dir);
        result
    }
}

#[cfg(windows)]
mod platform {
    //! The Windows spooler. Pages are rendered by Kagaz at up to 300 dpi and
    //! stretched onto the driver's page with GDI; the driver does the rest
    //! (copies, duplex, colour, paper come in through its DEVMODE).
    use super::super::{render_file_pages, render_scans, Plan, RenderedPage, Sides};
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateDCW, DeleteDC, GetDeviceCaps, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DEVMODEW, DIB_RGB_COLORS, DM_COLOR, DM_COPIES, DM_DUPLEX, DM_PAPERSIZE, HDC, LOGPIXELSX,
        LOGPIXELSY, PHYSICALHEIGHT, PHYSICALOFFSETX, PHYSICALOFFSETY, PHYSICALWIDTH, SRCCOPY,
    };
    use windows_sys::Win32::Graphics::Printing::{
        ClosePrinter, DocumentPropertiesW, EnumPrintersW, GetDefaultPrinterW, OpenPrinterW,
        PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_HANDLE, PRINTER_INFO_2W,
    };
    use windows_sys::Win32::Storage::Xps::{
        AbortDoc, EndDoc, EndPage, StartDocW, StartPage, DOCINFOW,
    };

    pub const AVAILABLE: bool = true;

    const DM_IN_BUFFER: u32 = 8;
    const DM_OUT_BUFFER: u32 = 2;
    const DMPAPER_LETTER: i16 = 1;
    const DMPAPER_LEGAL: i16 = 5;
    const DMPAPER_A4: i16 = 9;
    const DMPAPER_A5: i16 = 11;
    const DMCOLOR_MONOCHROME: i16 = 1;
    const DMCOLOR_COLOR: i16 = 2;
    const DMDUP_SIMPLEX: i16 = 1;
    const DMDUP_VERTICAL: i16 = 2;
    const DMDUP_HORIZONTAL: i16 = 3;
    /// Rendering finer than this only costs memory; GDI stretches up.
    const MAX_RENDER_DPI: u32 = 300;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn from_wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }

    fn default_printer() -> Option<String> {
        unsafe {
            let mut len = 0u32;
            GetDefaultPrinterW(null_mut(), &mut len);
            if len == 0 {
                return None;
            }
            let mut buf = vec![0u16; len as usize];
            if GetDefaultPrinterW(buf.as_mut_ptr(), &mut len) == 0 {
                return None;
            }
            Some(from_wide(buf.as_ptr()))
        }
    }

    pub fn queues() -> Result<Vec<Queue>, PrintError> {
        let default = default_printer();
        let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
        unsafe {
            let (mut needed, mut count) = (0u32, 0u32);
            EnumPrintersW(flags, null(), 2, null_mut(), 0, &mut needed, &mut count);
            if needed == 0 {
                return Ok(Vec::new());
            }
            // u64 backing keeps the PRINTER_INFO_2W array aligned.
            let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
            if EnumPrintersW(
                flags,
                null(),
                2,
                buf.as_mut_ptr() as *mut u8,
                needed,
                &mut needed,
                &mut count,
            ) == 0
            {
                return Err(PrintError::Unsupported(
                    "Windows would not list its printers".into(),
                ));
            }
            let infos =
                std::slice::from_raw_parts(buf.as_ptr() as *const PRINTER_INFO_2W, count as usize);
            Ok(infos
                .iter()
                .map(|i| {
                    let name = from_wide(i.pPrinterName);
                    let driver = from_wide(i.pDriverName);
                    let comment = from_wide(i.pComment);
                    Queue {
                        is_default: default.as_deref() == Some(name.as_str()),
                        description: if comment.is_empty() { driver } else { comment },
                        target: from_wide(i.pPortName),
                        name,
                    }
                })
                .collect())
        }
    }

    /// The driver's own defaults with the job's choices laid over them.
    fn devmode(queue: &str, req: &PrintRequest) -> Result<Vec<u64>, PrintError> {
        let name = wide(queue);
        unsafe {
            let mut printer = PRINTER_HANDLE::default();
            if OpenPrinterW(name.as_ptr(), &mut printer, null()) == 0 {
                return Err(PrintError::Unsupported(format!(
                    "Windows has no printer called \"{queue}\""
                )));
            }
            let size =
                DocumentPropertiesW(null_mut(), printer, name.as_ptr(), null_mut(), null(), 0);
            if size <= 0 {
                ClosePrinter(printer);
                return Err(PrintError::Unsupported(format!(
                    "the driver for \"{queue}\" gave no settings"
                )));
            }
            let mut buf = vec![0u64; (size as usize).div_ceil(8)];
            let dm = buf.as_mut_ptr() as *mut DEVMODEW;
            DocumentPropertiesW(
                null_mut(),
                printer,
                name.as_ptr(),
                dm,
                null(),
                DM_OUT_BUFFER,
            );
            let d = &mut *dm;
            d.dmFields |= DM_COPIES | DM_DUPLEX | DM_COLOR;
            d.Anonymous1.Anonymous1.dmCopies = req.copies.clamp(1, 999) as i16;
            d.dmDuplex = match req.sides {
                Sides::OneSided => DMDUP_SIMPLEX,
                Sides::TwoSidedLongEdge => DMDUP_VERTICAL,
                Sides::TwoSidedShortEdge => DMDUP_HORIZONTAL,
            };
            d.dmColor = if req.color {
                DMCOLOR_COLOR
            } else {
                DMCOLOR_MONOCHROME
            };
            let paper = req.media.as_deref().and_then(|m| match m {
                "iso_a4_210x297mm" => Some(DMPAPER_A4),
                "iso_a5_148x210mm" => Some(DMPAPER_A5),
                "na_letter_8.5x11in" => Some(DMPAPER_LETTER),
                "na_legal_8.5x14in" => Some(DMPAPER_LEGAL),
                _ => None,
            });
            if let Some(p) = paper {
                d.dmFields |= DM_PAPERSIZE;
                d.Anonymous1.Anonymous1.dmPaperSize = p;
            }
            // Let the driver reconcile the choices with what it can do.
            DocumentPropertiesW(
                null_mut(),
                printer,
                name.as_ptr(),
                dm,
                dm,
                DM_IN_BUFFER | DM_OUT_BUFFER,
            );
            ClosePrinter(printer);
            Ok(buf)
        }
    }

    struct Dc(HDC);
    impl Drop for Dc {
        fn drop(&mut self) {
            unsafe {
                DeleteDC(self.0);
            }
        }
    }

    /// Open the printer, plan the pages for its paper, render with
    /// `render`, and draw them.
    fn draw(
        queue: &str,
        req: &PrintRequest,
        job_name: &str,
        render: impl FnOnce(&Plan) -> Result<Vec<RenderedPage>, PrintError>,
    ) -> Result<String, PrintError> {
        let dm = devmode(queue, req)?;
        let name = wide(queue);
        let dc = unsafe {
            CreateDCW(
                null(),
                name.as_ptr(),
                null(),
                dm.as_ptr() as *const DEVMODEW,
            )
        };
        if dc.is_null() {
            return Err(PrintError::Unsupported(format!(
                "cannot open \"{queue}\" for printing"
            )));
        }
        let dc = Dc(dc);
        let (dpi_x, dpi_y, phys_w, phys_h, off_x, off_y) = unsafe {
            (
                GetDeviceCaps(dc.0, LOGPIXELSX as i32),
                GetDeviceCaps(dc.0, LOGPIXELSY as i32),
                GetDeviceCaps(dc.0, PHYSICALWIDTH as i32),
                GetDeviceCaps(dc.0, PHYSICALHEIGHT as i32),
                GetDeviceCaps(dc.0, PHYSICALOFFSETX as i32),
                GetDeviceCaps(dc.0, PHYSICALOFFSETY as i32),
            )
        };
        if dpi_x <= 0 || dpi_y <= 0 || phys_w <= 0 || phys_h <= 0 {
            return Err(PrintError::Unsupported(format!(
                "the driver for \"{queue}\" reported no page size"
            )));
        }
        let plan = Plan {
            dpi: (dpi_x as u32).min(MAX_RENDER_DPI),
            gray: !req.color,
            media: req.media.clone().unwrap_or_default(),
            width_pt: (phys_w as f64 * 72.0 / dpi_x as f64).round() as u32,
            height_pt: (phys_h as f64 * 72.0 / dpi_y as f64).round() as u32,
            sides: req.sides,
            // The driver turns back sides itself.
            sheet_back_rotated: false,
            copies: 1,
        };
        let pages = render(&plan)?;
        let doc_name = wide(job_name);
        let info = DOCINFOW {
            cbSize: std::mem::size_of::<DOCINFOW>() as i32,
            lpszDocName: doc_name.as_ptr(),
            lpszOutput: null(),
            lpszDatatype: null(),
            fwType: 0,
        };
        unsafe {
            let job = StartDocW(dc.0, &info);
            if job <= 0 {
                return Err(PrintError::Unsupported(format!(
                    "Windows would not start a job on \"{queue}\""
                )));
            }
            for page in &pages {
                let (bits, header) = dib(page);
                let bmi = BITMAPINFO {
                    bmiHeader: header,
                    bmiColors: [std::mem::zeroed()],
                };
                let ok = StartPage(dc.0) > 0
                    && StretchDIBits(
                        dc.0,
                        -off_x,
                        -off_y,
                        phys_w,
                        phys_h,
                        0,
                        0,
                        page.width as i32,
                        page.height as i32,
                        bits.as_ptr() as *const _,
                        &bmi,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    ) != 0
                    && EndPage(dc.0) > 0;
                if !ok {
                    AbortDoc(dc.0);
                    return Err(PrintError::Unsupported(format!(
                        "the driver for \"{queue}\" refused a page"
                    )));
                }
            }
            EndDoc(dc.0);
            Ok(job.to_string())
        }
    }

    /// A top-down 24-bit DIB (BGR, rows padded to four bytes).
    fn dib(page: &RenderedPage) -> (Vec<u8>, BITMAPINFOHEADER) {
        let (w, h) = (page.width as usize, page.height as usize);
        let stride = (w * 3).div_ceil(4) * 4;
        let mut bits = vec![0u8; stride * h];
        for y in 0..h {
            let row = &mut bits[y * stride..y * stride + w * 3];
            for x in 0..w {
                let (r, g, b) = if page.components == 1 {
                    let v = page.pixels[y * w + x];
                    (v, v, v)
                } else {
                    let i = (y * w + x) * 3;
                    (page.pixels[i], page.pixels[i + 1], page.pixels[i + 2])
                };
                row[x * 3..x * 3 + 3].copy_from_slice(&[b, g, r]);
            }
        }
        let header = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(h as i32),
            biPlanes: 1,
            biBitCount: 24,
            biCompression: BI_RGB,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        };
        (bits, header)
    }

    pub fn print_file(queue: &str, path: &Path, req: &PrintRequest) -> Result<String, PrintError> {
        let job_name = req.job_name.clone().unwrap_or_else(|| {
            path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Kagaz print")
                .to_string()
        });
        draw(queue, req, &job_name, |plan| {
            render_file_pages(path, plan, req.pages.as_deref(), &mut |_| {})
        })
    }

    pub fn print_scans(
        queue: &str,
        pages: &[Page],
        req: &PrintRequest,
        fit: Fit,
    ) -> Result<String, PrintError> {
        let job_name = req.job_name.clone().unwrap_or_else(|| "Kagaz copy".into());
        draw(queue, req, &job_name, |plan| {
            render_scans(pages, plan, fit, &mut |_| {})
        })
    }
}

#[cfg(not(any(windows, all(unix, not(any(target_os = "android", target_os = "ios"))))))]
mod platform {
    use super::*;

    pub const AVAILABLE: bool = false;

    fn unsupported() -> PrintError {
        PrintError::Unsupported(
            "this system's print queues are reached through its own print dialog".into(),
        )
    }

    pub fn queues() -> Result<Vec<Queue>, PrintError> {
        Ok(Vec::new())
    }

    pub fn print_file(_: &str, _: &Path, _: &PrintRequest) -> Result<String, PrintError> {
        Err(unsupported())
    }

    pub fn print_scans(
        _: &str,
        _: &[Page],
        _: &PrintRequest,
        _: Fit,
    ) -> Result<String, PrintError> {
        Err(unsupported())
    }
}

#[cfg(all(test, unix, not(target_os = "android")))]
mod tests {
    use super::platform::{lp_args, parse_queues};
    use super::*;
    use crate::print::Sides;

    #[test]
    fn reads_cups_queues() {
        let q = parse_queues(
            "Office_Laser\nPhoto\n",
            "Photo",
            "device for Office_Laser: ipp://192.0.2.40/ipp/print\ndevice for Photo: usb://Example/Photo?serial=1\n",
        );
        assert_eq!(q.len(), 2);
        assert_eq!(q[0].name, "Office_Laser");
        assert_eq!(q[0].description, "Office Laser");
        assert_eq!(q[0].target, "ipp://192.0.2.40/ipp/print");
        assert!(!q[0].is_default);
        assert!(q[1].is_default);
    }

    #[test]
    fn matches_a_device_by_address_or_model() {
        let q = Queue {
            name: "Example_LX-200_series".into(),
            description: "Example LX-200 series".into(),
            is_default: false,
            target: "implicitclass://Example_LX-200_series/".into(),
        };
        let mut d = Device {
            model: Some("LX-200".into()),
            ..Default::default()
        };
        assert!(q.matches(&d));
        d.model = Some("Other".into());
        assert!(!q.matches(&d));
        d.addresses = vec!["192.0.2.40".parse().unwrap()];
        let by_ip = Queue {
            target: "ipp://192.0.2.40/ipp/print".into(),
            ..q
        };
        assert!(by_ip.matches(&d));
    }

    #[test]
    fn passes_only_the_job_choices_to_lp() {
        let req = PrintRequest {
            copies: 2,
            sides: Sides::TwoSidedLongEdge,
            media: Some("iso_a4_210x297mm".into()),
            color: false,
            job_name: Some("Letter".into()),
            pages: Some("1-2, 4".into()),
        };
        assert_eq!(
            lp_args("Office", &req, &[]),
            [
                "-d",
                "Office",
                "-n",
                "2",
                "-o",
                "sides=two-sided-long-edge",
                "-o",
                "media=iso_a4_210x297mm",
                "-o",
                "print-color-mode=monochrome",
                "-P",
                "1-2,4",
                "-t",
                "Letter"
            ]
        );
    }
}
