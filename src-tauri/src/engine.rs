use crate::model::{
    Alert, LimitWindow, LocalSnapshot, RateLimits, RunState, Settings, StatusInfo, TrackerEvent,
};
use crate::timefmt::{format_local, human_delta, in_quiet_for, local_date_of};

pub struct Report {
    pub to_fire: Vec<Alert>,
    pub suppressed: usize,
}

struct Emitter<'a> {
    now_ms: i64,
    quiet: bool,
    state: &'a mut RunState,
    fired: Vec<Alert>,
    suppressed: usize,
}

impl<'a> Emitter<'a> {
    /// 命中过的 key 直接跳过；静默时段内的非严重提醒不标记，出时段后可以补发。
    fn emit(&mut self, key: String, level: &str, title: String, body: String, url: Option<String>) {
        if self.state.was_notified(&key) {
            return;
        }
        if self.quiet && level != "critical" {
            self.suppressed += 1;
            return;
        }
        self.state.mark_notified(&key, self.now_ms);
        let alert = Alert {
            key,
            level: level.to_string(),
            title,
            body,
            url,
            created_ms: self.now_ms,
        };
        self.state.history.push(alert.clone());
        self.fired.push(alert);
    }
}

fn window_rules(
    e: &mut Emitter,
    cfg: &Settings,
    label: &str,
    cur: &LimitWindow,
    prev: Option<&LimitWindow>,
    tz: i64,
) {
    let resets_at = cur.resets_at;
    let used = cur.used_percent;
    let key_label = label.to_string();

    if used >= cfg.critical_percent {
        e.emit(
            format!("crit:{key_label}:{resets_at}"),
            "critical",
            format!("{key_label}额度已用 {used:.0}%"),
            format!(
                "将在 {} 重置（还有 {}）",
                format_local(cur.resets_at_ms(), tz),
                human_delta(cur.minutes_left(e.now_ms))
            ),
            None,
        );
    } else if used >= cfg.warn_percent {
        let step = (used / 10.0).floor() as i64;
        e.emit(
            format!("warn:{key_label}:{resets_at}:{step}"),
            "warn",
            format!("{key_label}额度已用 {used:.0}%"),
            format!(
                "将在 {} 重置（还有 {}）",
                format_local(cur.resets_at_ms(), tz),
                human_delta(cur.minutes_left(e.now_ms))
            ),
            None,
        );
    }

    let left = cur.minutes_left(e.now_ms);
    if left > 0 && left <= cfg.reset_soon_minutes {
        e.emit(
            format!("soon:{key_label}:{resets_at}"),
            "warn",
            format!("{key_label}额度即将重置"),
            format!("还有 {left} 分钟，可以先把重活留到重置后跑。"),
            None,
        );
    }

    if let Some(p) = prev {
        if p.resets_at_ms() <= e.now_ms {
            e.emit(
                format!("done:{key_label}:{}", p.resets_at),
                "info",
                format!("{key_label}额度已重置"),
                format!(
                    "上一窗口重置时间 {}，本轮用量已清零或接近清零。",
                    format_local(p.resets_at_ms(), tz)
                ),
                None,
            );
        }
        // 窗口被提前，同时用量下降：通常对应一次官方发放的全局重置。
        if resets_at < p.resets_at && used < p.used_percent {
            e.emit(
                format!("forced:{key_label}:{resets_at}"),
                "info",
                format!("{key_label}额度被提前重置"),
                format!(
                    "重置时间从 {} 提前到 {}，用量从 {:.0}% 降到 {:.0}%，通常是一次全局重置。",
                    format_local(p.resets_at_ms(), tz),
                    format_local(cur.resets_at_ms(), tz),
                    p.used_percent,
                    used
                ),
                None,
            );
        }
    }
}

