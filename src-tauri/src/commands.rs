use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, Emitter as _, Manager};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_http::reqwest;
use tauri_plugin_notification::NotificationExt;

use crate::engine::evaluate;
use crate::error::{AppError, Result};
use crate::model::{CheckResult, LocalSnapshot, RunState, Settings, StatusInfo, TrackerEvent};
use crate::source_local::{default_codex_home, read_local};
use crate::source_org::{latest_confirmed, parse_tracker};
use crate::source_status::parse_status;
use crate::store::Store;

pub const TRACKER_URL: &str = "https://codexreset.org";
pub const STATUS_URL: &str = "https://codex-reset.com/api/status";
const UA: &str = "codex-reset-reminder/0.1";
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;

pub const IS_MOBILE: bool = cfg!(any(target_os = "android", target_os = "ios"));

pub struct Inner {
    pub store: Store,
    pub settings: Settings,
    pub state: RunState,
}

pub type Shared = Mutex<Inner>;

fn notification_id(key: &str) -> i64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h & 0x7fff_ffff_ffff_ffff) as i64
}

fn fire(app: &AppHandle, alerts: &[crate::model::Alert]) {
    for a in alerts {
        if let Err(e) = app
            .notification()
            .builder()
            .title(a.title.as_str())
            .body(a.body.as_str())
            .notification_id(notification_id(&a.key))
            .show()
        {
            eprintln!("[crr] 通知发送失败：{e}");
        }
    }
}

async fn get_text(url: &str) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(UA)
        .build()
        .map_err(|e| AppError::Http(format!("无法创建 HTTP 客户端：{e}")))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| AppError::Http(format!("{url} 请求失败：{e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(AppError::Http(format!("{url} 返回 {status}")));
    }
    let body = resp
        .text()
        .await
        .map_err(|e| AppError::Http(format!("{url} 读取内容失败：{e}")))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(AppError::Parse(format!(
            "{url} 返回 {} 字节，超出预期",
            body.len()
        )));
    }
    Ok(body)
}

fn read_now(fetched_at_ms: i64) -> LocalSnapshot {
    match default_codex_home() {
        Some(h) => read_local(&h, fetched_at_ms),
        None => LocalSnapshot::unavailable(fetched_at_ms, "找不到用户主目录，无法定位 ~/.codex"),
    }
}

/// 只读本机额度，不走网络，供界面高频刷新。
#[tauri::command]
pub fn crr_local() -> LocalSnapshot {
    let now = crate::timefmt::now_ms();
    let mut snap = read_now(now);
    if snap.limits.is_none() {
        let fallback = if IS_MOBILE {
            "移动端没有本机 Codex 会话日志，仅显示全局重置与服务状态"
        } else {
            "尚未读取到本机额度，先在 Codex 里跑一轮任务"
        };
        snap.reason = Some(snap.reason.unwrap_or_else(|| fallback.to_string()));
    }
    snap
}

#[tauri::command]
pub async fn crr_check(
    app: AppHandle,
    state: tauri::State<'_, Shared>,
    now_ms: i64,
    tz_offset_minutes: i64,
) -> std::result::Result<CheckResult, AppError> {
    let local = read_now(now_ms);
    let mut errors: Vec<String> = Vec::new();

    let tracker: Vec<TrackerEvent> = match get_text(TRACKER_URL).await {
        Ok(html) => {
            let e = parse_tracker(&html);
            if e.is_empty() {
                errors.push("追踪器页面结构可能变了：没解析到任何时间线节点".to_string());
            }
            e
        }
        Err(e) => {
            errors.push(format!("追踪器：{e}"));
            Vec::new()
        }
    };
    let status: Option<StatusInfo> = match get_text(STATUS_URL).await {
        Ok(json) => match parse_status(&json) {
            Ok(s) => Some(s),
            Err(e) => {
                errors.push(format!("状态接口：{e}"));
                None
            }
        },
        Err(e) => {
            errors.push(format!("状态接口：{e}"));
            None
        }
    };

    let report = {
        let mut guard = state.lock().map_err(|e| AppError::Io(e.to_string()))?;
        let cfg = guard.settings.clone();
        evaluate(
            &cfg,
            now_ms,
            tz_offset_minutes,
            Some(&local),
            &tracker,
            status.as_ref(),
            &mut guard.state,
        )
    };

    fire(&app, &report.to_fire);

    let result = {
        let mut guard = state.lock().map_err(|e| AppError::Io(e.to_string()))?;
        if let Err(e) = guard.store.save_state(&guard.state) {
            errors.push(format!("状态落盘：{e}"));
        }
        CheckResult {
            fetched_at_ms: now_ms,
            local: local.clone(),
            tracker: tracker.clone(),
            last_global_reset: latest_confirmed(&tracker),
            status: status.clone(),
            fired: report.to_fire.clone(),
            history: guard.state.history.clone(),
            suppressed_in_quiet: report.suppressed,
            errors: errors.clone(),
            settings: guard.settings.clone(),
        }
    };
    let _ = app.emit("crr-updated", &result);
    Ok(result)
}

