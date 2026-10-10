use serde::{Deserialize, Serialize};

/// 一个额度窗口（5 小时或每周）。字段直接来自 Codex 会话日志里的 rate_limits。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LimitWindow {
    pub used_percent: f64,
    pub window_minutes: i64,
    /// Unix 秒
    pub resets_at: i64,
}

impl LimitWindow {
    pub fn resets_at_ms(&self) -> i64 {
        self.resets_at.saturating_mul(1000)
    }

    pub fn minutes_left(&self, now_ms: i64) -> i64 {
        (self.resets_at_ms() - now_ms).div_euclid(60_000)
    }

    pub fn label(&self) -> &'static str {
        if self.window_minutes <= 360 {
            "5 小时"
        } else {
            "每周"
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Credits {
    #[serde(default)]
    pub has_credits: bool,
    #[serde(default)]
    pub unlimited: bool,
    #[serde(default)]
    pub balance: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimits {
    #[serde(default)]
    pub limit_id: String,
    #[serde(default)]
    pub primary: Option<LimitWindow>,
    #[serde(default)]
    pub secondary: Option<LimitWindow>,
    #[serde(default)]
    pub credits: Option<Credits>,
    #[serde(default)]
    pub plan_type: Option<String>,
}

/// 本机 Codex 会话日志读出来的快照。移动端读不到，source 为 unavailable。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LocalSnapshot {
    pub captured_at_ms: i64,
    /// local | unavailable
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub file_bytes: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limits: Option<RateLimits>,
}

impl LocalSnapshot {
    pub fn unavailable(now_ms: i64, reason: impl Into<String>) -> Self {
        Self {
            captured_at_ms: now_ms,
            source: "unavailable".into(),
            reason: Some(reason.into()),
            path: None,
            file_bytes: 0,
            limits: None,
        }
    }
}

/// codexreset.org 时间线上的一条记录（confirmed / post）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackerEvent {
    pub date_time: String,
    pub ts_ms: i64,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

impl TrackerEvent {
    pub fn id(&self) -> String {
        format!("{}:{}", self.kind, self.date_time)
    }

    pub fn is_confirmed(&self) -> bool {
        self.kind == "confirmed"
    }
}

/// codex-reset.com/api/status。实测：indicator 会是 "minor" 而 degraded=false、
/// incident_count=0，所以 indicator 单独不能作为故障判据。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusInfo {
    #[serde(default)]
    pub indicator: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub codex: String,
    #[serde(default)]
    pub degraded: bool,
    #[serde(default)]
    pub incident_count: i64,
    #[serde(default)]
    pub checked_at: Option<String>,
}

impl StatusInfo {
    pub fn in_trouble(&self) -> bool {
        self.degraded
            || self.incident_count > 0
            || (self.codex != "operational" && !self.codex.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    pub key: String,
    /// info | warn | critical
    pub level: String,
    pub title: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub created_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CheckResult {
    pub fetched_at_ms: i64,
    pub local: LocalSnapshot,
    pub tracker: Vec<TrackerEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_global_reset: Option<TrackerEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<StatusInfo>,
    pub fired: Vec<Alert>,
    pub history: Vec<Alert>,
    pub suppressed_in_quiet: usize,
    pub errors: Vec<String>,
    pub settings: Settings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub poll_minutes: u32,
    pub warn_percent: f64,
    pub critical_percent: f64,
    pub reset_soon_minutes: i64,
    pub quiet_hours: bool,
    pub quiet_start: u32,
    pub quiet_end: u32,
    pub notify_local: bool,
    pub notify_global_reset: bool,
    pub notify_tracker_post: bool,
    pub notify_status_incident: bool,
    pub autostart: bool,
    /// ntfy topic 或 Bark 地址；填了就把同样的提醒再推一份到手机，
    /// 这样 iPhone 不需要让 App 常驻也能收到。
    pub push_url: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            // 实测 codex-reset.com/api/status 的 Cache-Control 是 max-age=120，
            // 且首页压缩后仍有 152 KB，所以默认 15 分钟一轮，不追着服务器打。
            poll_minutes: 15,
            warn_percent: 80.0,
            critical_percent: 95.0,
            reset_soon_minutes: 15,
            quiet_hours: false,
            quiet_start: 23,
            quiet_end: 7,
            notify_local: true,
            notify_global_reset: true,
            notify_tracker_post: false,
            notify_status_incident: true,
            autostart: true,
            push_url: String::new(),
        }
    }
}

/// 去重与对比状态，落盘在 state.json。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RunState {
    pub seeded: bool,
    pub prev: Option<LocalSnapshot>,
    /// key -> 触发时间 ms
    pub notified: Vec<(String, i64)>,
    pub seen_tracker: Vec<String>,
    /// 上一次是否处于故障态，用于只在恢复时补一条通知
    pub status_troubled: bool,
    pub history: Vec<Alert>,
}

impl RunState {
    pub fn was_notified(&self, key: &str) -> bool {
        self.notified.iter().any(|(k, _)| k == key)
    }

    pub fn mark_notified(&mut self, key: &str, now_ms: i64) {
        if !self.was_notified(key) {
            self.notified.push((key.to_string(), now_ms));
        }
    }

    pub fn prune(&mut self, now_ms: i64, max_keys: usize, max_tracker: usize) {
        let cutoff = now_ms - 30 * 86_400_000;
        self.notified.retain(|(_, t)| *t >= cutoff);
        if self.notified.len() > max_keys {
            let drop = self.notified.len() - max_keys;
            self.notified.drain(0..drop);
        }
        if self.seen_tracker.len() > max_tracker {
            let start = self.seen_tracker.len() - max_tracker;
            self.seen_tracker = self.seen_tracker[start..].to_vec();
        }
        const HISTORY_MAX: usize = 60;
        if self.history.len() > HISTORY_MAX {
            let start = self.history.len() - HISTORY_MAX;
            self.history = self.history[start..].to_vec();
        }
    }
}