fn tracker_rules(e: &mut Emitter, cfg: &Settings, tracker: &[TrackerEvent], now_ms: i64, tz: i64) {
    const DAY_MS: i64 = 86_400_000;
    let seeding = !e.state.seeded;
    if seeding {
        e.state.seeded = true;
    }
    for ev in tracker {
        let id = ev.id();
        if e.state.seen_tracker.contains(&id) {
            continue;
        }
        // 首轮只登记一天之前的历史：刚装上就弹二十几条旧重置没有意义，
        // 但一天内真的发生过的新重置仍然要提醒，否则等于漏报。
        if seeding && ev.ts_ms < now_ms - DAY_MS {
            e.state.seen_tracker.push(id);
            continue;
        }
        let want = if ev.is_confirmed() {
            cfg.notify_global_reset
        } else {
            cfg.notify_tracker_post
        };
        if !want {
            e.state.seen_tracker.push(id);
            continue;
        }
        let quiet_len = (now_ms - ev.ts_ms) / 60_000;
        let body = if quiet_len > 0 {
            format!(
                "发生时间 {}，距今 {}。",
                format_local(ev.ts_ms, tz),
                human_delta(quiet_len)
            )
        } else {
            format!("发生时间 {}。", format_local(ev.ts_ms, tz))
        };
        let key = format!("tracker:{id}");
        let before = e.fired.len();
        if ev.is_confirmed() {
            e.emit(
                key,
                "info",
                "全局额度重置已确认".to_string(),
                format!("{body}来源见链接。"),
                ev.source_url.clone(),
            );
        } else {
            e.emit(
                key,
                "info",
                "追踪器有新的重置相关动态".to_string(),
                format!("{body}未确认前不作为重置依据。"),
                ev.source_url.clone(),
            );
        }
        // 被静默拦下时不登记，出静默期后仍可补发。
        if e.fired.len() > before {
            e.state.seen_tracker.push(id);
        }
        // 超过一天的旧条目不再补发，只登记。
        else if ev.ts_ms < now_ms - DAY_MS {
            e.state.seen_tracker.push(id);
        }
    }
}

fn status_rules(
    e: &mut Emitter,
    cfg: &Settings,
    status: Option<&StatusInfo>,
    now_ms: i64,
    tz: i64,
) {
    let Some(s) = status else { return };
    if !cfg.notify_status_incident {
        return;
    }
    let day = local_date_of(now_ms, tz);
    if s.in_trouble() {
        e.state.status_troubled = true;
        e.emit(
            format!("status:{day}:{}", s.indicator),
            "warn",
            "Codex 服务状态异常".to_string(),
            format!(
                "{}（codex: {}，事件 {} 条，检查时间 {}）",
                if s.description.is_empty() {
                    "服务降级"
                } else {
                    &s.description
                },
                if s.codex.is_empty() {
                    "未知"
                } else {
                    &s.codex
                },
                s.incident_count,
                format_local(now_ms, tz)
            ),
            None,
        );
    } else if e.state.status_troubled {
        e.state.status_troubled = false;
        e.emit(
            format!("status-recover:{day}"),
            "info",
            "Codex 服务已恢复正常".to_string(),
            format!(
                "状态 {}，检查时间 {}。",
                s.indicator,
                format_local(now_ms, tz)
            ),
            None,
        );
    }
}

