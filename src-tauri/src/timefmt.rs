use crate::model::Settings;

fn digit(byte: u8) -> Option<i64> {
    if byte.is_ascii_digit() {
        Some((byte - b'0') as i64)
    } else {
        None
    }
}

fn run_days(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 解析 `2026-10-07T03:35:09.000Z` 这类 ISO 串为 Unix 毫秒。
pub fn iso_to_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 {
        return None;
    }
    if b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, c: usize| -> Option<i64> {
        if c > b.len() {
            return None;
        }
        let mut v = 0i64;
        for i in a..c {
            v = v * 10 + digit(b[i])?;
        }
        Some(v)
    };
    let y = num(0, 4)?;
    let mo = num(5, 7)?;
    let d = num(8, 10)?;
    let h = num(11, 13)?;
    let mi = num(14, 16)?;
    let sec = num(17, 19)?;

    let mut idx = 19usize;
    let mut ms = 0i64;
    if b.len() > idx && b[idx] == b'.' {
        idx += 1;
        ms = num(idx, idx + 3)?;
        idx += 3;
    }
    let mut off_min = 0i64;
    if b.len() > idx {
        match b[idx] {
            b'Z' | b'z' => {}
            plus @ (b'+' | b'-') => {
                let sign = if plus == b'-' { -1 } else { 1 };
                if b.len() < idx + 6 || b[idx + 3] != b':' {
                    return None;
                }
                let oh = num(idx + 1, idx + 3)?;
                let om = num(idx + 4, idx + 6)?;
                off_min = sign * (oh * 60 + om);
            }
            _ => return None,
        }
    }
    if y < 1900 || mo < 1 || mo > 12 || d < 1 || d > 31 || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    let days = run_days(y, mo, d);
    Some((days * 86_400 + h * 3_600 + mi * 60 + sec) * 1_000 + ms - off_min * 60_000)
}

pub fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default()
}

pub fn local_date_of(now_ms: i64, tz_offset_minutes: i64) -> i64 {
    (now_ms.div_euclid(1_000) + tz_offset_minutes * 60).div_euclid(86_400)
}

pub fn local_hour_of(now_ms: i64, tz_offset_minutes: i64) -> u32 {
    let local = now_ms.div_euclid(1_000) + tz_offset_minutes * 60;
    local.div_euclid(3_600).rem_euclid(24) as u32
}

/// 用于通知正文的可读时间，按调用方传入的时区偏移渲染。
pub fn format_local(now_ms: i64, tz_offset_minutes: i64) -> String {
    let local = now_ms.div_euclid(1_000) + tz_offset_minutes * 60;
    let (y, m, d) = civil_from_days(local.div_euclid(86_400));
    let sod = local.rem_euclid(86_400);
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}",
        y,
        m,
        d,
        sod / 3_600,
        sod % 3_600 / 60
    )
}

pub fn human_delta(minutes: i64) -> String {
    let m = minutes.max(0);
    if m < 60 {
        format!("{m} 分钟")
    } else if m < 60 * 24 {
        format!("{} 小时 {} 分", m / 60, m % 60)
    } else {
        format!("{} 天 {} 小时", m / 1_440, m % 1_440 / 60)
    }
}

pub fn in_quiet_for(cfg: &Settings, now_ms: i64, tz_offset_minutes: i64) -> bool {
    if !cfg.quiet_hours || cfg.quiet_start == cfg.quiet_end {
        return false;
    }
    let h = local_hour_of(now_ms, tz_offset_minutes);
    if cfg.quiet_start < cfg.quiet_end {
        h >= cfg.quiet_start && h < cfg.quiet_end
    } else {
        // 跨零点：23 -> 7 表示 23:00 之后或 07:00 之前。
        h >= cfg.quiet_start || h < cfg.quiet_end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_matches_javascript_epoch_for_real_values() {
        let cases = [
            ("1970-01-01T00:00:00.000Z", 0i64),
            ("2026-01-01T00:00:00Z", 1_767_225_600_000),
            ("2000-02-29T12:30:00.000Z", 951_827_400_000),
            ("2026-06-29T00:00:00.000Z", 1_782_691_200_000),
            ("2026-10-07T03:35:09.000Z", 1_791_344_109_000),
            ("2026-10-10T12:26:51.807Z", 1_791_635_211_807),
        ];
        for (s, want) in cases {
            assert_eq!(iso_to_ms(s), Some(want), "{s}");
        }
    }

    #[test]
    fn offsets_and_leap_boundaries_are_honoured() {
        assert_eq!(
            iso_to_ms("2026-10-07T11:35:09.000+08:00"),
            Some(1_791_344_109_000)
        );
        assert_eq!(
            iso_to_ms("2026-10-07T03:35:09.000Z"),
            iso_to_ms("2026-10-06T22:35:09.000-05:00")
        );
    }

    #[test]
    fn malformed_input_returns_none_instead_of_panicking() {
        for s in [
            "",
            "x",
            "2026-10-07",
            "2026-10-07T03:35:09",
            "2026-13-07T00:00:00.000Z",
            "2026-10-07T25:00:00.000Z",
            "2026-10-07T03:35:09.Z",
            "2026-10-07T03:35:09+0800",
        ] {
            assert!(iso_to_ms(s).is_none(), "{s} 应解析失败");
        }
    }

    #[test]
    fn local_formatting_uses_the_passed_offset() {
        // 1791644400000 = 2026-10-10T15:00Z = 北京 23:00
        assert_eq!(format_local(1_791_644_400_000, 480), "2026-10-10 23:00");
        assert_eq!(format_local(1_791_644_400_000, 0), "2026-10-10 15:00");
        assert_eq!(local_hour_of(1_791_644_400_000, 480), 23);
        assert_eq!(
            local_date_of(1_791_644_400_000, 480),
            local_date_of(1_791_644_400_000 - 3_600_000, 480)
        );
    }

    #[test]
    fn quiet_window_wraps_past_midnight() {
        let cfg = Settings::default();
        assert!(cfg.quiet_start == 23 && cfg.quiet_end == 7);
        assert!(
            in_quiet_for(&cfg, 1_791_644_400_000, 480),
            "北京 23:00 静默"
        );
        assert!(
            in_quiet_for(&cfg, 1_791_644_400_000 + 4 * 3_600_000, 480),
            "北京 03:00 静默"
        );
        assert!(
            !in_quiet_for(&cfg, 1_791_644_400_000 + 8 * 3_600_000, 480),
            "北京 07:00 结束"
        );
        assert!(
            !in_quiet_for(&cfg, 1_791_644_400_000 - 4 * 3_600_000, 480),
            "北京 19:00 不静默"
        );
    }

    #[test]
    fn human_delta_reads_like_chinese_not_like_numbers() {
        assert_eq!(human_delta(7), "7 分钟");
        assert_eq!(human_delta(75), "1 小时 15 分");
        assert_eq!(human_delta(1_500), "1 天 1 小时");
        assert_eq!(human_delta(-3), "0 分钟");
    }
}
