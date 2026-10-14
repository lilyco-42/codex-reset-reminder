import type { CheckResult, LocalSnapshot, PlatformInfo, Settings } from "./types";

// 判断是否真在 Tauri 里：v2 注入的是 __TAURI_INTERNALS__（不是 v1 的 __TAURI__.core）。
// 用错这个判断的后果是"浏览器里打开也以为自己在桌面端"，然后每个调用都抛错。
const inTauri = () => typeof (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__ !== "undefined";

// ?mock=1 是给"不打包也能看界面"留的缝：截图核对布局、检查组件状态分支，
// 不需要设备，也不需要在他机器上跑产物。它和真数据在界面上明确区分（见 App 里的"演示数据"标记）。
const wantMock = () => new URLSearchParams(window.location.search).get("mock") === "1";

export const isPreview = !inTauri() && wantMock();
export const isRealHost = inTauri();

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  if (inTauri()) {
    const { invoke } = await import("@tauri-apps/api/core");
    return invoke<T>(cmd, args);
  }
  if (isPreview) {
    const { mockCheckResult, mockPlatform } = await import("./mock");
    if (cmd === "crr_check" || cmd === "crr_set_settings") return mockCheckResult() as unknown as T;
    if (cmd === "crr_local") return (mockCheckResult().local ?? null) as unknown as T;
    if (cmd === "crr_platform") return mockPlatform() as unknown as T;
    return undefined as T; // crr_reset_history / crr_window 在预览里无事可做
  }
  throw new Error("未运行在 Tauri 环境中：请用 tauri dev 或打包后的应用打开");
}

export const crrCheck = (nowMs: number, tzOffsetMinutes: number) =>
  call<CheckResult>("crr_check", { nowMs, tzOffsetMinutes });
export const crrLocal = () => call<LocalSnapshot | null>("crr_local");
export const crrPlatform = () => call<PlatformInfo>("crr_platform");
export const crrSetSettings = (settings: Settings) => call<Settings>("crr_set_settings", { settings });
export const crrResetHistory = () => call<unknown>("crr_reset_history");
export const crrHide = () => call<unknown>("crr_window", { action: "hide" });

export async function onTrayCheck(cb: () => void) {
  if (!inTauri()) return;
  const { listen } = await import("@tauri-apps/api/event");
  await listen("crr-tray-check", () => cb());
}