#[tauri::command]
pub fn crr_get_settings(state: tauri::State<'_, Shared>) -> Settings {
    lock(&state).settings.clone()
}

#[tauri::command]
pub fn crr_set_settings(
    app: AppHandle,
    state: tauri::State<'_, Shared>,
    settings: Settings,
) -> std::result::Result<Settings, AppError> {
    {
        let mut guard = lock(&state);
        let clean = clamp_settings(settings);
        guard.settings = clean.clone();
        guard.store.save_settings(&clean)?;
    }
    let enable = lock(&state).settings.autostart;
    if enable {
        let _ = app.autolaunch().enable();
    } else {
        let _ = app.autolaunch().disable();
    }
    let mut guard = lock(&state);
    let reloaded = guard.store.load_settings();
    guard.settings = reloaded.clone();
    Ok(reloaded)
}

#[tauri::command]
pub fn crr_reset_history(state: tauri::State<'_, Shared>) -> std::result::Result<(), AppError> {
    let mut guard = lock(&state);
    guard.state = RunState::default();
    guard.store.save_state(&guard.state)
}

/// 前端不引任何 npm 包，窗口显隐统一走这里。
#[tauri::command]
pub fn crr_window(app: AppHandle, action: String) -> std::result::Result<bool, AppError> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| AppError::Args("找不到主窗口".into()))?;
    let ok = match action.as_str() {
        "hide" => window.hide().is_ok(),
        "show" => {
            let shown = window.show().is_ok();
            let _ = window.unminimize();
            let _ = window.set_focus();
            shown
        }
        "minimize" => window.minimize().is_ok(),
        other => return Err(AppError::Args(format!("未知的窗口动作：{other}"))),
    };
    Ok(ok)
}

#[tauri::command]
pub fn crr_platform(state: tauri::State<'_, Shared>) -> Value {
    let guard = lock(&state);
    serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "mobile": IS_MOBILE,
        "config_dir": guard.store.root().display().to_string(),
        "tracker_url": TRACKER_URL,
        "status_url": STATUS_URL,
    })
}

fn clamp_settings(mut s: Settings) -> Settings {
    if s.poll_minutes < 1 {
        s.poll_minutes = 1;
    }
    if s.poll_minutes > 180 {
        s.poll_minutes = 180;
    }
    if !(0.0..=100.0).contains(&s.warn_percent) {
        s.warn_percent = 80.0;
    }
    if !(0.0..=100.0).contains(&s.critical_percent) {
        s.critical_percent = 95.0;
    }
    if s.critical_percent < s.warn_percent {
        s.critical_percent = (s.warn_percent + 5.0).min(100.0);
    }
    s.reset_soon_minutes = s.reset_soon_minutes.clamp(1, 240);
    s.quiet_start %= 24;
    s.quiet_end %= 24;
    s
}

fn lock<'a>(state: &'a tauri::State<'_, Shared>) -> std::sync::MutexGuard<'a, Inner> {
    match state.lock() {
        Ok(g) => g,
        Err(e) => e.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_stable_and_positive() {
        let a = notification_id("warn:每周:1:8");
        let b = notification_id("warn:每周:1:8");
        assert_eq!(a, b, "同一 key 必须映射到同一通知 id");
        assert!(a >= 0);
        assert_ne!(a, notification_id("warn:每周:1:9"));
    }

    #[test]
    fn clamping_repairs_impossible_settings() {
        let s = clamp_settings(Settings {
            poll_minutes: 0,
            warn_percent: -5.0,
            critical_percent: 10.0,
            reset_soon_minutes: 999,
            quiet_start: 40,
            quiet_end: 24,
            ..Settings::default()
        });
        assert_eq!(s.poll_minutes, 1);
        assert_eq!(s.warn_percent, 80.0);
        assert!(s.critical_percent >= s.warn_percent);
        assert_eq!(s.reset_soon_minutes, 240);
        assert_eq!(s.quiet_start, 16);
        assert_eq!(s.quiet_end, 0);
    }

    #[test]
    fn sources_are_the_measured_https_endpoints() {
        // 线上可达性由 CI 的 probe 车道负责，这里只锁住配置不被改错。
        assert_eq!(TRACKER_URL, "https://codexreset.org");
        assert_eq!(STATUS_URL, "https://codex-reset.com/api/status");
        assert!(!TRACKER_URL.contains("localhost"));
        if cfg!(any(target_os = "android", target_os = "ios")) {
            assert!(IS_MOBILE);
        } else {
            assert!(!IS_MOBILE, "桌面目标不应被判定为移动端");
        }
    }
}
