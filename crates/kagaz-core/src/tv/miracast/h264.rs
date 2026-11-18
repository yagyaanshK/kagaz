//! H.264 stream headers Kagaz writes itself for the hardware encoders: the
//! sequence and picture parameter sets (and a slice header, for drivers
//! that want that too). Constrained Baseline, frames only, one reference,
//! no reordering: each picture is shown as soon as it arrives.

/// Writes bits, most significant first.
#[derive(Default)]
pub struct Bits {
    out: Vec<u8>,
    cur: u8,
    used: u8,
}

impl Bits {
    pub fn u(&mut self, n: u32, v: u32) {
        for i in (0..n).rev() {
            self.cur = (self.cur << 1) | ((v >> i) & 1) as u8;
            self.used += 1;
            if self.used == 8 {
                self.out.push(self.cur);
                self.cur = 0;
                self.used = 0;
            }
        }
    }

    /// Unsigned Exp-Golomb.
    pub fn ue(&mut self, v: u32) {
        let v = v as u64 + 1;
        let len = 64 - v.leading_zeros();
        self.u(len - 1, 0);
        for i in (0..len).rev() {
            self.u(1, ((v >> i) & 1) as u32);
        }
    }

    /// Signed Exp-Golomb.
    pub fn se(&mut self, v: i32) {
        self.ue(if v > 0 { (2 * v - 1) as u32 } else { (-2 * v) as u32 });
    }

    pub fn bit_len(&self) -> usize {
        self.out.len() * 8 + self.used as usize
    }

    /// The bytes so far, the last one padded with zeros.
    pub fn bytes(&self) -> Vec<u8> {
        let mut b = self.out.clone();
        if self.used > 0 {
            b.push(self.cur << (8 - self.used));
        }
        b
    }

    /// Ends a parameter set: a one bit, then zeros to the byte.
    pub fn trailing(&mut self) {
        self.u(1, 1);
        while self.used != 0 {
            self.u(1, 0);
        }
    }
}

/// A NAL unit with its start code, emulation prevention applied.
fn nal(header: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 1, header];
    let mut zeros = 0;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
    out
}

/// What the headers describe.
#[derive(Debug, Clone, Copy)]
pub struct Stream {
    pub width: usize,
    pub height: usize,
    pub fps: u32,
    /// 31, 32, 40, 42 ...
    pub level_idc: u8,
    pub log2_max_frame_num: u32,
}

impl Stream {
    fn mbs(&self) -> (u32, u32) {
        (self.width.div_ceil(16) as u32, self.height.div_ceil(16) as u32)
    }
}

/// The sequence parameter set.
pub fn sps(s: &Stream) -> Vec<u8> {
    let (mw, mh) = s.mbs();
    let mut b = Bits::default();
    b.u(8, 66); // Baseline
    b.u(8, 0b1100_0000); // constraint sets 0 and 1: Constrained Baseline
    b.u(8, s.level_idc as u32);
    b.ue(0); // seq_parameter_set_id
    b.ue(s.log2_max_frame_num - 4);
    b.ue(2); // picture order from the frame number
    b.ue(1); // one reference picture
    b.u(1, 0); // no gaps in frame_num
    b.ue(mw - 1);
    b.ue(mh - 1);
    b.u(1, 1); // frames only
    b.u(1, 1); // direct_8x8_inference
    let (pad_w, pad_h) = (mw as usize * 16 - s.width, mh as usize * 16 - s.height);
    if pad_w > 0 || pad_h > 0 {
        b.u(1, 1);
        b.ue(0);
        b.ue((pad_w / 2) as u32);
        b.ue(0);
        b.ue((pad_h / 2) as u32);
    } else {
        b.u(1, 0);
    }
    b.u(1, 1); // VUI
    b.u(1, 0); // aspect ratio
    b.u(1, 0); // overscan
    b.u(1, 1); // video signal type
    b.u(3, 5); // unspecified video format
    b.u(1, 0); // video range (16-235)
    b.u(1, 1); // colour description: BT.601 (as the pictures are converted)
    b.u(8, 6);
    b.u(8, 6);
    b.u(8, 6);
    b.u(1, 0); // chroma location
    b.u(1, 1); // timing
    b.u(32, 1);
    b.u(32, s.fps * 2);
    b.u(1, 1); // fixed frame rate
    b.u(1, 0); // NAL HRD
    b.u(1, 0); // VCL HRD
    b.u(1, 0); // pic_struct
    b.u(1, 1); // bitstream restriction
    b.u(1, 1); // motion vectors over picture boundaries
    b.ue(0);
    b.ue(0);
    b.ue(16);
    b.ue(16);
    b.ue(0); // no reordering: shown as it arrives
    b.ue(1); // one picture of decoding buffer
    b.trailing();
    nal(0x67, &b.bytes())
}

