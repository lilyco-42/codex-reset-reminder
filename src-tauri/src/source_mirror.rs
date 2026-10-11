use serde::{Deserialize, Serialize};

use crate::error::{AppError, Result};
use crate::model::{StatusInfo, TrackerEvent};

/// 境内镜像：R2 自定义域名上的 data.json（Worker 每 10 分钟重写同一个对象）。
/// 实测 https://codexreset.lilyco42.top/data.json 在 cn-shanghai 返回 200；
/// 不要改用 lain42.top 的静态路径——那台 pingap 在 HTTP/1.1 上对静态文件同时回
/// Content-Length 与 Transfer-Encoding，严格的非浏览器客户端会直接读失败。
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
    /// 第二份夹具来自另一个独立生产者（Cloudflare Worker，JS 实现）。
    const CF: &str = include_str!("../tests/fixtures/mirror_data_cf.json");

    fn now_for_fixture() -> i64 {
        let v: serde_json::Value = serde_json::from_str(REAL).unwrap();
        v["generated_at_ms"].as_i64().unwrap() + 60_000
    }

    fn now_for_cf() -> i64 {
        let v: serde_json::Value = serde_json::from_str(CF).unwrap();
        v["generated_at_ms"].as_i64().unwrap() + 60_000
    }

    #[test]
    fn reads_the_real_server_generated_payload() {
        let d = parse_mirror(REAL, now_for_fixture()).expect("真实镜像应可解析");
        assert_eq!(d.events.len(), 38, "节点数应与抓取时实测一致");
        assert_eq!(d.events.iter().filter(|e| e.is_confirmed()).count(), 28);
        assert_eq!(d.events.iter().filter(|e| e.kind == "post").count(), 10);
        assert!(d.errors.is_empty());
        assert!(d.events.iter().all(|e| e.source_url.is_some()));
    }

    #[test]
    fn newest_confirmed_is_the_independently_measured_reset() {
        // events[0] 只是"最新一条"，可能是动态；全局重置要看 confirmed 里最新的那条。
        let d = parse_mirror(REAL, now_for_fixture()).unwrap();
        assert_eq!(d.events[0].kind, "post", "夹具里最新一条确实是动态");
        let c = d
            .events
            .iter()
            .find(|e| e.is_confirmed())
            .expect("应有 confirmed 节点");
        assert_eq!(c.date_time, "2026-10-07T03:35:09.000Z");
        assert_eq!(c.ts_ms, 1_791_344_109_000);
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
    fn reads_the_cloudflare_produced_payload_too() {
        let d = parse_mirror(CF, now_for_cf()).expect("第二个生产者的产物也应可读");
        assert_eq!(d.events.len(), 38);
        assert_eq!(d.events.iter().filter(|e| e.is_confirmed()).count(), 28);
        assert!(d.errors.is_empty());
        assert_eq!(d.status.expect("有状态").codex, "operational");
    }

    #[test]
    fn both_producers_share_one_contract() {
        // 两个独立实现（Python 在境内机、JS 在 Cloudflare）必须给同一套字段，
        // 否则读取端迟早只认其中一边。
        use std::collections::BTreeSet;
        let keys = |raw: &str| -> BTreeSet<String> {
            let v: serde_json::Value = serde_json::from_str(raw).unwrap();
            v.as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>()
        };
        let event_keys = |raw: &str| -> BTreeSet<String> {
            let v: serde_json::Value = serde_json::from_str(raw).unwrap();
            v["events"][0]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>()
        };
        assert_eq!(keys(REAL), keys(CF), "顶层字段必须一致");
        assert_eq!(event_keys(REAL), event_keys(CF), "事件字段必须一致");

        // 逐条内容也要一致（时间戳不同没关系，节点集合必须相同）。
        let ids = |raw: &str| -> BTreeSet<String> {
            let v: serde_json::Value = serde_json::from_str(raw).unwrap();
            v["events"]
                .as_array()
                .unwrap()
                .iter()
                .map(|e| {
                    format!(
                        "{}|{}",
                        e["date_time"].as_str().unwrap_or(""),
                        e["kind"].as_str().unwrap_or("")
                    )
                })
                .collect()
        };
        assert_eq!(ids(REAL), ids(CF), "两个生产者抓到的节点集合应相同");
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
