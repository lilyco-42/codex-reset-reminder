use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;

use crate::model::{LocalSnapshot, RateLimits};

/// 实测本机 rollout 会话文件可达 1.85 GB，所以只能尾部读取，不能整文件加载。
const TAIL_BYTES: usize = 4 * 1024 * 1024;
/// 目录遍历上限，避免异常目录把扫描拖死。
const MAX_SCAN_ENTRIES: usize = 4_000;

pub fn default_codex_home() -> Option<PathBuf> {
    if let Ok(v) = std::env::var("CODEX_HOME") {
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    for key in ["HOME", "USERPROFILE"] {
        if let Ok(v) = std::env::var(key) {
            if !v.is_empty() {
                return Some(PathBuf::from(v).join(".codex"));
            }
        }
    }
    None
}

fn mtime_ms(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .map(|t| match t.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => d.as_millis() as i64,
            Err(_) => 0,
        })
        .unwrap_or(0)
}

fn newest_session_file(sessions: &Path, budget: &mut usize) -> Option<PathBuf> {
    let mut best: Option<(i64, PathBuf)> = None;
    let mut stack: Vec<PathBuf> = vec![sessions.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if *budget == 0 {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if *budget == 0 {
                break;
            }
            *budget -= 1;
            let path = entry.path();
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                stack.push(path);
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".jsonl") {
                continue;
            }
            let t = mtime_ms(&path);
            let take = match &best {
                Some((bt, _)) => t > *bt,
                None => true,
            };
            if take {
                best = Some((t, path));
            }
        }
    }
    best.map(|(_, p)| p)
}

fn pick_rate_limits(v: &Value) -> Option<RateLimits> {
    let payload = v.get("payload")?;
    let rl = payload
        .get("rate_limits")
        .or_else(|| payload.get("info")?.get("rate_limits"))?;
    serde_json::from_value::<RateLimits>(rl.clone()).ok()
}

#[derive(Debug, Default)]
pub struct TailScan {
    pub dropped_partial: usize,
    pub marked: usize,
    pub parsed: usize,
    pub json_fail: usize,
    pub shape_miss: usize,
    pub found: Option<RateLimits>,
}

/// 尾部读取，从最后一行往前找第一条能解析出 rate_limits 的记录。
/// 首行可能被截断，所以丢弃。
pub fn parse_tail(text: &str) -> TailScan {
    let mut scan = TailScan::default();
    let lines: Vec<&str> = text.split('\n').filter(|l| !l.trim().is_empty()).collect();
    if lines.len() > 1 {
        scan.dropped_partial = 1;
    }
    let drop = scan.dropped_partial;
    for line in lines.iter().rev().skip(drop) {
        if !line.contains("rate_limits") {
            continue;
        }
        scan.marked += 1;
        match serde_json::from_str::<Value>(line) {
            Ok(v) => match pick_rate_limits(&v) {
                Some(rl) => {
                    scan.parsed += 1;
                    scan.found = Some(rl);
                    return scan;
                }
                None => scan.shape_miss += 1,
            },
            Err(_) => scan.json_fail += 1,
        }
    }
    scan
}

fn tail_read(path: &Path) -> std::io::Result<(String, i64)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len() as i64;
    let take = size.min(TAIL_BYTES as i64).max(0) as usize;
    let mut buf = vec![0u8; take];
    if take > 0 {
        file.seek(SeekFrom::End(-(take as i64)))?;
        file.read_exact(&mut buf)?;
    }
    Ok((String::from_utf8_lossy(&buf).into_owned(), size))
}

