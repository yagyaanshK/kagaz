//! Saving a live stream as an MP4 file while it plays: the camera's H.264
//! pictures as they came (nothing is encoded again) and the sound as plain
//! 16-bit samples ('sowt', which Android's players, VLC and QuickTime
//! read). The pictures and sound go to disk as they arrive; the index
//! that makes it a playable file is added when the recording stops.
//!
//! The file starts at the first full picture; sound before it is dropped.

use crate::media::{nal_type, nal_units, Media};
use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const VIDEO_SCALE: u32 = 90_000;
const AUDIO_RATE: u32 = 8_000;
const MOVIE_SCALE: u32 = 1_000;

struct Picture {
    offset: u64,
    size: u32,
    /// 90 kHz, from the first picture.
    at: u64,
    key: bool,
}

struct Sound {
    offset: u64,
    frames: u32,
}

/// A recording under way.
pub struct Recorder {
    out: BufWriter<File>,
    path: PathBuf,
    mdat_at: u64,
    pos: u64,
    sps: Vec<u8>,
    pps: Vec<u8>,
    pictures: Vec<Picture>,
    /// The last picture's timestamp as sent, and where that is in the file.
    last_ts: u32,
    last_at: u64,
    sounds: Vec<Sound>,
    /// Where the sound starts after the first picture (90 kHz).
    sound_starts: Option<u64>,
    first_ts: u32,
}

/// A finished recording.
#[derive(Debug, Clone)]
pub struct Saved {
    pub path: PathBuf,
    pub bytes: u64,
    pub seconds: f64,
}

impl Recorder {
    pub fn create(path: &Path) -> io::Result<Recorder> {
        let mut out = BufWriter::new(File::create(path)?);
        let ftyp = boxed(b"ftyp", &[b"isom".as_slice(), &0x200u32.to_be_bytes(), b"isom", b"iso2", b"avc1", b"mp41"].concat());
        out.write_all(&ftyp)?;
        let mdat_at = ftyp.len() as u64;
        // A 64-bit size, filled in at the end.
        out.write_all(&1u32.to_be_bytes())?;
        out.write_all(b"mdat")?;
        out.write_all(&0u64.to_be_bytes())?;
        Ok(Recorder {
            out,
            path: path.to_path_buf(),
            mdat_at,
            pos: mdat_at + 16,
            sps: Vec::new(),
            pps: Vec::new(),
            pictures: Vec::new(),
            last_ts: 0,
            last_at: 0,
            sounds: Vec::new(),
            sound_starts: None,
            first_ts: 0,
        })
    }

    /// How long it is so far, in seconds.
    pub fn seconds(&self) -> f64 {
        self.last_at as f64 / VIDEO_SCALE as f64
    }

    pub fn push(&mut self, m: &Media) -> io::Result<()> {
        match m {
            Media::Video { data, key, timestamp } => self.picture(data, *key, *timestamp),
            Media::Audio { pcm, timestamp } => self.sound(pcm, *timestamp),
            Media::Ended(_) => Ok(()),
        }
    }

    fn picture(&mut self, data: &[u8], key: bool, ts: u32) -> io::Result<()> {
        let mut body = Vec::with_capacity(data.len() + 8);
        for u in nal_units(data) {
            match nal_type(u) {
                7 if self.sps.is_empty() => self.sps = u.to_vec(),
                8 if self.pps.is_empty() => self.pps = u.to_vec(),
                7 | 8 | 9 => {}
                _ => {
                    body.extend_from_slice(&(u.len() as u32).to_be_bytes());
                    body.extend_from_slice(u);
                }
            }
        }
        if self.pictures.is_empty() {
            // Start on a full picture that carries its settings.
            if !key || self.sps.is_empty() || self.pps.is_empty() {
                self.sps.clear();
                self.pps.clear();
                return Ok(());
            }
            self.first_ts = ts;
            self.last_ts = ts;
        }
        if body.is_empty() {
            return Ok(());
        }
        // Timestamps wrap at 32 bits; one going backwards counts as no time.
        let step = ts.wrapping_sub(self.last_ts);
        if step < 1 << 31 {
            self.last_at += u64::from(step);
        }
        self.last_ts = ts;
        self.out.write_all(&body)?;
        self.pictures.push(Picture {
            offset: self.pos,
            size: body.len() as u32,
            at: self.last_at,
            key,
        });
        self.pos += body.len() as u64;
        Ok(())
    }

