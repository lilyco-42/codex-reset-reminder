# v0.1.0 首个版本

Codex 额度重置提醒器。盯三件事：本机 5 小时/每周额度窗口的重置倒计时、
官方发放的全局重置事件、Codex 服务状态。命中就发系统通知，也可以同时推到手机。

## 这个版本包含

- **本机额度**：读 `~/.codex/sessions` 会话日志尾部的 `rate_limits`
  （实测单文件 1.85 GB，所以只做尾部读取）。显示 5 小时 / 每周两个窗口的
  用量、重置倒计时和到点提醒。
- **全局重置追踪**：解析 codexreset.org 时间线的 `confirmed` 节点，
  出现新的确认重置立刻提醒；实测最新一条为 2026-10-07T03:35:09Z（北京 11:35）。
- **服务状态**：codex-reset.com/api/status。故障判据用 `degraded` /
  `incident_count` / `codex` 字段，不用 `indicator`（实测 indicator=minor
  时服务其实正常）。
- **提醒规则**：阈值警告(80%)/严重(95%)、重置前 15 分钟、重置完成、
  窗口被提前重置（全局重置的本地证据）、新确认的全局重置、服务故障与恢复。
  全部带去重键，静默时段（默认 23:00–07:00）只压非严重提醒且会补发。
- **推送到手机**：设置里填 ntfy topic 或 Bark 地址（必须 https），
  桌面端命中提醒时同时 POST 一份，iPhone 不需要让 App 常驻。
- 桌面端关窗口只隐藏到系统托盘，后台继续检查；支持开机自启。

## 各平台产物

| 平台 | 文件 | 说明 |
| --- | --- | --- |
| Windows | `*-windows-x64/*Setup*.exe`、`.msi` | NSIS + MSI |
| Linux | `.deb`、`.AppImage` | x86_64 |
| macOS | `.dmg` | |
| Android | `*-android-debug/*.apk` | 仅 arm64-v8a，debug 签名，可直接侧载 |
| iOS | `*-ios-unsigned/*.ipa` | **未签名**，需用 SideStore/AltStore 以免费 Apple ID 重签（7 天到期续签） |

## 已知限制

- 移动端读不到本机 `~/.codex`，只显示全局重置与服务状态；个人额度靠桌面端转发推送。
- 没有自建推送服务器，推送依赖 ntfy.sh / Bark 公共服务，iOS 侧最终仍走 APNs，有频率上限。
- Android 产物是 debug 签名，适合自用侧载，不走应用商店分发。
- 追踪器是第三方页面，结构可能变；仓库有每日探针车道，结构一变就红。

## 校验方式

每个包都由 GitHub Actions 的 `ci.yml` 产出，`check` 车道先跑
`cargo fmt --check` 与 52 个单元测试（夹具全部由线上真实数据脚本导出），
过了才进四平台矩阵。
