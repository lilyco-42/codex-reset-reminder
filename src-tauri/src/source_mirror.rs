use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::model::{StatusInfo, TrackerEvent};

/// 境内镜像（lain42.top 上的同步脚本产出的 data.json）。
/// 注意它和直接抓 codexreset.org 是两条路：镜像返回的是**已解析好的 JSON**，
/// 所以这里不做 HTML 解析，只做结构校验。
const STALE_MS: i64 = 6 * 3_600_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MirrorData {
    #[serde(default)]
    pub generated_at_ms: i64,
    #[serde(default)]
    pub events: Vec<TrackerEvent>,
    #[serde(default)]
    pub status: Option<StatusInfo>,
    #[serde(default)]
    pub errors: Vec<String>,
}

pub fn parse_mirror(json: &str, now_ms: i64) -> Result<MirrorData> {
    if json.trim().is_empty() {
        return Err(AppError::Parse("镜像返回空内容".into()));
    }
    let data: MirrorData =
        serde_json::from_str(json).map_err(|e| AppError::Parse(format!("镜像结构变化：{e}")))?;
    if data.events.is_empty() {
        return Err(AppError::Parse("镜像里没有任何时间线节点".into()));
    }
    let age = now_ms - data.generated_at_ms;
    if data.generated_at_ms <= 0 || age > STALE_MS {
        return Err(AppError::Parse(format!(
            "镜像数据已 {} 分钟未更新，同步任务可能停了",
            (age / 60_000).max(0)
        )));
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 夹具由服务器上 Python 同步脚本真实产出，不是手打的：
    /// 这样 Rust 读取端和 Python 写入端不可能各说各话。
    const REAL: &str = include_str!("../tests/fixtures/mirror_data.json");

    fn now_for_fixture() -> i64 {
        let v: serde_json::Value = serde_json::from_str(REAL).unwrap();
        v["generated_at_ms"].as_i64().unwrap() + 60_000
    }

    #[test]
    fn reads_the_real_server_generated_payload() {
        let d = parse_mirror(REAL, now_for_fixture()).expect("真实镜像应可解析");
        assert_eq!(d.events.len(), 37, "节点数应与实测一致");
        assert_eq!(d.events.iter().filter(|e| e.is_confirmed()).count(), 28);
        assert_eq!(d.events.iter().filter(|e| e.kind == "post").count(), 9);
        assert!(d.errors.is_empty());
        assert!(d.events.iter().all(|e| e.source_url.is_some()));
    }

    #[test]
    fn newest_event_matches_the_independently_measured_reset() {
        let d = parse_mirror(REAL, now_for_fixture()).unwrap();
        let first = &d.events[0];
        assert_eq!(first.date_time, "2026-10-07T03:35:09.000Z");
        assert_eq!(first.ts_ms, 1_791_344_109_000);
        assert!(first.is_confirmed());
    }

    #[test]
    fn events_arrive_newest_first_and_status_is_readable() {
        let d = parse_mirror(REAL, now_for_fixture()).unwrap();
        assert!(d.events.windows(2).all(|w| w[0].ts_ms >= w[1].ts_ms));
        let s = d.status.expect("镜像带状态字段");
        assert_eq!(s.codex, "operational");
        assert!(!s.in_trouble(), "当时服务正常，不该报警");
    }

    #[test]
    fn stale_mirror_is_rejected_so_the_app_falls_back() {
        // 同步任务停了的时候，镜像会一直供旧数据；不卡时效就等于静默漏报。
        let gen = now_for_fixture();
        let e = parse_mirror(REAL, gen + STALE_MS + 60_000).expect_err("过期镜像必须拒绝");
        assert!(e.to_string().contains("未更新"), "文案要说明原因：{e}");
    }

    #[test]
    fn empty_or_broken_payloads_are_errors_not_silence() {
        let n = now_for_fixture();
        assert!(parse_mirror("", n).is_err());
        assert!(parse_mirror("<html>502</html>", n).is_err());
        assert!(parse_mirror(r#"{"error":"boom","retry_after":30}"#, n).is_err());
        assert!(parse_mirror(r#"{"generated_at_ms":1,"events":[]}"#, n).is_err());
        assert!(parse_mirror(
            r#"{"generated_at_ms":0,"events":[{"date_time":"x","ts_ms":1,"kind":"post"}]}"#,
            n
        )
        .is_err());
    }

    #[test]
    fn unknown_extra_fields_are_tolerated() {
        let n = now_for_fixture();
        let body = format!(
            r#"{{"generated_at_ms":{},"future":{{"a":1}},"events":[{{"date_time":"2026-10-07T03:35:09.000Z","ts_ms":1791344109000,"kind":"confirmed","source_url":"https://x.com/a","brand_new":true}}]}}"#,
            n - 1000
        );
        let d = parse_mirror(&body, n).expect("多出来的字段不该让解析失败");
        assert_eq!(d.events.len(), 1);
        assert!(d.status.is_none(), "缺 status 应为 None 而不是报错");
    }
}
