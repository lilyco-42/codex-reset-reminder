use std::sync::Mutex;
use std::time::Duration;

use serde_json::Value;
use tauri::{AppHandle, Emitter as _, Manager};
use tauri_plugin_notification::NotificationExt;

use crate::engine::evaluate;
use crate::error::{AppError, Result};
use crate::model::{CheckResult, LocalSnapshot, RunState, Settings, StatusInfo, TrackerEvent};
use crate::source_local::{default_codex_home, read_local};
use crate::source_mirror::parse_mirror;
use crate::source_org::{latest_confirmed, parse_tracker};
use crate::source_status::parse_status;
use crate::store::Store;

pub const TRACKER_URL: &str = "https://codexreset.org";
pub const STATUS_URL: &str = "https://codex-reset.com/api/status";
const UA: &str = "codex-reset-reminder/0.1";
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
/// 镜像这一路的预算必须明显短于直连：workers.dev 在境内是被 DNS 污染的，
/// 解析出来的地址多半连得上但不回包，属于"挂住"而不是"立刻失败"。
/// 若按 FETCH_TIMEOUT 等，一次检查光是等镜像就烧掉 20 秒才想起退回直连。
const MIRROR_TIMEOUT: Duration = Duration::from_secs(6);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
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
    get_text_budget(url, FETCH_TIMEOUT).await
}