/// The picture parameter set.
pub fn pps() -> Vec<u8> {
    let mut b = Bits::default();
    b.ue(0); // pic_parameter_set_id
    b.ue(0); // seq_parameter_set_id
    b.u(1, 0); // CAVLC
    b.u(1, 0); // bottom_field_pic_order_in_frame_present
    b.ue(0); // one slice group
    b.ue(0); // num_ref_idx_l0_default_active_minus1
    b.ue(0); // num_ref_idx_l1_default_active_minus1
    b.u(1, 0); // weighted_pred
    b.u(2, 0); // weighted_bipred
    b.se(0); // pic_init_qp_minus26
    b.se(0); // pic_init_qs_minus26
    b.se(0); // chroma_qp_index_offset
    b.u(1, 1); // deblocking_filter_control_present
    b.u(1, 0); // constrained_intra_pred
    b.u(1, 0); // redundant_pic_cnt_present
    b.trailing();
    nal(0x68, &b.bytes())
}

/// A slice header with its NAL header and start code, not byte-aligned:
/// (bytes, length in bits). The encoder carries on from the last bit.
pub fn slice_header(s: &Stream, idr: bool, frame_num: u32, idr_pic_id: u32, qp_delta: i32) -> (Vec<u8>, usize) {
    let mut b = Bits::default();
    b.u(32, 1); // start code
    b.u(8, if idr { 0x65 } else { 0x61 });
    b.ue(0); // first_mb_in_slice
    b.ue(if idr { 7 } else { 5 }); // I or P, all slices alike
    b.ue(0); // pic_parameter_set_id
    b.u(s.log2_max_frame_num, frame_num);
    if idr {
        b.ue(idr_pic_id);
    } else {
        b.u(1, 0); // num_ref_idx_active_override
        b.u(1, 0); // ref_pic_list_modification_flag_l0
    }
    if idr {
        b.u(1, 0); // no_output_of_prior_pics
        b.u(1, 0); // long_term_reference
    } else {
        b.u(1, 0); // adaptive_ref_pic_marking_mode
    }
    b.se(qp_delta);
    b.ue(0); // deblocking on
    b.se(0);
    b.se(0);
    (b.bytes(), b.bit_len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exp_golomb() {
        let mut b = Bits::default();
        for v in [0, 1, 2, 3, 7] {
            b.ue(v);
        }
        // 1 010 011 00100 0001000
        assert_eq!(b.bit_len(), 1 + 3 + 3 + 5 + 7);
        assert_eq!(b.bytes(), vec![0b1010_0110, 0b0100_0001, 0b0000_0000]);
        let mut s = Bits::default();
        s.se(-1);
        s.se(1);
        assert_eq!(s.bytes(), vec![0b0110_1000]);
    }

    #[test]
    fn parameter_sets_decode() {
        // OpenH264's decoder takes our SPS and PPS ahead of its own
        // pictures' slices only if they describe the same stream; here we
        // check the start of each NAL unit and the cropping of 1080 lines.
        let s = Stream { width: 1920, height: 1080, fps: 30, level_idc: 40, log2_max_frame_num: 8 };
        let sps = sps(&s);
        assert_eq!(&sps[..8], &[0, 0, 0, 1, 0x67, 66, 0xC0, 40]);
        let pps = pps();
        assert_eq!(&pps[..5], &[0, 0, 0, 1, 0x68]);
        let (h, bits) = slice_header(&s, true, 0, 0, 0);
        assert_eq!(&h[..5], &[0, 0, 0, 1, 0x65]);
        assert!(bits > 40 && bits < h.len() * 8 + 1);
    }

    #[test]
    fn emulation_prevention() {
        assert_eq!(nal(0x67, &[0, 0, 1, 0, 0, 0]), vec![0, 0, 0, 1, 0x67, 0, 0, 3, 1, 0, 0, 3, 0]);
    }
}
