//! H.264 in the graphics chip on Linux, through VA-API (AMD and Intel
//! graphics; NVIDIA's own driver has no VA-API encoder).
//!
//! libva is opened only when this encoder is asked for, so Kagaz starts on a
//! computer without it and encodes with OpenH264 there. Low latency, as
//! Wi-Fi Display wants: Constrained Baseline, one reference picture, P
//! pictures only, a whole picture every ten seconds or when the TV asks.

use super::va_sys::*;
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};
use std::os::fd::AsRawFd;

type Dpy = *mut c_void;
type St = VAStatus;

/// The libva functions Kagaz uses, looked up at run time.
struct Lib {
    _va: *mut c_void,
    _drm: *mut c_void,
    get_display_drm: unsafe extern "C" fn(c_int) -> Dpy,
    initialize: unsafe extern "C" fn(Dpy, *mut c_int, *mut c_int) -> St,
    terminate: unsafe extern "C" fn(Dpy) -> St,
    set_info_callback: unsafe extern "C" fn(Dpy, Option<unsafe extern "C" fn(*mut c_void, *const c_char)>, *mut c_void) -> *mut c_void,
    set_error_callback: unsafe extern "C" fn(Dpy, Option<unsafe extern "C" fn(*mut c_void, *const c_char)>, *mut c_void) -> *mut c_void,
    error_str: unsafe extern "C" fn(St) -> *const c_char,
    max_num_entrypoints: unsafe extern "C" fn(Dpy) -> c_int,
    query_config_entrypoints: unsafe extern "C" fn(Dpy, c_int, *mut c_int, *mut c_int) -> St,
    get_config_attributes: unsafe extern "C" fn(Dpy, c_int, c_int, *mut VAConfigAttrib, c_int) -> St,
    create_config: unsafe extern "C" fn(Dpy, c_int, c_int, *mut VAConfigAttrib, c_int, *mut VAConfigID) -> St,
    destroy_config: unsafe extern "C" fn(Dpy, VAConfigID) -> St,
    create_surfaces:
        unsafe extern "C" fn(Dpy, c_uint, c_uint, c_uint, *mut VASurfaceID, c_uint, *mut c_void, c_uint) -> St,
    destroy_surfaces: unsafe extern "C" fn(Dpy, *mut VASurfaceID, c_int) -> St,
    create_context:
        unsafe extern "C" fn(Dpy, VAConfigID, c_int, c_int, c_int, *mut VASurfaceID, c_int, *mut VAContextID) -> St,
    destroy_context: unsafe extern "C" fn(Dpy, VAContextID) -> St,
    create_buffer: unsafe extern "C" fn(Dpy, VAContextID, VABufferType, c_uint, c_uint, *mut c_void, *mut VABufferID) -> St,
    destroy_buffer: unsafe extern "C" fn(Dpy, VABufferID) -> St,
    map_buffer: unsafe extern "C" fn(Dpy, VABufferID, *mut *mut c_void) -> St,
    unmap_buffer: unsafe extern "C" fn(Dpy, VABufferID) -> St,
    begin_picture: unsafe extern "C" fn(Dpy, VAContextID, VASurfaceID) -> St,
    render_picture: unsafe extern "C" fn(Dpy, VAContextID, *mut VABufferID, c_int) -> St,
    end_picture: unsafe extern "C" fn(Dpy, VAContextID) -> St,
    sync_surface: unsafe extern "C" fn(Dpy, VASurfaceID) -> St,
    derive_image: unsafe extern "C" fn(Dpy, VASurfaceID, *mut VAImage) -> St,
    create_image: unsafe extern "C" fn(Dpy, *mut VAImageFormat, c_int, c_int, *mut VAImage) -> St,
    put_image: unsafe extern "C" fn(Dpy, VASurfaceID, VAImageID, c_int, c_int, c_uint, c_uint, c_int, c_int, c_uint, c_uint) -> St,
    destroy_image: unsafe extern "C" fn(Dpy, VAImageID) -> St,
}

unsafe fn open(name: &str) -> Option<*mut c_void> {
    let c = std::ffi::CString::new(name).ok()?;
    let h = libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
    (!h.is_null()).then_some(h)
}

