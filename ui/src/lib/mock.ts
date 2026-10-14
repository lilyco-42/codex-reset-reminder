import type { CheckResult } from "./types";

// 这不是"随手编的数据"：数值取自仓库里那两份实测 fixture ——
//   src-tauri/tests/fixtures/rate_limits_live.json  → primary 15% / secondary 45% / credits 无
//   src-tauri/tests/fixtures/mirror_data.json      → 38 条事件、28 条已确认、status indicator=minor
// 唯一被改动的是时间：fixture 是 2026-10-11 抓的，直接用会让所有倒计时都显示"已到点"，
// 于是把"距今多久"这个差值平移到当前时间，好让"即将重置"那条分支在预览里也看得见。
// 只有 `?mock=1` 且不在 Tauri 里时才会走到这里，界面上也会打上"演示数据"标记。
const shift = (absoluteMs: number, fromMs: number) => Date.now() + (absoluteMs - fromMs);

const CAPTURED = 1791687354796; // mirror_data.generated_at_ms

export function mockCheckResult(): CheckResult {
  return {
    source_used: "mirror",
    fetched_at_ms: Date.now(),
    local: {
      source: "local",
      file_bytes: 1_850_346_264,
      captured_at_ms: Date.now() - 42_000,
      limits: {
        primary: { used_percent: 15, resets_at: Math.round(shift(1791650325000, CAPTURED) / 1000) },
        secondary: { used_percent: 45, resets_at: Math.round(shift(1792160751000, CAPTURED) / 1000) },
        credits: { has_credits: false, unlimited: false, balance: "0" },
      },
    },
    last_global_reset: { ts_ms: shift(1791344109000, CAPTURED) },
    tracker: [
      {
        kind: "confirmed",
        ts_ms: shift(1791670580000, CAPTURED),
        source_url: "https://x.com/thsottiaux/status/2109045392684032299",
      },
      {
        kind: "confirmed",
        ts_ms: shift(1791344109000, CAPTURED),
        source_url: "https://x.com/thsottiaux/status/2107676072871600470",
      },
      { kind: "post", ts_ms: shift(1791300000000, CAPTURED), source_url: "https://codexreset.org" },
    ],
    status: {
      indicator: "minor",
      description: "Partial System Degradation",
      codex: "operational",
      degraded: false,
      incident_count: 0,
    },
    fired: [],
    history: [
      {
        key: "demo-1",
        title: "每周窗口使用率达 80%",
        body: "阈值 80%，当前 82%",
        level: "warn",
        created_ms: Date.now() - 3_600_000,
        url: "https://codexreset.org",
      },
      {
        key: "demo-2",
        title: "全局重置已确认",
        body: "来源：追踪器确认记录",
        level: "info",
        created_ms: Date.now() - 7_200_000,
        url: "https://codexreset.org",
      },
    ],
    settings: {
      poll_minutes: 15,
      warn_percent: 80,
      critical_percent: 95,
      reset_soon_minutes: 15,
      quiet_hours: true,
      quiet_start: 23,
      quiet_end: 7,
      notify_local: true,
      notify_global_reset: true,
      notify_tracker_post: true,
      notify_status_incident: true,
      autostart: false,
      push_url: "",
      mirror_url: "https://codexreset.lilyco42.top/data.json",
    },
    errors: [],
    suppressed_in_quiet: 0,
  };
}

export function mockPlatform() {
  return { os: "preview", arch: "web", mobile: false, config_dir: "(浏览器预览)" };
}