pub fn read_local(codex_home: &Path, now_ms: i64) -> LocalSnapshot {
    let mut budget = MAX_SCAN_ENTRIES;
    let mut picked: Option<PathBuf> = None;
    for d in ["sessions", "archived_sessions"] {
        let p = codex_home.join(d);
        if p.is_dir() {
            if let Some(f) = newest_session_file(&p, &mut budget) {
                picked = Some(f);
                break;
            }
        }
    }
    let Some(path) = picked else {
        return LocalSnapshot::unavailable(
            now_ms,
            format!(
                "没找到会话文件，检查过 {}",
                codex_home.join("sessions").display()
            ),
        );
    };
    let path_str = path.display().to_string();
    let (text, size) = match tail_read(&path) {
        Ok(v) => v,
        Err(e) => {
            let mut s = LocalSnapshot::unavailable(now_ms, format!("读取失败：{e}"));
            s.path = Some(path_str);
            return s;
        }
    };
    let scan = parse_tail(&text);
    match scan.found {
        Some(limits) => LocalSnapshot {
            captured_at_ms: now_ms,
            source: "local".into(),
            reason: None,
            path: Some(path_str),
            file_bytes: size,
            limits: Some(limits),
        },
        None => {
            let reason = if scan.marked == 0 {
                "尾部 4 MB 没有额度记录，可能刚重启过 Codex".to_string()
            } else {
                format!(
                    "找到 {} 条额度标记，解析成功 {} 条，结构不符 {} 条，JSON 失败 {} 条",
                    scan.marked, scan.parsed, scan.shape_miss, scan.json_fail
                )
            };
            LocalSnapshot {
                captured_at_ms: now_ms,
                source: "unavailable".into(),
                reason: Some(reason),
                path: Some(path_str),
                file_bytes: size,
                limits: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(payload: Value) -> String {
        serde_json::to_string(
            &json!({ "timestamp": "2026-10-10T12:00:00.000Z", "payload": payload }),
        )
        .unwrap()
    }

    fn rl(used: f64, resets: i64, window: i64) -> Value {
        json!({ "rate_limits": {
            "limit_id": "codex",
            "primary": { "used_percent": used, "window_minutes": window, "resets_at": resets },
            "secondary": null,
            "credits": null
        } })
    }

    #[test]
    fn tail_prefers_the_newest_block_not_the_oldest() {
        let text = format!(
            "{}\n{}\n{}\n",
            line(rl(3.0, 111, 300)),
            "noise line without markers",
            line(rl(77.0, 222, 300))
        );
        let scan = parse_tail(&text);
        let got = scan.found.expect("应解析出额度");
        assert_eq!(got.primary.expect("primary").used_percent, 77.0);
        assert_eq!(scan.marked, 2, "两条都应被标记命中");
        assert_eq!(scan.parsed, 1, "只有最后一条被真正解析就够");
    }

    #[test]
    fn truncated_first_line_is_dropped_not_fatal() {
        let whole = line(rl(12.0, 333, 10_080));
        let mut text = String::new();
        text.push_str(&whole[0..40]);
        text.push('\n');
        text.push_str(&whole);
        text.push('\n');
        let scan = parse_tail(&text);
        assert!(scan.found.is_some(), "截断首行不应影响结果");
        assert_eq!(scan.dropped_partial, 1);
    }

    #[test]
    fn rate_limits_nested_under_info_is_also_accepted() {
        let v: Value = serde_json::from_str(
            r#"{"payload":{"info":{"rate_limits":{"limit_id":"codex","primary":{"used_percent":5.0,"window_minutes":300,"resets_at":9}}}}}"#,
        )
        .unwrap();
        assert!(pick_rate_limits(&v).is_some());
    }

    #[test]
    fn markers_without_shape_are_counted_not_swallowed() {
        let scan = parse_tail("{\"payload\":{\"rate_limits\":[]}}\n");
        assert_eq!(scan.marked, 1);
        assert_eq!(scan.shape_miss, 1);
        assert!(scan.found.is_none());
    }

    #[test]
    fn real_live_fixture_still_parses_through_the_reader() {
        let raw = include_str!("../tests/fixtures/rate_limits_live.json");
        let v: Value = serde_json::from_str(raw).expect("fixture 必须是合法 JSON");
        let first = v["blocks"][0]["rate_limits"].clone();
        let line = serde_json::to_string(&json!({ "payload": first })).unwrap();
        let scan = parse_tail(&format!("{line}\n"));
        let got = scan.found.expect("真实 payload 应可解析");
        assert_eq!(got.limit_id, "codex");
        let p = got.primary.expect("primary");
        assert_eq!(p.window_minutes, 300);
        assert_eq!(p.resets_at, 1_791_650_325, "应与本机实测值一致");
        assert_eq!(got.credits.expect("credits").balance, "0");
    }
}