pub fn evaluate(
    cfg: &Settings,
    now_ms: i64,
    tz: i64,
    local: Option<&LocalSnapshot>,
    tracker: &[TrackerEvent],
    status: Option<&StatusInfo>,
    state: &mut RunState,
) -> Report {
    let quiet = in_quiet_for(cfg, now_ms, tz);
    let prev = state.prev.clone();
    let mut e = Emitter {
        now_ms,
        quiet,
        state,
        fired: Vec::new(),
        suppressed: 0,
    };

    if cfg.notify_local {
        if let Some(rl) = local.and_then(|s| s.limits.as_ref()) {
            let prev_rl: Option<&RateLimits> = prev.as_ref().and_then(|p| p.limits.as_ref());
            if let Some(cur) = rl.primary.as_ref() {
                let p = prev_rl.and_then(|x| x.primary.as_ref());
                window_rules(&mut e, cfg, cur.label(), cur, p, tz);
            }
            if let Some(cur) = rl.secondary.as_ref() {
                let p = prev_rl.and_then(|x| x.secondary.as_ref());
                window_rules(&mut e, cfg, cur.label(), cur, p, tz);
            }
        }
    }

    // 抓取失败时跳过整段：否则首轮会把 seeded 标记消费掉，等追踪器恢复时会
    // 一次性弹出几十条历史重置。
    if !tracker.is_empty() {
        tracker_rules(&mut e, cfg, tracker, now_ms, tz);
    }
    status_rules(&mut e, cfg, status, now_ms, tz);

    if let Some(snap) = local {
        if snap.limits.is_some() {
            e.state.prev = Some(snap.clone());
        }
    }
    e.state.prune(now_ms, 600, 200);
    Report {
        to_fire: e.fired,
        suppressed: e.suppressed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Credits;

    const NOW: i64 = 1_791_635_211_807; // 2026-10-10T12:26:51.807Z
    const BJ: i64 = 480; // 东 8 区，北京时间 20:26
    const QUIET_NOW: i64 = 1_791_644_400_000; // 2026-10-10T15:00:00Z = 北京 23:00
    const PRIMARY_RESET: i64 = 1_791_650_325; // 实测本机值，北京 00:38

    fn win(used: f64, window: i64, resets_at: i64) -> LimitWindow {
        LimitWindow {
            used_percent: used,
            window_minutes: window,
            resets_at,
        }
    }

    fn snap(primary: Option<LimitWindow>, secondary: Option<LimitWindow>) -> LocalSnapshot {
        LocalSnapshot {
            captured_at_ms: NOW,
            source: "local".into(),
            reason: None,
            path: Some("/tmp/x.jsonl".into()),
            file_bytes: 100,
            limits: Some(RateLimits {
                limit_id: "codex".into(),
                primary,
                secondary,
                credits: Some(Credits {
                    has_credits: false,
                    unlimited: false,
                    balance: "0".into(),
                }),
                plan_type: None,
            }),
        }
    }

    fn ev(kind: &str, date_time: &str, ts_ms: i64) -> TrackerEvent {
        TrackerEvent {
            date_time: date_time.to_string(),
            ts_ms,
            kind: kind.to_string(),
            source_url: Some("https://x.com/thsottiaux/status/1".into()),
        }
    }

    fn status(degraded: bool, indicator: &str, codex: &str, count: i64) -> StatusInfo {
        StatusInfo {
            indicator: indicator.into(),
            description: "Partial System Degradation".into(),
            codex: codex.into(),
            degraded,
            incident_count: count,
            checked_at: None,
        }
    }

    fn run(
        cfg: &Settings,
        now: i64,
        local: Option<&LocalSnapshot>,
        t: &[TrackerEvent],
        s: Option<&StatusInfo>,
        st: &mut RunState,
    ) -> Report {
        evaluate(cfg, now, BJ, local, t, s, st)
    }

    #[test]
    fn warn_fires_once_per_window_and_is_deduped() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let local = snap(Some(win(82.0, 300, PRIMARY_RESET)), None);
        let r = run(&cfg, NOW, Some(&local), &[], None, &mut st);
        assert_eq!(r.to_fire.len(), 1, "首轮应恰好一条");
        assert_eq!(r.to_fire[0].level, "warn");
        let again = run(&cfg, NOW + 60_000, Some(&local), &[], None, &mut st);
        assert!(again.to_fire.is_empty(), "同一档不应重复提醒");
    }

    #[test]
    fn below_warn_threshold_is_silent_and_critical_still_fires() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let low = snap(Some(win(15.0, 300, PRIMARY_RESET)), None);
        assert!(run(&cfg, NOW, Some(&low), &[], None, &mut st)
            .to_fire
            .is_empty());
        let high = snap(Some(win(96.0, 300, PRIMARY_RESET)), None);
        let r = run(&cfg, NOW, Some(&high), &[], None, &mut st);
        assert_eq!(r.to_fire.len(), 1);
        assert_eq!(r.to_fire[0].level, "critical");
    }

    #[test]
    fn reset_soon_only_inside_the_configured_window() {
        let mut cfg = Settings::default();
        cfg.reset_soon_minutes = 15;
        let mut st = RunState::default();
        let soon = snap(Some(win(30.0, 300, NOW / 1000 + 600)), None);
        let r = run(&cfg, NOW, Some(&soon), &[], None, &mut st);
        assert_eq!(r.to_fire.len(), 1, "10 分钟后重置应提醒");
        assert!(r.to_fire[0].title.contains("即将重置"));

        let mut st2 = RunState::default();
        let far = snap(Some(win(30.0, 300, NOW / 1000 + 3_600)), None);
        assert!(
            run(&cfg, NOW, Some(&far), &[], None, &mut st2)
                .to_fire
                .is_empty(),
            "1 小时后不应提醒"
        );
    }

    #[test]
    fn elapsed_previous_window_reports_reset_done() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let past = snap(Some(win(90.0, 300, NOW / 1000 - 120)), None);
        run(&cfg, NOW, Some(&past), &[], None, &mut st);
        let fresh = snap(Some(win(2.0, 300, NOW / 1000 + 14_400)), None);
        let r = run(&cfg, NOW + 1_000, Some(&fresh), &[], None, &mut st);
        assert!(
            r.to_fire.iter().any(|a| a.title.contains("已重置")),
            "应报告上一窗口重置"
        );
    }

    #[test]
    fn earlier_resets_at_with_lower_usage_is_read_as_forced_reset() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let before = snap(Some(win(60.0, 300, NOW / 1000 + 7_200)), None);
        run(&cfg, NOW, Some(&before), &[], None, &mut st);
        let after = snap(Some(win(5.0, 300, NOW / 1000 + 900)), None);
        let r = run(&cfg, NOW + 1_000, Some(&after), &[], None, &mut st);
        let a = r
            .to_fire
            .iter()
            .find(|a| a.title.contains("提前重置"))
            .expect("应识别提前重置");
        assert!(a.body.contains("全局重置"));
    }

    fn history(n: usize) -> Vec<TrackerEvent> {
        (0..n)
            .map(|i| {
                let day = i as i64 / 3 + 1;
                let hour = i as i64 % 3;
                ev(
                    "confirmed",
                    &format!("2026-06-{day:02}T{hour:02}:00:00.000Z"),
                    NOW - (i as i64 + 2) * 86_400_000,
                )
            })
            .collect()
    }

    #[test]
    fn first_run_does_not_spam_the_whole_history() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let old = history(28);
        assert_eq!(
            old.iter()
                .map(|e| e.id())
                .collect::<std::collections::HashSet<_>>()
                .len(),
            28,
            "夹具必须互不相同，否则测试是空的"
        );
        let r = run(&cfg, NOW, None, &old, None, &mut st);
        assert!(r.to_fire.is_empty(), "首轮只登记历史，不应弹窗");
        assert!(st.seeded);
        assert_eq!(st.seen_tracker.len(), 28, "28 条都应被登记");
    }

    #[test]
    fn first_run_still_notifies_a_reset_from_the_last_day() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let fresh = ev("confirmed", "2026-10-10T12:00:00.000Z", NOW - 3_600_000);
        let old = ev(
            "confirmed",
            "2026-06-29T00:00:00.000Z",
            NOW - 100 * 86_400_000,
        );
        let r = run(&cfg, NOW, None, &[old, fresh], None, &mut st);
        assert_eq!(r.to_fire.len(), 1, "只应报一天内这条，更早的仍被吞掉");
        assert_eq!(r.to_fire[0].title, "全局额度重置已确认");
        assert_eq!(st.seen_tracker.len(), 2, "两条都应登记，不能重复报");
    }

    #[test]
    fn a_failed_fetch_does_not_burn_the_first_run_seed() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let r = run(&cfg, NOW, None, &[], None, &mut st);
        assert!(r.to_fire.is_empty());
        assert!(!st.seeded, "追踪器抓取失败时不应消费首轮种子");

        let old = history(28);
        let r2 = run(&cfg, NOW + 1_000, None, &old, None, &mut st);
        assert!(r2.to_fire.is_empty(), "恢复后的第一轮应只登记，不刷屏");
        assert!(st.seeded);
    }

    #[test]
    fn a_newly_confirmed_global_reset_fires_exactly_once() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let a = vec![ev(
            "confirmed",
            "2026-10-07T03:35:09.000Z",
            1_791_344_109_000,
        )];
        run(&cfg, NOW, None, &a, None, &mut st);
        let b = vec![
            ev("confirmed", "2026-10-10T12:00:00.000Z", NOW - 1_600_000),
            ev("confirmed", "2026-10-07T03:35:09.000Z", 1_791_344_109_000),
        ];
        let r = run(&cfg, NOW + 60_000, None, &b, None, &mut st);
        assert_eq!(r.to_fire.len(), 1);
        assert_eq!(r.to_fire[0].title, "全局额度重置已确认");
        assert_eq!(
            r.to_fire[0].url.as_deref(),
            Some("https://x.com/thsottiaux/status/1")
        );
        let again = run(&cfg, NOW + 120_000, None, &b, None, &mut st);
        assert!(again.to_fire.is_empty());
    }

    #[test]
    fn tracker_posts_are_off_by_default() {
        let cfg = Settings::default();
        assert!(!cfg.notify_tracker_post);
        let mut st = RunState::default();
        st.seeded = true;
        let a = vec![ev("post", "2026-10-10T12:00:00.000Z", NOW - 300_000)];
        assert!(run(&cfg, NOW, None, &a, None, &mut st).to_fire.is_empty());
        assert_eq!(st.seen_tracker.len(), 1, "关闭的类别仍应登记");
    }

    #[test]
    fn quiet_hours_suppress_warn_but_not_critical() {
        let mut cfg = Settings::default();
        cfg.quiet_hours = true;
        assert!(in_quiet_for(&cfg, QUIET_NOW, BJ), "北京 23:00 应在静默时段");
        let mut st = RunState::default();
        let warn = snap(Some(win(85.0, 300, PRIMARY_RESET)), None);
        let r = run(&cfg, QUIET_NOW, Some(&warn), &[], None, &mut st);
        assert!(r.to_fire.is_empty());
        assert_eq!(r.suppressed, 1, "应统计被静默的条数");

        let crit = snap(Some(win(99.0, 300, PRIMARY_RESET)), None);
        let r2 = run(&cfg, QUIET_NOW + 1_000, Some(&crit), &[], None, &mut st);
        assert_eq!(r2.to_fire.len(), 1, "严重提醒不受静默限制");

        // 出静默期后，之前被压住的 warn 可以补发
        let mut st3 = RunState::default();
        run(&cfg, QUIET_NOW, Some(&warn), &[], None, &mut st3);
        let out = QUIET_NOW + 8 * 3_600_000; // 北京 07:00
        let r3 = run(&cfg, out, Some(&warn), &[], None, &mut st3);
        assert!(
            r3.to_fire.iter().any(|a| a.key.starts_with("warn:")),
            "静默结束后应补发被压住的警告（此时上一窗口也已到点，所以不能断言只有一条）"
        );
    }

    #[test]
    fn quiet_hours_wrap_past_midnight_and_respect_the_flag() {
        let mut cfg = Settings::default();
        cfg.quiet_hours = true;
        assert!(!in_quiet_for(&cfg, NOW, BJ), "北京 20:26 不在 23-07");
        cfg.quiet_hours = false;
        assert!(!in_quiet_for(&cfg, QUIET_NOW, BJ), "关闭后任何时间都不静默");
    }

    #[test]
    fn minor_indicator_without_degradation_is_silent() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let s = status(false, "minor", "operational", 0);
        assert!(run(&cfg, NOW, None, &[], Some(&s), &mut st)
            .to_fire
            .is_empty());
    }

    #[test]
    fn incident_fires_then_recovery_fires_once() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let bad = status(true, "major", "degraded", 2);
        let r = run(&cfg, NOW, None, &[], Some(&bad), &mut st);
        assert_eq!(r.to_fire.len(), 1);
        assert_eq!(r.to_fire[0].title, "Codex 服务状态异常");
        assert!(st.status_troubled);

        let good = status(false, "none", "operational", 0);
        let r2 = run(&cfg, NOW + 60_000, None, &[], Some(&good), &mut st);
        assert_eq!(r2.to_fire.len(), 1);
        assert_eq!(r2.to_fire[0].title, "Codex 服务已恢复正常");
        let r3 = run(&cfg, NOW + 120_000, None, &[], Some(&good), &mut st);
        assert!(r3.to_fire.is_empty(), "恢复只报一次");
    }

    #[test]
    fn disabling_local_rules_silences_quota_alerts_only() {
        let mut cfg = Settings::default();
        cfg.notify_local = false;
        let mut st = RunState::default();
        let local = snap(Some(win(99.0, 300, PRIMARY_RESET)), None);
        let r = run(&cfg, NOW, Some(&local), &[], None, &mut st);
        assert!(r.to_fire.is_empty());
        let bad = status(true, "major", "degraded", 1);
        let r2 = run(&cfg, NOW + 1_000, Some(&local), &[], Some(&bad), &mut st);
        assert_eq!(r2.to_fire.len(), 1, "服务状态不受 notify_local 影响");
    }

    #[test]
    fn unavailable_local_snapshot_produces_no_quota_alerts() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let mobile = LocalSnapshot::unavailable(NOW, "移动端无法读取本机 Codex 会话");
        assert!(run(&cfg, NOW, Some(&mobile), &[], None, &mut st)
            .to_fire
            .is_empty());
        assert!(st.prev.is_none(), "无额度快照不应覆盖上一轮对比基准");
    }

    #[test]
    fn both_windows_are_evaluated_independently() {
        let cfg = Settings::default();
        let mut st = RunState::default();
        let local = snap(
            Some(win(5.0, 300, PRIMARY_RESET)),
            Some(win(88.0, 10_080, PRIMARY_RESET + 500_000)),
        );
        let r = run(&cfg, NOW, Some(&local), &[], None, &mut st);
        assert_eq!(r.to_fire.len(), 1);
        assert!(r.to_fire[0].title.contains("每周"), "命中的应是每周窗口");
    }
}