unsafe fn sym<T: Copy>(h: *mut c_void, name: &str) -> Option<T> {
    let c = std::ffi::CString::new(name).ok()?;
    let p = libc::dlsym(h, c.as_ptr());
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller names a function of the type T.
    Some(std::mem::transmute_copy(&p))
}

impl Lib {
    fn load() -> Result<Lib, String> {
        unsafe {
            let va = open("libva.so.2").ok_or("this computer has no VA-API (libva)")?;
            let drm = open("libva-drm.so.2").ok_or("this computer has no VA-API for DRM (libva-drm)")?;
            let missing = |n: &str| format!("libva lacks {n}");
            macro_rules! f {
                ($h:expr, $n:literal) => {
                    sym($h, $n).ok_or_else(|| missing($n))?
                };
            }
            Ok(Lib {
                _va: va,
                _drm: drm,
                get_display_drm: f!(drm, "vaGetDisplayDRM"),
                initialize: f!(va, "vaInitialize"),
                terminate: f!(va, "vaTerminate"),
                set_info_callback: f!(va, "vaSetInfoCallback"),
                set_error_callback: f!(va, "vaSetErrorCallback"),
                error_str: f!(va, "vaErrorStr"),
                max_num_entrypoints: f!(va, "vaMaxNumEntrypoints"),
                query_config_entrypoints: f!(va, "vaQueryConfigEntrypoints"),
                get_config_attributes: f!(va, "vaGetConfigAttributes"),
                create_config: f!(va, "vaCreateConfig"),
                destroy_config: f!(va, "vaDestroyConfig"),
                create_surfaces: f!(va, "vaCreateSurfaces"),
                destroy_surfaces: f!(va, "vaDestroySurfaces"),
                create_context: f!(va, "vaCreateContext"),
                destroy_context: f!(va, "vaDestroyContext"),
                create_buffer: f!(va, "vaCreateBuffer"),
                destroy_buffer: f!(va, "vaDestroyBuffer"),
                map_buffer: f!(va, "vaMapBuffer"),
                unmap_buffer: f!(va, "vaUnmapBuffer"),
                begin_picture: f!(va, "vaBeginPicture"),
                render_picture: f!(va, "vaRenderPicture"),
                end_picture: f!(va, "vaEndPicture"),
                sync_surface: f!(va, "vaSyncSurface"),
                derive_image: f!(va, "vaDeriveImage"),
                create_image: f!(va, "vaCreateImage"),
                put_image: f!(va, "vaPutImage"),
                destroy_image: f!(va, "vaDestroyImage"),
            })
        }
    }

    fn check(&self, what: &str, s: St) -> Result<(), String> {
        if s == VA_STATUS_SUCCESS as St {
            return Ok(());
        }
        let text = unsafe { CStr::from_ptr((self.error_str)(s)) }.to_string_lossy().into_owned();
        Err(format!("VA-API {what}: {text}"))
    }
}

unsafe extern "C" fn quiet(_: *mut c_void, _: *const c_char) {}

/// What the encoder asks of the chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Power {
    /// The chip's low-power encoder where it has one (Intel), else the
    /// normal one.
    Battery,
    /// The normal encoder.
    Performance,
}

/// One H.264 encoder on the graphics chip.
pub struct Vaapi {
    lib: Lib,
    _node: std::fs::File,
    dpy: Dpy,
    config: VAConfigID,
    context: VAContextID,
    /// [input, reconstructed A, reconstructed B]
    surfaces: [VASurfaceID; 3],
    coded: VABufferID,
    width: usize,
    height: usize,
    mbs_w: u32,
    mbs_h: u32,
    fps: u32,
    bits_per_second: u32,
    level: u8,
    frame_num: u32,
    idr_id: u16,
    since_key: u32,
    key_next: bool,
    recon: usize,
    image: Option<VAImage>,
    derived: bool,
    /// The driver takes Kagaz's own sequence and picture headers.
    packed: bool,
    /// Which chip and entry point: for the record.
    pub described: String,
}

// The display handle is used from one thread at a time.
unsafe impl Send for Vaapi {}

const LOG2_MAX_FRAME_NUM: u32 = 8;

