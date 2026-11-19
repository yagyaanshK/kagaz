//! H.264 in an NVIDIA graphics chip on Linux, through NVENC (NVIDIA's own
//! driver has no VA-API encoder).
//!
//! The driver's libraries (libcuda and libnvidia-encode) are opened only
//! when this encoder is asked for, so Kagaz starts on any computer. Same
//! stream as the other encoders: Constrained Baseline, one reference, P
//! pictures only, no reordering, a whole picture when the TV asks; the
//! pictures are turned into NV12 by Kagaz itself (BT.601 video range, as
//! the headers say) so colours match the other encoders.
//!
//! Not yet run on an NVIDIA chip: written to NVIDIA's header and samples,
//! and it reports plainly when the driver refuses something.

use super::nv_sys::*;
use super::vaapi::{rgba_to_nv12, Power};
use std::ffi::{c_char, c_int, c_uint, c_void, CStr};

type CuResult = c_int;

/// The CUDA driver functions NVENC needs: a context on the first chip.
struct Cuda {
    _lib: *mut c_void,
    ctx_destroy: unsafe extern "C" fn(*mut c_void) -> CuResult,
}

/// libnvidia-encode's two entry points.
struct Lib {
    _lib: *mut c_void,
    api: NV_ENCODE_API_FUNCTION_LIST,
}