    fn sound(&mut self, pcm: &[i16], ts: u32) -> io::Result<()> {
        if self.pictures.is_empty() || pcm.is_empty() {
            return Ok(());
        }
        if self.sound_starts.is_none() {
            let after = ts.wrapping_sub(self.first_ts);
            self.sound_starts = Some(if after < 1 << 31 { u64::from(after) } else { 0 });
        }
        let mut bytes = Vec::with_capacity(pcm.len() * 2);
        for s in pcm {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        self.out.write_all(&bytes)?;
        self.sounds.push(Sound {
            offset: self.pos,
            frames: pcm.len() as u32,
        });
        self.pos += bytes.len() as u64;
        Ok(())
    }

    /// Add the index and close the file. A recording without a single
    /// picture is removed.
    pub fn finish(mut self) -> io::Result<Saved> {
        if self.pictures.is_empty() {
            drop(self.out);
            let _ = std::fs::remove_file(&self.path);
            return Err(io::Error::other("no picture arrived to record"));
        }
        self.out.flush()?;
        let file = self.out.get_mut();
        file.seek(SeekFrom::Start(self.mdat_at + 8))?;
        file.write_all(&(self.pos - self.mdat_at).to_be_bytes())?;
        file.seek(SeekFrom::Start(self.pos))?;
        let moov = self.moov();
        self.out.write_all(&moov)?;
        self.out.flush()?;
        let bytes = self.pos + moov.len() as u64;
        Ok(Saved {
            seconds: self.video_duration() as f64 / VIDEO_SCALE as f64,
            path: self.path,
            bytes,
        })
    }

    /// Each picture lasts until the next; the last as long as the one before.
    fn durations(&self) -> Vec<u32> {
        let n = self.pictures.len();
        let mut out: Vec<u32> = (1..n)
            .map(|i| (self.pictures[i].at - self.pictures[i - 1].at).clamp(1, u64::from(u32::MAX)) as u32)
            .collect();
        out.push(out.last().copied().unwrap_or(VIDEO_SCALE / 15));
        out
    }

    fn video_duration(&self) -> u64 {
        self.durations().iter().map(|d| u64::from(*d)).sum()
    }

    fn moov(&self) -> Vec<u8> {
        let (width, height) = sps_size(&self.sps).unwrap_or((1920, 1080));
        let video_len = self.video_duration();
        let frames: u64 = self.sounds.iter().map(|s| u64::from(s.frames)).sum();
        let sound_delay = self.sound_starts.unwrap_or(0) * u64::from(MOVIE_SCALE) / u64::from(VIDEO_SCALE);
        let movie_len = (video_len * u64::from(MOVIE_SCALE) / u64::from(VIDEO_SCALE))
            .max(sound_delay + frames * u64::from(MOVIE_SCALE) / u64::from(AUDIO_RATE));
        let mut traks = self.video_trak(width, height, video_len, movie_len);
        if frames > 0 {
            traks.extend(self.sound_trak(frames, sound_delay));
        }
        let mut mvhd = vec![0u8; 4];
        mvhd.extend([0u32, 0, MOVIE_SCALE, movie_len as u32].iter().flat_map(|v| v.to_be_bytes()));
        mvhd.extend(0x0001_0000u32.to_be_bytes());
        mvhd.extend(0x0100u16.to_be_bytes());
        mvhd.extend([0u8; 10]);
        mvhd.extend(MATRIX.iter().flat_map(|v| v.to_be_bytes()));
        mvhd.extend([0u8; 24]);
        mvhd.extend(3u32.to_be_bytes());
        boxed(b"moov", &[boxed(b"mvhd", &mvhd), traks].concat())
    }

    fn video_trak(&self, width: u16, height: u16, len: u64, movie_len: u64) -> Vec<u8> {
        let tkhd = tkhd(1, movie_len.min(video_movie(len)), 0, width, height);
        let mut avc1 = vec![0u8; 6];
        avc1.extend(1u16.to_be_bytes());
        avc1.extend([0u8; 16]);
        avc1.extend(width.to_be_bytes());
        avc1.extend(height.to_be_bytes());
        avc1.extend(0x0048_0000u32.to_be_bytes());
        avc1.extend(0x0048_0000u32.to_be_bytes());
        avc1.extend(0u32.to_be_bytes());
        avc1.extend(1u16.to_be_bytes());
        avc1.extend([0u8; 32]);
        avc1.extend(0x0018u16.to_be_bytes());
        avc1.extend(0xFFFFu16.to_be_bytes());
        let mut avcc = vec![1, self.sps[1], self.sps[2], self.sps[3], 0xFF, 0xE1];
        avcc.extend((self.sps.len() as u16).to_be_bytes());
        avcc.extend(&self.sps);
        avcc.push(1);
        avcc.extend((self.pps.len() as u16).to_be_bytes());
        avcc.extend(&self.pps);
        avc1.extend(boxed(b"avcC", &avcc));
        let stsd = full(&[1u32.to_be_bytes().as_slice(), &boxed(b"avc1", &avc1)].concat());

        let mut stts = Vec::new();
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for d in self.durations() {
            match runs.last_mut() {
                Some((n, last)) if *last == d => *n += 1,
                _ => runs.push((1, d)),
            }
        }
        stts.extend((runs.len() as u32).to_be_bytes());
        for (n, d) in runs {
            stts.extend(n.to_be_bytes());
            stts.extend(d.to_be_bytes());
        }
        let keys: Vec<u32> = (1..=self.pictures.len() as u32).filter(|i| self.pictures[*i as usize - 1].key).collect();
        let mut stss = (keys.len() as u32).to_be_bytes().to_vec();
        stss.extend(keys.iter().flat_map(|k| k.to_be_bytes()));
        let mut stsz = 0u32.to_be_bytes().to_vec();
        stsz.extend((self.pictures.len() as u32).to_be_bytes());
        stsz.extend(self.pictures.iter().flat_map(|p| p.size.to_be_bytes()));
        let stsc = [1u32, 1, 1, 1].iter().flat_map(|v| v.to_be_bytes()).collect::<Vec<_>>();
        let mut co64 = (self.pictures.len() as u32).to_be_bytes().to_vec();
        co64.extend(self.pictures.iter().flat_map(|p| p.offset.to_be_bytes()));
        let stbl = boxed(
            b"stbl",
            &[
                boxed(b"stsd", &stsd),
                boxed(b"stts", &full(&stts)),
                boxed(b"stss", &full(&stss)),
                boxed(b"stsc", &full(&stsc)),
                boxed(b"stsz", &full(&stsz)),
                boxed(b"co64", &full(&co64)),
            ]
            .concat(),
        );
        let vmhd = boxed(b"vmhd", &[0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
        let minf = boxed(b"minf", &[vmhd, dinf(), stbl].concat());
        let mdia = boxed(b"mdia", &[mdhd(VIDEO_SCALE, len), hdlr(b"vide", "VideoHandler"), minf].concat());
        boxed(b"trak", &[tkhd, mdia].concat())
    }

    fn sound_trak(&self, frames: u64, delay: u64) -> Vec<u8> {
        let len = frames * u64::from(MOVIE_SCALE) / u64::from(AUDIO_RATE);
        let tkhd = tkhd(2, delay + len, 0x0100, 0, 0);
        // Sound that starts after the first picture waits for it.
        let edts = if delay > 0 {
            let mut elst = 2u32.to_be_bytes().to_vec();
            elst.extend((delay as u32).to_be_bytes());
            elst.extend((-1i32).to_be_bytes());
            elst.extend(0x0001_0000u32.to_be_bytes());
            elst.extend((len as u32).to_be_bytes());
            elst.extend(0u32.to_be_bytes());
            elst.extend(0x0001_0000u32.to_be_bytes());
            boxed(b"edts", &boxed(b"elst", &full(&elst)))
        } else {
            Vec::new()
        };
        let mut sowt = vec![0u8; 6];
        sowt.extend(1u16.to_be_bytes());
        sowt.extend([0u8; 8]); // version, revision, vendor
        sowt.extend(1u16.to_be_bytes()); // channels
        sowt.extend(16u16.to_be_bytes()); // bits
        sowt.extend([0u8; 4]); // compression id, packet size
        sowt.extend((AUDIO_RATE << 16).to_be_bytes());
        let stsd = full(&[1u32.to_be_bytes().as_slice(), &boxed(b"sowt", &sowt)].concat());
        let mut stts = 1u32.to_be_bytes().to_vec();
        stts.extend((frames as u32).to_be_bytes());
        stts.extend(1u32.to_be_bytes());
        // Chunks of the same size share a line.
        let mut runs: Vec<(u32, u32)> = Vec::new();
        for (i, s) in self.sounds.iter().enumerate() {
            if runs.last().map(|r| r.1) != Some(s.frames) {
                runs.push((i as u32 + 1, s.frames));
            }
        }
        let mut stsc = (runs.len() as u32).to_be_bytes().to_vec();
        for (first, n) in runs {
            stsc.extend(first.to_be_bytes());
            stsc.extend(n.to_be_bytes());
            stsc.extend(1u32.to_be_bytes());
        }
        let mut stsz = 2u32.to_be_bytes().to_vec();
        stsz.extend((frames as u32).to_be_bytes());
        let mut co64 = (self.sounds.len() as u32).to_be_bytes().to_vec();
        co64.extend(self.sounds.iter().flat_map(|s| s.offset.to_be_bytes()));
        let stbl = boxed(
            b"stbl",
            &[
                boxed(b"stsd", &stsd),
                boxed(b"stts", &full(&stts)),
                boxed(b"stsc", &full(&stsc)),
                boxed(b"stsz", &full(&stsz)),
                boxed(b"co64", &full(&co64)),
            ]
            .concat(),
        );
        let smhd = boxed(b"smhd", &[0u8; 8]);
        let minf = boxed(b"minf", &[smhd, dinf(), stbl].concat());
        let mdia = boxed(b"mdia", &[mdhd(AUDIO_RATE, frames), hdlr(b"soun", "SoundHandler"), minf].concat());
        boxed(b"trak", &[tkhd, edts, mdia].concat())
    }
}

const MATRIX: [u32; 9] = [0x0001_0000, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000];

fn video_movie(len: u64) -> u64 {
    len * u64::from(MOVIE_SCALE) / u64::from(VIDEO_SCALE)
}

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len() + 8);
    b.extend(((body.len() + 8) as u32).to_be_bytes());
    b.extend(kind);
    b.extend(body);
    b
}

/// A full box's body: version 0, no flags.
fn full(body: &[u8]) -> Vec<u8> {
    [&[0u8; 4][..], body].concat()
}

fn tkhd(id: u32, duration: u64, volume: u16, width: u16, height: u16) -> Vec<u8> {
    let mut b = vec![0, 0, 0, 3]; // enabled, in the movie
    b.extend([0u32, 0, id, 0, duration as u32].iter().flat_map(|v| v.to_be_bytes()));
    b.extend([0u8; 8]);
    b.extend([0u8; 4]); // layer, alternate group
    b.extend(volume.to_be_bytes());
    b.extend([0u8; 2]);
    b.extend(MATRIX.iter().flat_map(|v| v.to_be_bytes()));
    b.extend((u32::from(width) << 16).to_be_bytes());
    b.extend((u32::from(height) << 16).to_be_bytes());
    boxed(b"tkhd", &b)
}

fn mdhd(scale: u32, duration: u64) -> Vec<u8> {
    let mut b = vec![0u8; 4];
    b.extend([0u32, 0, scale, duration as u32].iter().flat_map(|v| v.to_be_bytes()));
    b.extend(0x55C4u16.to_be_bytes()); // "und"
    b.extend([0u8; 2]);
    boxed(b"mdhd", &b)
}

fn hdlr(kind: &[u8; 4], name: &str) -> Vec<u8> {
    let mut b = vec![0u8; 8];
    b.extend(kind);
    b.extend([0u8; 12]);
    b.extend(name.as_bytes());
    b.push(0);
    boxed(b"hdlr", &b)
}

fn dinf() -> Vec<u8> {
    let url = boxed(b"url ", &[0, 0, 0, 1]);
    let dref = boxed(b"dref", &[full(&1u32.to_be_bytes()), url].concat());
    boxed(b"dinf", &dref)
}

/// The picture's size from its sequence parameter set.
fn sps_size(sps: &[u8]) -> Option<(u16, u16)> {
    // Without the bytes that only keep start codes out of the data.
    let mut raw = Vec::with_capacity(sps.len());
    let mut zeros = 0;
    for &b in sps.get(1..)? {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        raw.push(b);
    }
    let mut r = Bits { data: &raw, at: 0 };
    let profile = r.bits(8)?;
    r.bits(16)?; // constraints, level
    r.ue()?;
    let mut chroma = 1;
    if [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135].contains(&profile) {
        chroma = r.ue()?;
        if chroma == 3 {
            r.bits(1)?;
        }
        r.ue()?;
        r.ue()?;
        r.bits(1)?;
        if r.bits(1)? == 1 {
            for i in 0..if chroma == 3 { 12 } else { 8 } {
                if r.bits(1)? == 1 {
                    let size = if i < 6 { 16 } else { 64 };
                    let (mut last, mut next) = (8i64, 8i64);
                    for _ in 0..size {
                        if next != 0 {
                            next = (last + r.se()? + 256) % 256;
                        }
                        if next != 0 {
                            last = next;
                        }
                    }
                }
            }
        }
    }
    r.ue()?;
    match r.ue()? {
        0 => {
            r.ue()?;
        }
        1 => {
            r.bits(1)?;
            r.se()?;
            r.se()?;
            for _ in 0..r.ue()? {
                r.se()?;
            }
        }
        _ => {}
    }
    r.ue()?;
    r.bits(1)?;
    let w = r.ue()? + 1;
    let h = r.ue()? + 1;
    let frame_only = r.bits(1)?;
    if frame_only == 0 {
        r.bits(1)?;
    }
    r.bits(1)?;
    let (mut cl, mut cr, mut ct, mut cb) = (0, 0, 0, 0);
    if r.bits(1)? == 1 {
        cl = r.ue()?;
        cr = r.ue()?;
        ct = r.ue()?;
        cb = r.ue()?;
    }
    let (ux, uy) = if chroma == 0 { (1, 2 - frame_only) } else { (2, 2 * (2 - frame_only)) };
    let width = w * 16 - ux * (cl + cr);
    let height = (2 - frame_only) * h * 16 - uy * (ct + cb);
    Some((u16::try_from(width).ok()?, u16::try_from(height).ok()?))
}

struct Bits<'a> {
    data: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn bits(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.data.get(self.at / 8)?;
            v = (v << 1) | u32::from((byte >> (7 - self.at % 8)) & 1);
            self.at += 1;
        }
        Some(v)
    }
    fn ue(&mut self) -> Option<u32> {
        let mut zeros = 0;
        while self.bits(1)? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        Some((1u32 << zeros) - 1 + self.bits(zeros)?)
    }
    fn se(&mut self) -> Option<i64> {
        let k = i64::from(self.ue()?);
        Some(if k % 2 == 1 { (k + 1) / 2 } else { -(k / 2) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 640x360 Baseline SPS and its PPS, as x264 writes them.
    const SPS: [u8; 25] = [
        0x67, 0x42, 0xC0, 0x16, 0xD9, 0x00, 0xA0, 0x2F, 0xF9, 0x70, 0x11, 0x00, 0x00, 0x03, 0x00, 0x01, 0x00, 0x00, 0x03,
        0x00, 0x1E, 0x0F, 0x16, 0x2E, 0x48,
    ];
    const PPS: [u8; 5] = [0x68, 0xCB, 0x83, 0xCB, 0x20];
    // 1920x1080 High, cropped from 1088 lines.
    const SPS_HIGH: [u8; 27] = [
        0x67, 0x64, 0x00, 0x28, 0xAC, 0xD9, 0x40, 0x78, 0x02, 0x27, 0xE5, 0xC0, 0x44, 0x00, 0x00, 0x03, 0x00, 0x04, 0x00,
        0x00, 0x03, 0x00, 0x78, 0x3C, 0x60, 0xC6, 0x58,
    ];

    fn picture(key: bool, ts: u32) -> Media {
        let mut data = Vec::new();
        if key {
            data.extend([0, 0, 0, 1]);
            data.extend(SPS);
            data.extend([0, 0, 0, 1]);
            data.extend(PPS);
        }
        data.extend([0, 0, 0, 1, if key { 0x65 } else { 0x41 }, 1, 2, 3]);
        Media::Video { data, key, timestamp: ts }
    }

    #[test]
    fn reads_the_picture_size() {
        assert_eq!(sps_size(&SPS), Some((640, 360)));
        assert_eq!(sps_size(&SPS_HIGH), Some((1920, 1080)));
    }

    #[test]
    fn writes_a_file_that_starts_on_a_full_picture() {
        let dir = std::env::temp_dir().join(format!("kagaz-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.mp4");
        let mut r = Recorder::create(&path).unwrap();
        r.push(&picture(false, 0)).unwrap(); // before any full picture: dropped
        r.push(&Media::Audio { pcm: vec![1; 160], timestamp: 0 }).unwrap(); // likewise
        for i in 0..30u32 {
            r.push(&picture(i % 10 == 0, 9000 + i * 6000)).unwrap();
            r.push(&Media::Audio { pcm: vec![i as i16; 320], timestamp: 9000 + i * 6000 }).unwrap();
        }
        let saved = r.finish().unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(saved.bytes, bytes.len() as u64);
        assert!((saved.seconds - 2.0).abs() < 0.01, "{}", saved.seconds);
        // The top-level boxes cover the file exactly.
        let mut at = 0usize;
        let mut kinds = Vec::new();
        while at < bytes.len() {
            let mut size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as u64;
            if size == 1 {
                size = u64::from_be_bytes(bytes[at + 8..at + 16].try_into().unwrap());
            }
            kinds.push(String::from_utf8_lossy(&bytes[at + 4..at + 8]).into_owned());
            at += size as usize;
        }
        assert_eq!(at, bytes.len());
        assert_eq!(kinds, ["ftyp", "mdat", "moov"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn nothing_recorded_leaves_no_file() {
        let path = std::env::temp_dir().join(format!("kagaz-mp4-empty-{}.mp4", std::process::id()));
        let mut r = Recorder::create(&path).unwrap();
        r.push(&picture(false, 0)).unwrap();
        assert!(r.finish().is_err());
        assert!(!path.exists());
    }
}