impl Vaapi {
    /// Opens the first render node's encoder at this size.
    pub fn open(width: usize, height: usize, fps: u32, bits_per_second: u32, power: Power) -> Result<Vaapi, String> {
        let lib = Lib::load()?;
        let node = (128..136)
            .filter_map(|n| std::fs::OpenOptions::new().read(true).write(true).open(format!("/dev/dri/renderD{n}")).ok())
            .next()
            .ok_or("no graphics render node (/dev/dri/renderD*) can be opened")?;
        unsafe {
            let dpy = (lib.get_display_drm)(node.as_raw_fd());
            if dpy.is_null() {
                return Err("VA-API could not use the graphics render node".into());
            }
            (lib.set_info_callback)(dpy, Some(quiet), std::ptr::null_mut());
            (lib.set_error_callback)(dpy, Some(quiet), std::ptr::null_mut());
            let (mut major, mut minor) = (0, 0);
            lib.check("start", (lib.initialize)(dpy, &mut major, &mut minor))?;

            let profile = VAProfile::VAProfileH264ConstrainedBaseline as c_int;
            let mut eps = vec![0 as c_int; (lib.max_num_entrypoints)(dpy).max(1) as usize];
            let mut n = 0;
            let found = (lib.query_config_entrypoints)(dpy, profile, eps.as_mut_ptr(), &mut n);
            let eps = &eps[..n.max(0) as usize];
            let normal = VAEntrypoint::VAEntrypointEncSlice as c_int;
            let low = VAEntrypoint::VAEntrypointEncSliceLP as c_int;
            let entry = match power {
                Power::Battery if eps.contains(&low) => low,
                _ if eps.contains(&normal) => normal,
                _ if eps.contains(&low) => low,
                _ => {
                    (lib.terminate)(dpy);
                    let _ = found;
                    return Err("this graphics chip cannot encode H.264 through VA-API".into());
                }
            };

            let mut attrs = [
                VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribRTFormat, value: 0 },
                VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribRateControl, value: 0 },
                VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribEncPackedHeaders, value: 0 },
            ];
            lib.check("abilities", (lib.get_config_attributes)(dpy, profile, entry, attrs.as_mut_ptr(), 3))?;
            if std::env::var_os("KAGAZ_DEBUG").is_some() {
                eprintln!("va: entry {entry}, rt {:#x}, rc {:#x}, packed {:#x}", attrs[0].value, attrs[1].value, attrs[2].value);
            }
            if attrs[0].value & VA_RT_FORMAT_YUV420 == 0 {
                (lib.terminate)(dpy);
                return Err("the graphics chip's encoder takes no 4:2:0 pictures".into());
            }
            // Variable bitrate: a still screen costs next to nothing (constant
            // bitrate pads with filler, measured at 25 Mbit/s for a still
            // desktop on AMD).
            let rc = if attrs[1].value & VA_RC_VBR != 0 {
                VA_RC_VBR
            } else if attrs[1].value & VA_RC_CBR != 0 {
                VA_RC_CBR
            } else {
                VA_RC_CQP
            };
            // Kagaz writes the sequence and picture headers itself where the
            // driver takes them (AMD's leaves them out otherwise).
            let all = VA_ENC_PACKED_HEADER_SEQUENCE | VA_ENC_PACKED_HEADER_PICTURE | VA_ENC_PACKED_HEADER_SLICE;
            let packed = if attrs[2].value & all == all { all } else { 0 };
            let packed = if attrs[2].value == VA_ATTRIB_NOT_SUPPORTED { 0 } else { packed };
            let mut want = [
                VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribRTFormat, value: VA_RT_FORMAT_YUV420 },
                VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribRateControl, value: rc },
                VAConfigAttrib { type_: VAConfigAttribType_VAConfigAttribEncPackedHeaders, value: packed },
            ];
            let n = if packed != 0 { 3 } else { 2 };
            let mut config = 0;
            lib.check("set-up", (lib.create_config)(dpy, profile, entry, want.as_mut_ptr(), n, &mut config))?;

            let mut surfaces = [0 as VASurfaceID; 3];
            let s = (lib.create_surfaces)(
                dpy,
                VA_RT_FORMAT_YUV420,
                width as c_uint,
                height as c_uint,
                surfaces.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                0,
            );
            lib.check("pictures", s)?;
            let mut context = 0;
            lib.check(
                "encoder",
                (lib.create_context)(
                    dpy,
                    config,
                    width as c_int,
                    height as c_int,
                    VA_PROGRESSIVE as c_int,
                    surfaces.as_mut_ptr(),
                    3,
                    &mut context,
                ),
            )?;
            let mut coded = 0;
            let size = (width * height * 3 / 2).max(1 << 20) as c_uint;
            lib.check(
                "output buffer",
                (lib.create_buffer)(dpy, context, VABufferType_VAEncCodedBufferType, size, 1, std::ptr::null_mut(), &mut coded),
            )?;
            let level = match (height > 720, fps > 30) {
                (true, true) => 42,
                (true, false) => 40,
                (false, true) => 32,
                (false, false) => 31,
            };
            let described = format!(
                "VA-API {major}.{minor}, {} encoder, {}",
                if entry == low { "low-power" } else { "normal" },
                match rc {
                    VA_RC_CBR => "constant bitrate",
                    VA_RC_VBR => "variable bitrate",
                    _ => "fixed quality",
                }
            );
            Ok(Vaapi {
                lib,
                _node: node,
                dpy,
                config,
                context,
                surfaces,
                coded,
                width,
                height,
                mbs_w: width.div_ceil(16) as u32,
                mbs_h: height.div_ceil(16) as u32,
                fps,
                bits_per_second,
                level,
                frame_num: 0,
                idr_id: 0,
                since_key: 0,
                key_next: true,
                recon: 0,
                image: None,
                derived: true,
                packed: packed != 0,
                described,
            })
        }
    }

    pub fn key_next(&mut self) {
        self.key_next = true;
    }

    fn stream(&self) -> super::h264::Stream {
        super::h264::Stream {
            width: self.width,
            height: self.height,
            fps: self.fps,
            level_idc: self.level,
            log2_max_frame_num: LOG2_MAX_FRAME_NUM,
        }
    }

    /// A header Kagaz wrote, handed to the driver to put in the stream.
    fn packed_header(&self, kind: VAEncPackedHeaderType, bytes: &[u8], bits: usize) -> Result<[VABufferID; 2], String> {
        let p = VAEncPackedHeaderParameterBuffer {
            type_: kind,
            bit_length: bits as u32,
            has_emulation_bytes: 1,
            va_reserved: [0; 4],
        };
        let param = self.buffer(VABufferType_VAEncPackedHeaderParameterBufferType, &p)?;
        let mut id = 0;
        let s = unsafe {
            (self.lib.create_buffer)(
                self.dpy,
                self.context,
                VABufferType_VAEncPackedHeaderDataBufferType,
                bytes.len() as c_uint,
                1,
                bytes.as_ptr() as *mut c_void,
                &mut id,
            )
        };
        self.lib.check("headers", s)?;
        Ok([param, id])
    }

    fn buffer<T>(&self, kind: VABufferType, value: &T) -> Result<VABufferID, String> {
        let mut id = 0;
        let s = unsafe {
            (self.lib.create_buffer)(
                self.dpy,
                self.context,
                kind,
                std::mem::size_of::<T>() as c_uint,
                1,
                value as *const T as *mut c_void,
                &mut id,
            )
        };
        self.lib.check("parameters", s)?;
        Ok(id)
    }

    /// A misc parameter buffer: the type, then the structure.
    fn misc<T>(&self, kind: VAEncMiscParameterType, value: &T) -> Result<VABufferID, String> {
        let mut bytes = vec![0u8; 4 + std::mem::size_of::<T>()];
        bytes[..4].copy_from_slice(&kind.to_ne_bytes());
        // SAFETY: T is a plain C structure.
        let raw = unsafe { std::slice::from_raw_parts(value as *const T as *const u8, std::mem::size_of::<T>()) };
        bytes[4..].copy_from_slice(raw);
        let mut id = 0;
        let s = unsafe {
            (self.lib.create_buffer)(
                self.dpy,
                self.context,
                VABufferType_VAEncMiscParameterBufferType,
                bytes.len() as c_uint,
                1,
                bytes.as_mut_ptr() as *mut c_void,
                &mut id,
            )
        };
        self.lib.check("parameters", s)?;
        Ok(id)
    }

    /// Writes the RGBA picture into the input surface as NV12.
    fn upload(&mut self, rgba: &[u8]) -> Result<(), String> {
        let (w, h) = (self.width, self.height);
        unsafe {
            if self.image.is_none() {
                let mut img: VAImage = std::mem::zeroed();
                if (self.lib.derive_image)(self.dpy, self.surfaces[0], &mut img) == VA_STATUS_SUCCESS as St
                    && img.format.fourcc == VA_FOURCC_NV12
                {
                    self.derived = true;
                } else {
                    let mut fmt: VAImageFormat = std::mem::zeroed();
                    fmt.fourcc = VA_FOURCC_NV12;
                    fmt.byte_order = 1; // VA_LSB_FIRST
                    fmt.bits_per_pixel = 12;
                    img = std::mem::zeroed();
                    self.lib.check(
                        "picture upload",
                        (self.lib.create_image)(self.dpy, &mut fmt, w as c_int, h as c_int, &mut img),
                    )?;
                    self.derived = false;
                }
                self.image = Some(img);
            }
            let img = self.image.unwrap();
            let mut p: *mut c_void = std::ptr::null_mut();
            self.lib.check("picture upload", (self.lib.map_buffer)(self.dpy, img.buf, &mut p))?;
            let base = p as *mut u8;
            let y_plane =
                std::slice::from_raw_parts_mut(base.add(img.offsets[0] as usize), img.pitches[0] as usize * h);
            let uv_plane =
                std::slice::from_raw_parts_mut(base.add(img.offsets[1] as usize), img.pitches[1] as usize * h / 2);
            rgba_to_nv12(rgba, w, h, y_plane, img.pitches[0] as usize, uv_plane, img.pitches[1] as usize);
            self.lib.check("picture upload", (self.lib.unmap_buffer)(self.dpy, img.buf))?;
            if !self.derived {
                self.lib.check(
                    "picture upload",
                    (self.lib.put_image)(
                        self.dpy,
                        self.surfaces[0],
                        img.image_id,
                        0,
                        0,
                        w as c_uint,
                        h as c_uint,
                        0,
                        0,
                        w as c_uint,
                        h as c_uint,
                    ),
                )?;
            }
        }
        Ok(())
    }

    /// Encodes one RGBA picture; Annex B bytes, and whether it is a whole
    /// (key) picture.
    pub fn encode(&mut self, rgba: &[u8]) -> Result<(Vec<u8>, bool), String> {
        self.upload(rgba)?;
        let idr = self.key_next || self.since_key >= self.fps * 10;
        if idr {
            self.frame_num = 0;
            self.since_key = 0;
            self.key_next = false;
        }
        let cur = self.surfaces[1 + self.recon];
        let prev = self.surfaces[1 + (self.recon ^ 1)];
        let invalid = VAPictureH264 {
            picture_id: VA_INVALID_SURFACE,
            frame_idx: 0,
            flags: VA_PICTURE_H264_INVALID,
            TopFieldOrderCnt: 0,
            BottomFieldOrderCnt: 0,
            va_reserved: [0; 4],
        };
        let poc = (2 * self.frame_num) as i32;
        let reference = VAPictureH264 {
            picture_id: prev,
            frame_idx: self.frame_num.wrapping_sub(1),
            flags: VA_PICTURE_H264_SHORT_TERM_REFERENCE,
            TopFieldOrderCnt: poc - 2,
            BottomFieldOrderCnt: 0,
            va_reserved: [0; 4],
        };

        let mut buffers = Vec::new();
        unsafe {
            if idr {
                let mut seq: VAEncSequenceParameterBufferH264 = std::mem::zeroed();
                seq.level_idc = self.level;
                seq.intra_period = self.fps * 10;
                seq.intra_idr_period = self.fps * 10;
                seq.ip_period = 1;
                seq.bits_per_second = self.bits_per_second;
                seq.max_num_ref_frames = 1;
                seq.picture_width_in_mbs = self.mbs_w as u16;
                seq.picture_height_in_mbs = self.mbs_h as u16;
                seq.seq_fields.bits.set_chroma_format_idc(1);
                seq.seq_fields.bits.set_frame_mbs_only_flag(1);
                seq.seq_fields.bits.set_direct_8x8_inference_flag(1);
                seq.seq_fields.bits.set_log2_max_frame_num_minus4(LOG2_MAX_FRAME_NUM - 4);
                seq.seq_fields.bits.set_pic_order_cnt_type(2);
                let pad_h = self.mbs_h as usize * 16 - self.height;
                let pad_w = self.mbs_w as usize * 16 - self.width;
                if pad_h > 0 || pad_w > 0 {
                    seq.frame_cropping_flag = 1;
                    seq.frame_crop_bottom_offset = (pad_h / 2) as u32;
                    seq.frame_crop_right_offset = (pad_w / 2) as u32;
                }
                seq.vui_parameters_present_flag = 1;
                seq.vui_fields.bits.set_timing_info_present_flag(1);
                seq.vui_fields.bits.set_fixed_frame_rate_flag(1);
                seq.num_units_in_tick = 1;
                seq.time_scale = self.fps * 2;
                buffers.push(self.buffer(VABufferType_VAEncSequenceParameterBufferType, &seq)?);

                let mut rc: VAEncMiscParameterRateControl = std::mem::zeroed();
                rc.bits_per_second = self.bits_per_second;
                // With variable bitrate: up to the full rate in big changes.
                rc.target_percentage = 70;
                rc.window_size = 500;
                rc.initial_qp = 26;
                rc.min_qp = 10;
                rc.max_qp = 40;
                buffers.push(self.misc(VAEncMiscParameterType_VAEncMiscParameterTypeRateControl, &rc)?);
                let mut fr: VAEncMiscParameterFrameRate = std::mem::zeroed();
                fr.framerate = self.fps;
                buffers.push(self.misc(VAEncMiscParameterType_VAEncMiscParameterTypeFrameRate, &fr)?);
                let mut hrd: VAEncMiscParameterHRD = std::mem::zeroed();
                // Half a second of buffer: low delay.
                hrd.buffer_size = self.bits_per_second / 2;
                hrd.initial_buffer_fullness = self.bits_per_second / 4;
                buffers.push(self.misc(VAEncMiscParameterType_VAEncMiscParameterTypeHRD, &hrd)?);
                if self.packed {
                    let s = self.stream();
                    let sps = super::h264::sps(&s);
                    let pps = super::h264::pps();
                    buffers.extend(self.packed_header(VAEncPackedHeaderType_VAEncPackedHeaderSequence, &sps, sps.len() * 8)?);
                    buffers.extend(self.packed_header(VAEncPackedHeaderType_VAEncPackedHeaderPicture, &pps, pps.len() * 8)?);
                }
            }

            let mut pic: VAEncPictureParameterBufferH264 = std::mem::zeroed();
            pic.CurrPic = VAPictureH264 {
                picture_id: cur,
                frame_idx: self.frame_num,
                flags: 0,
                TopFieldOrderCnt: poc,
                BottomFieldOrderCnt: 0,
                va_reserved: [0; 4],
            };
            for r in pic.ReferenceFrames.iter_mut() {
                *r = invalid;
            }
            if !idr {
                pic.ReferenceFrames[0] = reference;
            }
            pic.coded_buf = self.coded;
            pic.frame_num = self.frame_num as u16;
            pic.pic_init_qp = 26;
            pic.pic_fields.bits.set_idr_pic_flag(idr as u32);
            pic.pic_fields.bits.set_reference_pic_flag(1);
            pic.pic_fields.bits.set_deblocking_filter_control_present_flag(1);
            buffers.push(self.buffer(VABufferType_VAEncPictureParameterBufferType, &pic)?);

            let mut slice: VAEncSliceParameterBufferH264 = std::mem::zeroed();
            slice.num_macroblocks = self.mbs_w * self.mbs_h;
            slice.macroblock_info = VA_INVALID_ID;
            slice.slice_type = if idr { 2 } else { 0 };
            slice.idr_pic_id = self.idr_id;
            for r in slice.RefPicList0.iter_mut().chain(slice.RefPicList1.iter_mut()) {
                *r = invalid;
            }
            if !idr {
                slice.RefPicList0[0] = reference;
            }
            buffers.push(self.buffer(VABufferType_VAEncSliceParameterBufferType, &slice)?);
            if self.packed {
                let (header, bits) = super::h264::slice_header(
                    &self.stream(),
                    idr,
                    self.frame_num,
                    self.idr_id as u32,
                    0,
                );
                buffers.extend(self.packed_header(VAEncPackedHeaderType_VAEncPackedHeaderSlice, &header, bits)?);
            }

            let l = &self.lib;
            let result = (|| {
                l.check("encode", (l.begin_picture)(self.dpy, self.context, self.surfaces[0]))?;
                l.check("encode", (l.render_picture)(self.dpy, self.context, buffers.as_mut_ptr(), buffers.len() as c_int))?;
                l.check("encode", (l.end_picture)(self.dpy, self.context))?;
                l.check("encode", (l.sync_surface)(self.dpy, self.surfaces[0]))?;
                let mut seg: *mut c_void = std::ptr::null_mut();
                l.check("encoded picture", (l.map_buffer)(self.dpy, self.coded, &mut seg))?;
                let mut out = Vec::new();
                let mut s = seg as *const VACodedBufferSegment;
                while !s.is_null() {
                    let part = &*s;
                    out.extend_from_slice(std::slice::from_raw_parts(part.buf as *const u8, part.size as usize));
                    s = part.next as *const VACodedBufferSegment;
                }
                l.check("encoded picture", (l.unmap_buffer)(self.dpy, self.coded))?;
                Ok::<_, String>(out)
            })();
            for b in buffers {
                (l.destroy_buffer)(self.dpy, b);
            }
            let out = result?;
            if idr {
                self.idr_id = self.idr_id.wrapping_add(1);
            }
            self.frame_num = (self.frame_num + 1) % (1 << LOG2_MAX_FRAME_NUM);
            self.since_key += 1;
            self.recon ^= 1;
            Ok((out, idr))
        }
    }
}