unsafe fn open(name: &CStr) -> Option<*mut c_void> {
    let h = libc::dlopen(name.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
    (!h.is_null()).then_some(h)
}

unsafe fn sym<T: Copy>(h: *mut c_void, name: &CStr) -> Option<T> {
    let p = libc::dlsym(h, name.as_ptr());
    // SAFETY: the caller names a function of the type T.
    (!p.is_null()).then(|| std::mem::transmute_copy(&p))
}

/// A structure's version word (nvEncodeAPI.h, NVENCAPI_STRUCT_VERSION).
const fn ver(n: u32) -> u32 {
    NVENCAPI_VERSION | (n << 16) | (0x7 << 28)
}
const FUNCTION_LIST_VER: u32 = ver(2);
const OPEN_SESSION_VER: u32 = ver(1);
const PRESET_CONFIG_VER: u32 = ver(4) | (1 << 31);
const CONFIG_VER: u32 = ver(8) | (1 << 31);
const INITIALIZE_VER: u32 = ver(5) | (1 << 31);
const CREATE_INPUT_VER: u32 = ver(1);
const CREATE_BITSTREAM_VER: u32 = ver(1);
const LOCK_INPUT_VER: u32 = ver(1);
const LOCK_BITSTREAM_VER: u32 = ver(2);
const PIC_PARAMS_VER: u32 = ver(6) | (1 << 31);
const RC_PARAMS_VER: u32 = ver(1);
const RECONFIGURE_VER: u32 = ver(1) | (1 << 31);
const INFINITE: u32 = 0xffff_ffff;

const fn guid(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> GUID {
    GUID { Data1: d1, Data2: d2, Data3: d3, Data4: d4 }
}
const H264: GUID = guid(0x6bc82762, 0x4e63, 0x4ca4, [0xaa, 0x85, 0x1e, 0x50, 0xf3, 0x21, 0xf6, 0xbf]);
const BASELINE: GUID = guid(0x0727bcaa, 0x78c4, 0x4c83, [0x8c, 0x2f, 0xef, 0x3d, 0xff, 0x26, 0x7c, 0x6a]);
/// The fastest preset (battery) and a middle one (performance).
const PRESET_P1: GUID = guid(0xfc0a8d3e, 0x45f8, 0x4cf8, [0x80, 0xc7, 0x29, 0x88, 0x71, 0x59, 0x0e, 0xbf]);
const PRESET_P4: GUID = guid(0x90a7b826, 0xdf06, 0x4862, [0xb9, 0xd2, 0xcd, 0x6d, 0x73, 0xa0, 0x86, 0x81]);

/// One H.264 encoder on an NVIDIA chip.
pub struct Nvenc {
    cuda: Cuda,
    lib: Lib,
    ctx: *mut c_void,
    enc: *mut c_void,
    input: NV_ENC_INPUT_PTR,
    output: NV_ENC_OUTPUT_PTR,
    width: usize,
    height: usize,
    frame: u64,
    key_next: bool,
    /// The settings the session was opened with, kept for a new rate
    /// (`init` points at `config`, so both stay boxed).
    config: Box<NV_ENC_CONFIG>,
    init: Box<NV_ENC_INITIALIZE_PARAMS>,
    fps: u32,
    /// For the record: the chip's encoder and its setting.
    pub described: String,
}

// The session is used from one thread at a time.
unsafe impl Send for Nvenc {}

impl Nvenc {
    /// Opens the first NVIDIA chip's encoder at this size.
    pub fn open(width: usize, height: usize, fps: u32, bits_per_second: u32, power: Power) -> Result<Nvenc, String> {
        unsafe {
            let cu = open(c"libcuda.so.1").ok_or("no NVIDIA driver (libcuda)")?;
            let enc_lib = open(c"libnvidia-encode.so.1").ok_or("no NVIDIA encoder library (libnvidia-encode)")?;
            let missing = |n: &str| format!("the NVIDIA driver lacks {n}");
            let cu_init: unsafe extern "C" fn(c_uint) -> CuResult = sym(cu, c"cuInit").ok_or_else(|| missing("cuInit"))?;
            let device_get: unsafe extern "C" fn(*mut c_int, c_int) -> CuResult =
                sym(cu, c"cuDeviceGet").ok_or_else(|| missing("cuDeviceGet"))?;
            let ctx_create: unsafe extern "C" fn(*mut *mut c_void, c_uint, c_int) -> CuResult =
                sym(cu, c"cuCtxCreate_v2").ok_or_else(|| missing("cuCtxCreate_v2"))?;
            let ctx_destroy: unsafe extern "C" fn(*mut c_void) -> CuResult =
                sym(cu, c"cuCtxDestroy_v2").ok_or_else(|| missing("cuCtxDestroy_v2"))?;
            let max_version: unsafe extern "C" fn(*mut u32) -> NVENCSTATUS =
                sym(enc_lib, c"NvEncodeAPIGetMaxSupportedVersion").ok_or_else(|| missing("NvEncodeAPIGetMaxSupportedVersion"))?;
            let create_instance: unsafe extern "C" fn(*mut NV_ENCODE_API_FUNCTION_LIST) -> NVENCSTATUS =
                sym(enc_lib, c"NvEncodeAPICreateInstance").ok_or_else(|| missing("NvEncodeAPICreateInstance"))?;

            // The driver must speak API 12.0 (driver 520 or newer).
            let mut supported = 0u32;
            max_version(&mut supported);
            if supported < (12 << 4) {
                return Err(format!(
                    "the NVIDIA driver is too old for Kagaz's encoder (API {}.{}, 12.0 wanted)",
                    supported >> 4,
                    supported & 0xf
                ));
            }
            if cu_init(0) != 0 {
                return Err("the NVIDIA chip could not be started (cuInit)".into());
            }
            let mut dev = 0;
            if device_get(&mut dev, 0) != 0 {
                return Err("no NVIDIA chip found".into());
            }
            let mut ctx = std::ptr::null_mut();
            if ctx_create(&mut ctx, 0, dev) != 0 {
                return Err("the NVIDIA chip refused a context".into());
            }
            let cuda = Cuda { _lib: cu, ctx_destroy };
            let mut api: NV_ENCODE_API_FUNCTION_LIST = std::mem::zeroed();
            api.version = FUNCTION_LIST_VER;
            if create_instance(&mut api) != _NVENCSTATUS_NV_ENC_SUCCESS {
                (cuda.ctx_destroy)(ctx);
                return Err("the NVIDIA encoder could not be reached".into());
            }
            let lib = Lib { _lib: enc_lib, api };
            let mut v = Nvenc {
                cuda,
                lib,
                ctx,
                enc: std::ptr::null_mut(),
                input: std::ptr::null_mut(),
                output: std::ptr::null_mut(),
                width,
                height,
                frame: 0,
                key_next: true,
                config: Box::new(std::mem::zeroed()),
                init: Box::new(std::mem::zeroed()),
                fps,
                described: String::new(),
            };
            v.start(fps, bits_per_second, power)?;
            Ok(v)
        }
    }

    fn check(&self, what: &str, s: NVENCSTATUS) -> Result<(), String> {
        if s == _NVENCSTATUS_NV_ENC_SUCCESS {
            return Ok(());
        }
        let detail = unsafe {
            match (self.enc.is_null(), self.lib.api.nvEncGetLastErrorString) {
                (false, Some(f)) => {
                    let p: *const c_char = f(self.enc);
                    if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() }
                }
                _ => String::new(),
            }
        };
        Err(format!("NVENC {what}: error {s}{}{detail}", if detail.is_empty() { "" } else { ", " }))
    }

    unsafe fn start(&mut self, fps: u32, bits_per_second: u32, power: Power) -> Result<(), String> {
        let f = &self.lib.api;
        let call = |name: &str| format!("the NVIDIA encoder lacks {name}");
        let open_ex = f.nvEncOpenEncodeSessionEx.ok_or_else(|| call("nvEncOpenEncodeSessionEx"))?;
        let preset_ex = f.nvEncGetEncodePresetConfigEx.ok_or_else(|| call("nvEncGetEncodePresetConfigEx"))?;
        let init = f.nvEncInitializeEncoder.ok_or_else(|| call("nvEncInitializeEncoder"))?;
        let create_input = f.nvEncCreateInputBuffer.ok_or_else(|| call("nvEncCreateInputBuffer"))?;
        let create_output = f.nvEncCreateBitstreamBuffer.ok_or_else(|| call("nvEncCreateBitstreamBuffer"))?;

        let mut p: NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS = std::mem::zeroed();
        p.version = OPEN_SESSION_VER;
        p.deviceType = _NV_ENC_DEVICE_TYPE_NV_ENC_DEVICE_TYPE_CUDA;
        p.device = self.ctx;
        p.apiVersion = NVENCAPI_VERSION;
        let mut enc = std::ptr::null_mut();
        self.check("session", open_ex(&mut p, &mut enc))?;
        self.enc = enc;

        let (preset, preset_name) = match power {
            Power::Battery => (PRESET_P1, "fastest preset"),
            Power::Performance => (PRESET_P4, "balanced preset"),
        };
        let tuning = NV_ENC_TUNING_INFO_NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;
        let mut pc: NV_ENC_PRESET_CONFIG = std::mem::zeroed();
        pc.version = PRESET_CONFIG_VER;
        pc.presetCfg.version = CONFIG_VER;
        self.check("preset", preset_ex(self.enc, H264, preset, tuning, &mut pc))?;
        let mut cfg = pc.presetCfg;
        cfg.version = CONFIG_VER;
        cfg.profileGUID = BASELINE;
        cfg.gopLength = INFINITE;
        cfg.frameIntervalP = 1;
        cfg.rcParams.version = RC_PARAMS_VER;
        cfg.rcParams.rateControlMode = _NV_ENC_PARAMS_RC_MODE_NV_ENC_PARAMS_RC_VBR;
        cfg.rcParams.averageBitRate = bits_per_second;
        cfg.rcParams.maxBitRate = bits_per_second / 2 * 3;
        // About one picture's worth: low delay.
        cfg.rcParams.vbvBufferSize = bits_per_second / fps.max(1);
        cfg.rcParams.vbvInitialDelay = cfg.rcParams.vbvBufferSize;
        cfg.rcParams.set_zeroReorderDelay(1);
        let h = &mut cfg.encodeCodecConfig.h264Config;
        h.idrPeriod = INFINITE;
        h.maxNumRefFrames = 1;
        h.entropyCodingMode = _NV_ENC_H264_ENTROPY_CODING_MODE_NV_ENC_H264_ENTROPY_CODING_MODE_CAVLC;
        h.set_repeatSPSPPS(1);
        h.set_outputAUD(0);
        let vui = &mut h.h264VUIParameters;
        vui.videoSignalTypePresentFlag = 1;
        vui.videoFormat = _NV_ENC_VUI_VIDEO_FORMAT_NV_ENC_VUI_VIDEO_FORMAT_UNSPECIFIED;
        vui.videoFullRangeFlag = 0;
        vui.colourDescriptionPresentFlag = 1;
        vui.colourPrimaries = _NV_ENC_VUI_COLOR_PRIMARIES_NV_ENC_VUI_COLOR_PRIMARIES_SMPTE170M;
        vui.transferCharacteristics = _NV_ENC_VUI_TRANSFER_CHARACTERISTIC_NV_ENC_VUI_TRANSFER_CHARACTERISTIC_SMPTE170M;
        vui.colourMatrix = _NV_ENC_VUI_MATRIX_COEFFS_NV_ENC_VUI_MATRIX_COEFFS_SMPTE170M;

        *self.config = cfg;
        let mut ip: NV_ENC_INITIALIZE_PARAMS = std::mem::zeroed();
        ip.version = INITIALIZE_VER;
        ip.encodeGUID = H264;
        ip.presetGUID = preset;
        ip.encodeWidth = self.width as u32;
        ip.encodeHeight = self.height as u32;
        ip.darWidth = self.width as u32;
        ip.darHeight = self.height as u32;
        ip.maxEncodeWidth = self.width as u32;
        ip.maxEncodeHeight = self.height as u32;
        ip.frameRateNum = fps;
        ip.frameRateDen = 1;
        ip.enablePTD = 1;
        ip.encodeConfig = &mut *self.config;
        ip.tuningInfo = tuning;
        ip.bufferFormat = _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_NV12;
        *self.init = ip;
        let initialised = init(self.enc, &mut *self.init);
        self.check("set-up", initialised)?;

        let mut ci: NV_ENC_CREATE_INPUT_BUFFER = std::mem::zeroed();
        ci.version = CREATE_INPUT_VER;
        ci.width = self.width as u32;
        ci.height = self.height as u32;
        ci.bufferFmt = _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_NV12;
        self.check("picture buffer", create_input(self.enc, &mut ci))?;
        self.input = ci.inputBuffer;
        let mut co: NV_ENC_CREATE_BITSTREAM_BUFFER = std::mem::zeroed();
        co.version = CREATE_BITSTREAM_VER;
        self.check("output buffer", create_output(self.enc, &mut co))?;
        self.output = co.bitstreamBuffer;
        self.described = format!("NVENC 12.0, {preset_name}, variable bitrate");
        Ok(())
    }

    pub fn key_next(&mut self) {
        self.key_next = true;
    }

    /// A new rate for the running session (NVENC's reconfiguration, no
    /// whole picture). Not yet tried on an NVIDIA chip.
    pub fn set_bit_rate(&mut self, bits_per_second: u32) -> bool {
        let Some(reconfigure) = self.lib.api.nvEncReconfigureEncoder else { return false };
        let rc = &mut self.config.rcParams;
        rc.averageBitRate = bits_per_second;
        rc.maxBitRate = bits_per_second / 2 * 3;
        rc.vbvBufferSize = bits_per_second / self.fps.max(1);
        rc.vbvInitialDelay = rc.vbvBufferSize;
        self.init.encodeConfig = &mut *self.config;
        // SAFETY: the session is open; the parameters point at the kept
        // configuration, which lives as long as the session.
        unsafe {
            let mut r: NV_ENC_RECONFIGURE_PARAMS = std::mem::zeroed();
            r.version = RECONFIGURE_VER;
            r.reInitEncodeParams = *self.init;
            r.set_resetEncoder(0);
            r.set_forceIDR(0);
            reconfigure(self.enc, &mut r) == _NVENCSTATUS_NV_ENC_SUCCESS
        }
    }

    /// Encodes one RGBA picture; Annex B bytes, and whether it is a whole
    /// (key) picture.
    pub fn encode(&mut self, rgba: &[u8]) -> Result<(Vec<u8>, bool), String> {
        let (w, h) = (self.width, self.height);
        let f = &self.lib.api;
        let missing = || "the NVIDIA encoder lacks a function it needs".to_string();
        unsafe {
            let mut li: NV_ENC_LOCK_INPUT_BUFFER = std::mem::zeroed();
            li.version = LOCK_INPUT_VER;
            li.inputBuffer = self.input;
            self.check("picture upload", f.nvEncLockInputBuffer.ok_or_else(missing)?(self.enc, &mut li))?;
            let pitch = li.pitch as usize;
            let base = li.bufferDataPtr as *mut u8;
            // NV12 as NVENC lays it out: the Y rows, then the UV rows.
            let y = std::slice::from_raw_parts_mut(base, pitch * h);
            let uv = std::slice::from_raw_parts_mut(base.add(pitch * h), pitch * h / 2);
            rgba_to_nv12(rgba, w, h, y, pitch, uv, pitch);
            self.check("picture upload", f.nvEncUnlockInputBuffer.ok_or_else(missing)?(self.enc, self.input))?;

            let key = std::mem::take(&mut self.key_next);
            let mut pp: NV_ENC_PIC_PARAMS = std::mem::zeroed();
            pp.version = PIC_PARAMS_VER;
            pp.inputWidth = w as u32;
            pp.inputHeight = h as u32;
            pp.inputPitch = pitch as u32;
            pp.inputBuffer = self.input;
            pp.outputBitstream = self.output;
            pp.bufferFmt = _NV_ENC_BUFFER_FORMAT_NV_ENC_BUFFER_FORMAT_NV12;
            pp.pictureStruct = _NV_ENC_PIC_STRUCT_NV_ENC_PIC_STRUCT_FRAME;
            pp.inputTimeStamp = self.frame;
            pp.frameIdx = self.frame as u32;
            if key {
                pp.encodePicFlags =
                    _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_FORCEIDR | _NV_ENC_PIC_FLAGS_NV_ENC_PIC_FLAG_OUTPUT_SPSPPS;
            }
            self.frame += 1;
            self.check("encoding", f.nvEncEncodePicture.ok_or_else(missing)?(self.enc, &mut pp))?;

            let mut lb: NV_ENC_LOCK_BITSTREAM = std::mem::zeroed();
            lb.version = LOCK_BITSTREAM_VER;
            lb.outputBitstream = self.output;
            self.check("output", f.nvEncLockBitstream.ok_or_else(missing)?(self.enc, &mut lb))?;
            let out = std::slice::from_raw_parts(lb.bitstreamBufferPtr as *const u8, lb.bitstreamSizeInBytes as usize)
                .to_vec();
            let idr = lb.pictureType == _NV_ENC_PIC_TYPE_NV_ENC_PIC_TYPE_IDR;
            self.check("output", f.nvEncUnlockBitstream.ok_or_else(missing)?(self.enc, self.output))?;
            Ok((out, idr))
        }
    }
}

impl Drop for Nvenc {
    fn drop(&mut self) {
        let f = &self.lib.api;
        unsafe {
            if !self.enc.is_null() {
                if let (false, Some(d)) = (self.input.is_null(), f.nvEncDestroyInputBuffer) {
                    d(self.enc, self.input);
                }
                if let (false, Some(d)) = (self.output.is_null(), f.nvEncDestroyBitstreamBuffer) {
                    d(self.enc, self.output);
                }
                if let Some(d) = f.nvEncDestroyEncoder {
                    d(self.enc);
                }
            }
            (self.cuda.ctx_destroy)(self.ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_words_match_the_header() {
        // NVENCAPI_STRUCT_VERSION(2) for API 12.0, and the config's with
        // its top bit.
        assert_eq!(FUNCTION_LIST_VER, 12 | (2 << 16) | (7 << 28));
        assert_eq!(CONFIG_VER, 12 | (8 << 16) | (7 << 28) | (1 << 31));
    }

    /// On an NVIDIA chip: whole pictures come out and decode. Skipped
    /// elsewhere (as on the laptop this was written on).
    #[test]
    fn encodes_on_an_nvidia_chip() {
        let mut v = match Nvenc::open(1280, 720, 30, 8_000_000, Power::Performance) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("skipped: {e}");
                return;
            }
        };
        let mut pattern = super::super::video::Pattern::new(1280, 720);
        let mut decoder = openh264::decoder::Decoder::new().unwrap();
        let mut decoded = 0;
        for t in 0..60 {
            let (bytes, key) = v.encode(pattern.draw(t * 33)).unwrap();
            assert_eq!(key, t == 0, "picture {t}");
            for nal in openh264::nal_units(&bytes) {
                if let Ok(Some(_)) = decoder.decode(nal) {
                    decoded += 1;
                }
            }
        }
        assert!(decoded >= 58, "{decoded} of 60 decoded");
    }
}
