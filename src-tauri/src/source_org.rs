use crate::model::TrackerEvent;
use crate::timefmt::iso_to_ms;

const MARK: &str = "data-testid=\"reset-timeline-item\"";

fn html_unescape(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

/// 只取标签内部的属性，不越过当前标签的右边界。
fn attr(tag: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = tag.find(&key)? + key.len();
    let end = tag[start..].find('"')?;
    let raw = &tag[start..start + end];
    if raw.is_empty() {
        None
    } else {
        Some(html_unescape(raw))
    }
}

/// 解析 codexreset.org 的服务端渲染时间线。
/// 实测结构：`<div ... data-datetime=.. data-kind=confirmed|post data-source-url=.. data-testid="reset-timeline-item">`
pub fn parse_tracker(html: &str) -> Vec<TrackerEvent> {
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = html[from..].find(MARK) {
        let pos = from + rel;
        let start = html[..pos].rfind('<').unwrap_or(0);
        let end = html[pos..].find('>').map(|x| pos + x).unwrap_or(html.len());
        let tag = &html[start..end.max(start)];
        if let (Some(date_time), Some(kind)) = (attr(tag, "data-datetime"), attr(tag, "data-kind"))
        {
            if let Some(ts_ms) = iso_to_ms(&date_time) {
                out.push(TrackerEvent {
                    date_time,
                    ts_ms,
                    kind,
                    source_url: attr(tag, "data-source-url"),
                });
            }
        }
        from = pos + MARK.len();
    }
    out.sort_by(|a, b| b.ts_ms.cmp(&a.ts_ms));
    out
}

pub fn latest_confirmed(events: &[TrackerEvent]) -> Option<TrackerEvent> {
    events.iter().find(|e| e.is_confirmed()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> String {
        include_str!("../tests/fixtures/timeline_fragment.html").to_string()
    }

    fn expected() -> Vec<(String, String, String)> {
        let raw = include_str!("../tests/fixtures/timeline_expected.json");
        let v: serde_json::Value =
            serde_json::from_str(raw).expect("expected fixture 应为合法 JSON");
        v.as_array()
            .expect("expected fixture 应为数组")
            .iter()
            .map(|e| {
                (
                    e["datetime"].as_str().unwrap_or("").to_string(),
                    e["kind"].as_str().unwrap_or("").to_string(),
                    e["source_url"].as_str().unwrap_or("").to_string(),
                )
            })
            .collect()
    }

    #[test]
    fn parses_every_real_node_with_exact_attributes() {
        let events = parse_tracker(&fixture());
        let mut exp = expected();
        exp.sort_by(|a, b| b.0.cmp(&a.0));
        assert_eq!(events.len(), exp.len(), "节点数量应与抓取测量一致");
        for (got, want) in events.iter().zip(exp.iter()) {
            assert_eq!(got.date_time, want.0);
            assert_eq!(got.kind, want.1);
            assert_eq!(got.source_url.as_deref(), Some(want.2.as_str()));
        }
    }

    #[test]
    fn newest_confirmed_matches_the_measured_last_global_reset() {
        let events = parse_tracker(&fixture());
        let last = latest_confirmed(&events).expect("fixture 含 confirmed 节点");
        assert_eq!(last.date_time, "2026-10-07T03:35:09.000Z");
        assert_eq!(last.ts_ms, 1_791_344_109_000);
    }

    #[test]
    fn results_are_sorted_newest_first() {
        let events = parse_tracker(&fixture());
        assert!(events.windows(2).all(|w| w[0].ts_ms >= w[1].ts_ms));
    }

    #[test]
    fn nodes_without_a_parsable_datetime_are_skipped_not_counted() {
        let html = "<div data-kind=\"confirmed\" data-testid=\"reset-timeline-item\"></div>";
        assert!(parse_tracker(html).is_empty());
    }

    #[test]
    fn empty_page_is_not_an_error() {
        assert!(parse_tracker("").is_empty());
    }

    #[test]
    fn attributes_in_a_different_order_still_parse() {
        let html = "<div data-testid=\"reset-timeline-item\" data-kind=\"confirmed\" data-datetime=\"2026-10-07T03:35:09.000Z\" data-source-url=\"https://x.com/a&amp;b\">x</div>";
        let e = &parse_tracker(html)[0];
        assert_eq!(e.source_url.as_deref(), Some("https://x.com/a&b"));
        assert_eq!(e.ts_ms, 1_791_344_109_000);
    }

    #[test]
    fn parses_the_whole_real_page_not_just_a_hand_picked_slice() {
        // 37 个节点是抓取实测数量（28 confirmed + 9 post），
        // 只测 3 个节点的代表性不够：真实页面里属性顺序和取值范围都有差异。
        let html = include_str!("../tests/fixtures/timeline_all_nodes.html").to_string();
        let raw = include_str!("../tests/fixtures/timeline_all_expected.json");
        let want: serde_json::Value = serde_json::from_str(raw).unwrap();
        let want = want.as_array().unwrap();
        assert_eq!(want.len(), 37, "夹具本身应与抓取一致");

        let events = parse_tracker(&html);
        assert_eq!(events.len(), 37, "37 个节点应全部解出来");
        assert_eq!(
            events.iter().filter(|e| e.is_confirmed()).count(),
            28,
            "confirmed 数量应与实测一致"
        );
        assert_eq!(
            events.iter().filter(|e| e.kind == "post").count(),
            9,
            "post 数量应与实测一致"
        );
        assert!(
            events.iter().all(|e| e.source_url.is_some()),
            "实测每个节点都带来源链接"
        );
        assert!(
            events.windows(2).all(|w| w[0].ts_ms >= w[1].ts_ms),
            "必须按时间倒序"
        );
        assert_eq!(events[0].date_time, "2026-10-07T03:35:09.000Z");
        assert_eq!(events[0].kind, "confirmed");

        // 逐条比对，防止只对上数量却对不上内容。
        let mut want_sorted: Vec<&serde_json::Value> = want.iter().collect();
        want_sorted.sort_by(|a, b| {
            b["datetime"]
                .as_str()
                .unwrap_or("")
                .cmp(a["datetime"].as_str().unwrap_or(""))
        });
        for (got, w) in events.iter().zip(want_sorted.iter()) {
            assert_eq!(got.date_time, w["datetime"].as_str().unwrap());
            assert_eq!(got.kind, w["kind"].as_str().unwrap());
            assert_eq!(
                got.source_url.as_deref(),
                w["source_url"].as_str(),
                "来源链接要逐条对上"
            );
        }
    }
}