impl Drop for Vaapi {
    fn drop(&mut self) {
        unsafe {
            if let Some(img) = self.image.take() {
                (self.lib.destroy_image)(self.dpy, img.image_id);
            }
            (self.lib.destroy_buffer)(self.dpy, self.coded);
            (self.lib.destroy_context)(self.dpy, self.context);
            (self.lib.destroy_surfaces)(self.dpy, self.surfaces.as_mut_ptr(), 3);
            (self.lib.destroy_config)(self.dpy, self.config);
            (self.lib.terminate)(self.dpy);
        }
    }
}

/// RGBA to NV12 (BT.601, video range, as OpenH264 does), on several threads.
pub fn rgba_to_nv12(rgba: &[u8], w: usize, h: usize, y: &mut [u8], y_pitch: usize, uv: &mut [u8], uv_pitch: usize) {
    const THREADS: usize = 6;
    let pairs = h / 2;
    let band = pairs.div_ceil(THREADS).max(1);
    std::thread::scope(|s| {
        let mut ys = y.chunks_mut(band * 2 * y_pitch);
        let mut uvs = uv.chunks_mut(band * uv_pitch);
        for b in 0..pairs.div_ceil(band) {
            let (Some(yb), Some(uvb)) = (ys.next(), uvs.next()) else { break };
            s.spawn(move || {
                for k in 0..band {
                    let pair = b * band + k;
                    if pair >= pairs {
                        break;
                    }
                    for dy in 0..2 {
                        let row = &rgba[(pair * 2 + dy) * w * 4..(pair * 2 + dy + 1) * w * 4];
                        let out = &mut yb[(k * 2 + dy) * y_pitch..(k * 2 + dy) * y_pitch + w];
                        for (o, p) in out.iter_mut().zip(row.chunks_exact(4)) {
                            let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
                            *o = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
                        }
                    }
                    let top = &rgba[pair * 2 * w * 4..];
                    let bottom = &rgba[(pair * 2 + 1) * w * 4..];
                    let out = &mut uvb[k * uv_pitch..k * uv_pitch + w];
                    for x in (0..w).step_by(2) {
                        let i = x * 4;
                        let avg = |c: usize| {
                            (top[i + c] as i32 + top[i + 4 + c] as i32 + bottom[i + c] as i32 + bottom[i + 4 + c] as i32) / 4
                        };
                        let (r, g, b) = (avg(0), avg(1), avg(2));
                        out[x] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
                        out[x + 1] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nv12_of_white_and_black() {
        let (w, h) = (4, 4);
        let mut rgba = vec![255u8; w * h * 4];
        for px in rgba[..w * 2 * 4].chunks_exact_mut(4) {
            px[..3].copy_from_slice(&[0, 0, 0]);
        }
        let (mut y, mut uv) = (vec![0u8; w * h], vec![0u8; w * h / 2]);
        rgba_to_nv12(&rgba, w, h, &mut y, w, &mut uv, w);
        assert_eq!(y[0], 16);
        assert_eq!(y[w * 3], 235);
        assert!(uv.iter().all(|&c| (127..=129).contains(&c)));
    }

    /// On this computer's graphics chip, when it has one: whole pictures
    /// come out and decode. Skipped without VA-API.
    #[test]
    fn encodes_on_the_graphics_chip() {
        let Ok(mut v) = Vaapi::open(1280, 720, 30, 8_000_000, Power::Performance) else {
            eprintln!("skipped: no VA-API H.264 encoder here");
            return;
        };
        eprintln!("{}", v.described);
        let mut pattern = super::super::video::Pattern::new(1280, 720);
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let mut decoded = 0;
        let start = std::time::Instant::now();
        for t in 0..60 {
            let (bytes, key) = v.encode(pattern.draw(t * 33)).unwrap();
            if t < 2 {
                let types: Vec<u8> = openh264::nal_units(&bytes).map(|n| n.iter().skip_while(|&&b| b == 0).nth(1).map(|b| b & 0x1f).unwrap_or(99)).collect();
                eprintln!("picture {t}: {} bytes, NAL types {types:?}, starts {:02x?}", bytes.len(), &bytes[..bytes.len().min(12)]);
            }
            assert_eq!(key, t == 0, "picture {t}");
            for nal in openh264::nal_units(&bytes) {
                if let Ok(Some(_)) = decoder.decode(nal) {
                    decoded += 1;
                }
            }
        }
        eprintln!("60 pictures in {:?}, {decoded} decoded", start.elapsed());
        assert!(decoded >= 58, "{decoded} of 60 decoded");
    }
}

#[cfg(test)]
mod quality {
    use super::*;
    use crate::tv::miracast::video::quality::{desktop, psnr};

    #[test]
    #[ignore]
    fn measure_on_the_chip() {
        for (w, h, fps, bits) in [(1920usize, 1080usize, 30u32, 16_000_000u32), (1920, 1080, 60, 24_000_000), (1280, 720, 60, 14_000_000)] {
            let Ok(mut v) = Vaapi::open(w, h, fps, bits, Power::Performance) else { return };
            let mut d = openh264::decoder::Decoder::new().unwrap();
            let (mut total, mut worst, mut bytes, mut last) = (0f64, 0f64, 0usize, 0.0);
            for t in 0..60 {
                let src = desktop(w, h, t);
                let a = std::time::Instant::now();
                let (out, _) = v.encode(&src).unwrap();
                let took = a.elapsed().as_secs_f64();
                total += took;
                worst = f64::max(worst, took);
                bytes += out.len();
                for nal in openh264::nal_units(&out) {
                    if let Ok(Some(y)) = d.decode(nal) {
                        last = psnr(&src, w, h, &y);
                    }
                }
            }
            eprintln!(
                "{w}x{h}@{fps}: {:.1} ms a picture (worst {:.1}), {:.1} dB, {:.1} Mbit/s",
                total / 60.0 * 1e3,
                worst * 1e3,
                last,
                bytes as f64 * 8.0 / 1e6 * fps as f64 / 60.0
            );
        }
    }
}
