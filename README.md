# Codex 额度重置提醒器

跨平台监控 Codex 额度窗口与全局重置事件，在重置前后主动发系统通知。
Tauri 2 + 纯 HTML/JS 前端，零 npm 运行时依赖。

| 平台 | 状态 | 产物 |
| --- | --- | --- |
| Windows | CI 构建 | NSIS `.exe` + `.msi` |
| Linux | CI 构建 | `.deb` + `.AppImage` |
| macOS | CI 构建 | `.dmg` |
| Android | CI 构建 | debug `.apk`（可侧载） |
| iOS | 标签/手动触发构建 | 模拟器 `.app`（真机需 Apple 账号签名） |

## 三个数据源（都是实测出来的，不是猜的）

1. **本机 Codex 会话日志** `~/.codex/sessions/**/rollout-*.jsonl`
   - 实测本机单个会话文件 1 850 346 264 B（1.85 GB），所以只做**尾部读取**（4 MB），绝不整文件加载。
   - 取 `payload.rate_limits`：`primary`（300 分钟窗口）与 `secondary`（10 080 分钟窗口），字段 `used_percent` / `window_minutes` / `resets_at`（Unix 秒）。
   - 正向对照：实测 `resets_at=1791650325` → 距抓取时刻 4.2 小时，`secondary` 比 `primary` 晚 5.91 天，与 300/10080 分钟窗口一致。
   - 兼容 `payload.info.rate_limits` 这一层嵌套。
2. **codexreset.org 时间线**（服务端渲染 HTML）
   - 解析 `data-testid="reset-timeline-item"` 节点上的 `data-datetime` / `data-kind` / `data-source-url`。
   - 实测 37 个节点（28 `confirmed` + 9 `post`），最新 `confirmed` = `2026-10-07T03:35:09.000Z`。
3. **codex-reset.com/api/status**（JSON）
   - 实测字段：`indicator` `description` `codex` `degraded` `incident_count` `checked_at`。
   - 注意：`indicator` 可以是 `minor` 而 `degraded=false`、`incident_count=0`，所以**故障判据不看 indicator**，只看 `degraded || incident_count>0 || codex!="operational"`。这条判断有专门的回归测试（`minor_indicator_alone_is_not_an_incident`）。

## 提醒规则

| 规则 | 级别 | 去重键 |
| --- | --- | --- |
| 窗口用量 ≥ 警告阈值（默认 80%） | warn | 标签+窗口+每 10% 一档 |
| 窗口用量 ≥ 严重阈值（默认 95%） | critical | 标签+重置时间 |
| 距重置 ≤ N 分钟（默认 15） | warn | 标签+重置时间 |
| 上一窗口已到重置时间 | info | 标签+重置时间 |
| 本机 `resets_at` 提前且用量下降（=全局重置的本地证据） | info | 标签+新重置时间 |
| 追踪器出现新的 `confirmed` | info | 类别+时间 |
| 追踪器新的 `post`（默认关闭） | info | 类别+时间 |
| 服务状态故障 / 恢复 | warn / info | 日期+indicator |

- **静默时段**：默认 23:00–07:00（按设备时区）。被压住的提醒不记去重键，出时段后补发；`critical` 不受静默限制。
- **首轮不刷屏**：第一次运行只登记追踪器历史，不弹任何旧重置；抓取失败时不消费这个种子（有回归测试）。
- 去重键与对比基准落盘在配置目录 `state.json`（Unix 0600），设置落 `settings.json`。

## 本地开发

```bash
npm run dev          # tauri dev
npm run test         # cargo test --locked
npm run probe        # 校验线上数据源结构
npm run icon         # 从 src-tauri/app-icon.png 重新生成图标集
```

## 构建与发布

编译、测试、打包**全部在 GitHub Actions**，本机不跑原生工具链。

- `.github/workflows/ci.yml`：`check`（fmt + 单测）→ `desktop`（三平台矩阵）/ `android`；`probe` 独立车道；打 `v*` 标签时追加 `ios` 与 `release`。
- `check` 故意放在矩阵前面：Rust API 名字写错只花几分钟，不会烧掉四平台矩阵。
- 所有 action 按 commit SHA 固定，SHA 由 `gh api repos/<owner>/<repo>/commits/<tag>` 解析并逐个验证过。

## 已知边界

- **移动端读不到本机额度**：iOS/Android 没有 `~/.codex` 会话日志，界面会明确标注，只显示全局重置与服务状态。要让手机看到个人额度，需要桌面端把快照上报给自己的服务再推给手机（`new-api` 里已有 `FetchCodexWhamRateLimitResetCredits` 可复用）。
- **没有服务端推送**：提醒依赖应用进程存活（桌面端关窗口只是隐藏到托盘）。真正的离线推送需要 APNs/FCM 加一台服务器。
- **iOS 真机分发**：需要 Apple 账号签名；免费账号走 AltStore/SideStore 自签，7 天需重签。CI 默认只构建未签名模拟器包，用来证明 Rust/Swift 胶水能编过。
- 追踪器是第三方页面，结构会变。`source-drift.yml` 每天跑一次探针，红了就说明解析器要改。
