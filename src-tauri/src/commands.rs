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

fn notify_id(key: &str) -> i32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in key.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    // 插件侧是 i32 标识，这里折一次保证落在正数区间。
    ((h & 0x7fff_ffff) | 1) as i32
}

/// 判定推送地址格式：Bark 走 JSON，其余（ntfy 等）走纯文本 + Title 头。
fn push_kind(url: &str) -> Option<&'static str> {
    let u = url.trim();
    if u.is_empty() {
        return None;
    }
    if !u.starts_with("https://") {
        return Some("rejected");
    }
    if u.contains("api.day.app") {
        Some("bark")
    } else {
        Some("plain")
    }
}

async fn push_remote(url: &str, title: &str, body: &str) -> std::result::Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .user_agent(UA)
        .build()
        .map_err(|e| e.to_string())?;
    let kind = push_kind(url).ok_or("未配置推送地址")?;
    // 插件的 reqwest 没有开 json feature，这里自己序列化 body，不去动 feature 图。
    let resp = match kind {
        "bark" => {
            let payload = serde_json::to_vec(&serde_json::json!({ "title": title, "body": body }))
                .map_err(|e| e.to_string())?;
            client
                .post(url)
                .header("Content-Type", "application/json")
                .body(payload)
                .send()
                .await
        }
        "plain" => {
            client
                .post(url)
                .header("Title", title)
                .header("Priority", "4")
                .body(format!("{title}\n{body}"))
                .send()
                .await
        }
        _ => return Err("推送地址必须是 https://".to_string()),
    }
    .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("推送服务返回 {}", resp.status()));
    }
    Ok(())
}

async fn fire(
    app: &AppHandle,
    alerts: &[crate::model::Alert],
    push_url: Option<&str>,
) -> Vec<String> {
    let mut errors = Vec::new();
    for a in alerts {
        if let Err(e) = app
            .notification()
            .builder()
            .title(a.title.as_str())
            .body(a.body.as_str())
            .id(notify_id(&a.key))
            .show()
        {
            errors.push(format!("本机通知：{e}"));
        }
        if let Some(url) = push_url {
            if let Err(e) = push_remote(url, &a.title, &a.body).await {
                errors.push(format!("远程推送：{e}"));
            }
        }
    }
    errors
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

    let (report, push_target) = {
        let mut guard = state.lock().map_err(|e| AppError::Io(e.to_string()))?;
        let cfg = guard.settings.clone();
        let push = cfg.push_url.clone();
        let r = evaluate(
            &cfg,
            now_ms,
            tz_offset_minutes,
            Some(&local),
            &tracker,
            status.as_ref(),
            &mut guard.state,
        );
        (r, push)
    };

    let target = if push_target.trim().is_empty() {
        None
    } else {
        Some(push_target.trim().to_string())
    };
    let mut push_errors = fire(&app, &report.to_fire, target.as_deref()).await;
    errors.append(&mut push_errors);

    let result = {
        let guard = state.lock().map_err(|e| AppError::Io(e.to_string()))?;
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
        let clean = clamp_settings(settings)?;
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

fn clamp_settings(mut s: Settings) -> Result<Settings> {
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
    s.push_url = s.push_url.trim().to_string();
    if !s.push_url.is_empty() && !s.push_url.starts_with("https://") {
        return Err(AppError::Args(
            "推送地址必须以 https:// 开头（ntfy topic 或 Bark 地址）".into(),
        ));
    }
    Ok(s)
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
    fn ids_are_stable_positive_and_fit_i32() {
        let a = notify_id("warn:每周:1:8");
        let b = notify_id("warn:每周:1:8");
        assert_eq!(a, b, "同一 key 必须映射到同一通知 id");
        assert!(a > 0, "id 必须是正数，插件用它做标识");
        assert!(a <= i32::MAX);
        assert_ne!(a, notify_id("warn:每周:1:9"));
    }

    #[test]
    fn push_target_is_classified_and_https_only() {
        assert_eq!(push_kind(""), None, "留空即关闭远程推送");
        assert_eq!(
            push_kind("https://ntfy.sh/my-codex"),
            Some("plain"),
            "ntfy 走纯文本 + Title 头"
        );
        assert_eq!(
            push_kind("https://api.day.app/KEY123"),
            Some("bark"),
            "Bark 走 JSON"
        );
        assert_eq!(
            push_kind("http://ntfy.local/x"),
            Some("rejected"),
            "明文 http 必须拒绝"
        );
        assert_eq!(push_kind("file:///etc/passwd"), Some("rejected"));
    }

    #[test]
    fn saving_a_non_https_push_url_is_rejected_with_a_chinese_message() {
        let bad = Settings {
            push_url: "http://evil.example/x".into(),
            ..Settings::default()
        };
        let e = clamp_settings(bad).expect_err("非 https 应报错");
        assert!(e.to_string().contains("https://"), "错误文案要能看懂：{e}");
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
            push_url: "  https://ntfy.sh/x  ".into(),
            ..Settings::default()
        })
        .expect("合法设置应通过");
        assert_eq!(s.poll_minutes, 1);
        assert_eq!(s.warn_percent, 80.0);
        assert!(s.critical_percent >= s.warn_percent);
        assert_eq!(s.reset_soon_minutes, 240);
        assert_eq!(s.quiet_start, 16);
        assert_eq!(s.quiet_end, 0);
        assert_eq!(s.push_url, "https://ntfy.sh/x", "推送地址应被 trim");
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
