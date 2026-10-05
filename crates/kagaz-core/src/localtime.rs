//! The wall-clock time, for naming files. Tiny platform shims instead of a
//! date-time crate, since all that is needed is "now, in local time".

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl LocalTime {
    /// "2026-10-05-1432", the stamp used in default scan file names.
    pub fn file_stamp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}-{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute
        )
    }
}

/// The current local time; falls back to UTC where the platform shim is missing.
pub fn now() -> LocalTime {
    platform_now().unwrap_or_else(utc_now)
}

#[cfg(unix)]
fn platform_now() -> Option<LocalTime> {
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    // SAFETY: localtime_r writes into the tm we own and reads the time_t we pass.
    let ok = unsafe { !libc::localtime_r(&t, &mut tm).is_null() };
    ok.then(|| LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u32,
        day: tm.tm_mday as u32,
        hour: tm.tm_hour as u32,
        minute: tm.tm_min as u32,
        second: tm.tm_sec as u32,
    })
}

#[cfg(windows)]
fn platform_now() -> Option<LocalTime> {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetLocalTime(out: *mut SystemTime);
    }
    let mut st = SystemTime::default();
    // SAFETY: GetLocalTime only writes the struct we pass.
    unsafe { GetLocalTime(&mut st) };
    Some(LocalTime {
        year: st.year as i32,
        month: st.month as u32,
        day: st.day as u32,
        hour: st.hour as u32,
        minute: st.minute as u32,
        second: st.second as u32,
    })
}

#[cfg(not(any(unix, windows)))]
fn platform_now() -> Option<LocalTime> {
    None
}

fn utc_now() -> LocalTime {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    LocalTime {
        year: (if m <= 2 { y + 1 } else { y }) as i32,
        month: m,
        day: d,
        hour: (rem / 3_600) as u32,
        minute: (rem % 3_600 / 60) as u32,
        second: (rem % 60) as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_is_sortable() {
        let t = LocalTime {
            year: 2026,
            month: 10,
            day: 5,
            hour: 14,
            minute: 32,
            second: 9,
        };
        assert_eq!(t.file_stamp(), "2026-10-05-1432");
    }

    #[test]
    fn utc_fallback_is_sane() {
        let t = utc_now();
        assert!(t.year >= 2026);
        assert!((1..=12).contains(&t.month));
        assert!((1..=31).contains(&t.day));
        assert!(t.hour < 24 && t.minute < 60 && t.second < 60);
        let local = now();
        assert!((local.year - t.year).abs() <= 1);
    }
}