async fn get_text_budget(url: &str, budget: Duration) -> Result<String> {
    // connect_timeout 不能大于总预算，否则 reqwest 会整个建不出客户端。
    let connect = if CONNECT_TIMEOUT < budget {
        CONNECT_TIMEOUT
    } else {
        budget
    };
    let client = reqwest::Client::builder()
        .timeout(budget)
        .connect_timeout(connect)
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

    let cfg = {
        let guard = state.lock().map_err(|e| AppError::Io(e.to_string()))?;
        guard.settings.clone()
    };

    let mut tracker: Vec<TrackerEvent> = Vec::new();
    let mut status: Option<StatusInfo> = None;
    let mut from_mirror = false;

    // 镜像优先：境内直连两个第三方站点不稳定，自建镜像一次给齐事件与状态。
    let mirror = cfg.mirror_url.trim().to_string();
    if !mirror.is_empty() {
        match get_text_budget(&mirror, MIRROR_TIMEOUT).await {
            Ok(json) => match parse_mirror(&json, now_ms) {
                Ok(m) => {
                    from_mirror = true;
                    tracker = m.events;
                    status = m.status;
                    for e in m.errors {
                        errors.push(format!("镜像内部告警：{e}"));
                    }
                }
                Err(e) => errors.push(format!("镜像：{e}；改用直连")),
            },
            Err(e) => errors.push(format!("镜像：{e}；改用直连")),
        }
    }

    if !from_mirror {
        tracker = match get_text(TRACKER_URL).await {
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
        status = match get_text(STATUS_URL).await {
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
    }

    let (report, push_target) = {
        let mut guard = state.lock().map_err(|e| AppError::Io(e.to_string()))?;
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
            source_used: if from_mirror {
                "mirror".into()
            } else {
                "direct".into()
            },
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

/// 开机自启只在桌面端存在：autostart 插件在 android/ios 目标上整套 Rust API 都被
/// cfg 掉了（实测安卓编译报 unresolved import / cannot find init）。
#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn apply_autostart(app: &AppHandle, enable: bool) {
    use tauri_plugin_autostart::ManagerExt as AutostartExt;
    if enable {
        let _ = app.autolaunch().enable();
    } else {
        let _ = app.autolaunch().disable();
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn apply_autostart(_app: &AppHandle, _enable: bool) {}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn minimize_window(window: &tauri::WebviewWindow) -> std::result::Result<(), String> {
    window.minimize().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn minimize_window(_window: &tauri::WebviewWindow) -> std::result::Result<(), String> {
    Err("该平台没有最小化".to_string())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
fn restore_window(window: &tauri::WebviewWindow) {
    let _ = window.unminimize();
}

#[cfg(any(target_os = "android", target_os = "ios"))]
fn restore_window(_window: &tauri::WebviewWindow) {}

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
    apply_autostart(&app, enable);
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
            restore_window(&window);
            let _ = window.set_focus();
            shown
        }
        "minimize" => minimize_window(&window).is_ok(),
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
    s.mirror_url = s.mirror_url.trim().to_string();
    if !s.mirror_url.is_empty() && !s.mirror_url.starts_with("https://") {
        return Err(AppError::Args(
            "镜像地址必须以 https:// 开头，留空表示直连两个官方站点".into(),
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
    fn mirror_url_is_optional_and_https_only() {
        let s = clamp_settings(Settings::default()).expect("默认设置应通过");
        assert!(
            s.mirror_url.is_empty(),
            "默认不启用镜像，行为与 v0.1.0 一致"
        );
        let ok = clamp_settings(Settings {
            mirror_url: "  https://lain42.top/codex/data.json  ".into(),
            ..Settings::default()
        })
        .expect("https 镜像地址应通过");
        assert_eq!(ok.mirror_url, "https://lain42.top/codex/data.json");
        let bad = clamp_settings(Settings {
            mirror_url: "http://lain42.top/codex/data.json".into(),
            ..Settings::default()
        });
        assert!(bad.is_err(), "明文 http 镜像必须拒绝");
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

    /// 只在测试里用的本机服务。respond=false 时收下连接后一言不发，
    /// 用来复现境内解析到黑洞地址时"连得上、不回包"的那种挂起。
    fn start_local_server(respond: bool) -> u16 {
        // 别让环境里的代理把发往 127.0.0.1 的请求拐走。
        std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("本机监听应可用");
        let port = listener.local_addr().expect("应能拿到本机监听端口").port();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut stream) = conn else { continue };
                if respond {
                    let mut buf = [0u8; 512];
                    let _ = std::io::Read::read(&mut stream, &mut buf);
                    let body = "{\"ok\":true}";
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = std::io::Write::write_all(&mut stream, head.as_bytes());
                    let _ = std::io::Write::flush(&mut stream);
                } else {
                    // 故意既不回包也不关连接，留着让超时自己触发。
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_secs(120));
                        drop(stream);
                    });
                }
            }
        });
        port
    }

    #[test]
    fn blackholed_mirror_fails_inside_its_own_budget() {
        let port = start_local_server(false);
        let url = format!("http://127.0.0.1:{port}/data.json");
        let started = std::time::Instant::now();
        let r = tauri::async_runtime::block_on(get_text_budget(&url, MIRROR_TIMEOUT));
        let elapsed = started.elapsed();
        let secs = MIRROR_TIMEOUT.as_secs();
        assert!(r.is_err(), "黑洞服务必须失败，不能当成取到了数据");
        assert!(
            elapsed < Duration::from_secs(15),
            "镜像预算 {secs} 秒，实测 {elapsed:?}，说明超时没生效"
        );
    }

    #[test]
    fn a_responding_server_still_reads_through_the_same_path() {
        // 正向对照：上一条的失败必须来自"对方不回包"，而不是这条路本身走不通。
        let port = start_local_server(true);
        let url = format!("http://127.0.0.1:{port}/data.json");
        let body = tauri::async_runtime::block_on(get_text(&url)).expect("正常服务应能读到内容");
        assert_eq!(body, "{\"ok\":true}");
    }

    #[test]
    fn one_check_cycle_fits_inside_the_minimum_poll_interval() {
        // 轮询间隔最小被 clamp 到 1 分钟，一次检查的最坏网络耗时（镜像 + 两个直连）
        // 要留有余量，否则上一轮没跑完下一轮就叠上来了。
        assert!(
            MIRROR_TIMEOUT < FETCH_TIMEOUT,
            "镜像必须先于直连放弃，直连才是数据的真正来源"
        );
        let worst = MIRROR_TIMEOUT + FETCH_TIMEOUT + FETCH_TIMEOUT;
        assert!(
            worst < Duration::from_secs(60),
            "一次检查最坏要 {worst:?}，已经贴住最小轮询间隔"
        );
    }
}
