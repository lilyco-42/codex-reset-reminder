# Codex 额度重置提醒器

跨平台监控 Codex 额度窗口与全局重置事件，在重置前后主动发系统通知。
Tauri 2 + 纯 HTML/JS 前端，零 npm 运行时依赖。

| 平台 | 状态 | 产物 |
| --- | --- | --- |
| Windows | CI 构建 | NSIS `.exe` + `.msi` |
| Linux | CI 构建 | `.deb` + `.AppImage` |
| macOS | CI 构建 | `.dmg` |
| Android | CI 构建 | `app-universal-debug.apk`（只编 aarch64 一个 Rust 目标，实际仅含 arm64-v8a，可直接侧载） |
| iOS | 标签/手动触发构建 | **未签名 `.ipa`**，需 SideStore/AltStore 重签 |

## 三个数据源（都是实测出来的，不是猜的）

1. **本机 Codex 会话日志** `~/.codex/sessions/**/rollout-*.jsonl`
   - 实测本机单个会话文件 1 850 346 264 B（1.85 GB），所以只做**尾部读取**（4 MB），绝不整文件加载。
   - 取 `payload.rate_limits`：`primary`（300 分钟窗口）与 `secondary`（10 080 分钟窗口），字段 `used_percent` / `window_minutes` / `resets_at`（Unix 秒）。
   - 正向对照：实测 `resets_at=1791650325` → 距抓取时刻 4.2 小时，`secondary` 比 `primary` 晚 5.91 天，与 300/10080 分钟窗口一致。
   - 兼容 `payload.info.rate_limits` 这一层嵌套。
2. **codexreset.org 时间线**（服务端渲染 HTML）
   - 解析 `data-testid="reset-timeline-item"` 节点上的 `data-datetime` / `data-kind` / `data-source-url`。
   - 2026-10-10 快照为 37 个节点（28 `confirmed` + 9 `post`），10-11 复测已涨到 38（28 + 10）——
     节点数会随时间增长，所以夹具是冻结快照，测试里的数字对应快照而不是"当前线上"。
     最新 `confirmed` = `2026-10-07T03:35:09.000Z`。
3. **codex-reset.com/api/status**（JSON）
   - 实测字段：`indicator` `description` `codex` `degraded` `incident_count` `checked_at`。
   - 注意：`indicator` 可以是 `minor` 而 `degraded=false`、`incident_count=0`，所以**故障判据不看 indicator**，只看 `degraded || incident_count>0 || codex!="operational"`。这条判断有专门的回归测试（`minor_indicator_alone_is_not_an_incident`）。
   - 实测该接口会限流：返回 200 + `{"error":...,"retry_after":...}`。若按结构体直接反序列化，`serde(default)` 会把它读成"全部正常"，所以解析前先看原始 JSON 形状（回归测试 `rate_limited_error_body_is_never_read_as_healthy`）。
   - 实测 `Cache-Control: public, max-age=120`，且首页 brotli 后 152 KB（不压缩 999 KB），因此 reqwest 开了 `gzip`+`brotli`，默认轮询间隔 15 分钟。

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
npm install          # 只装 @tauri-apps/cli 一个开发依赖
npm run dev          # tauri dev
npm run test         # cargo test --locked
npm run probe        # 校验线上数据源结构
npm run icon         # 从 src-tauri/app-icon.png 重新生成图标集
```

安卓车道必须走 `npm run tauri`：Tauri 生成的 gradle 里 `rustBuild<Abi>` 任务
直接调用 `npm run tauri`，所以 `package.json` 里必须有 `tauri` 脚本和本地 CLI
（桌面车道仍然用 `npx --yes @tauri-apps/cli@2`，不依赖 node_modules）。

## 构建与发布

编译、测试、打包**全部在 GitHub Actions**，本机不跑原生工具链。

- `.github/workflows/ci.yml`：`check`（fmt + 单测）→ `desktop`（三平台矩阵）/ `android`；`probe` 独立车道；打 `v*` 标签时追加 `ios` 与 `release`。
- `check` 故意放在矩阵前面：Rust API 名字写错只花几分钟，不会烧掉四平台矩阵。
- 所有 action 按 commit SHA 固定，SHA 由 `gh api repos/<owner>/<repo>/commits/<tag>` 解析并逐个验证过。

## 预研结论（build vs buy）

| 项目 | ★ | 许可 | 为什么没直接用 |
| --- | --- | --- | --- |
| `zangzi2018/quota-pool-for-codex` | 101 | AGPL-3.0 | Swift+C# 自建控制台，要自己跑服务器（Swift 208 KB / C# 64 KB 代码量实测） |
| `deLiseLINO/codex-quota` | 83 | MIT | Go 终端程序，只有桌面 |
| `Vincent-hechuan/codex-quota-band` | 38 | MIT | 推到小米手环，不是手机 App |
| `DiMY-CN/CodexQuotaMonitor` | 12 | 无许可 | 仅 Windows WPF；无许可=默认保留全部权利，不能取用 |
| `HoshinoSuzumi/codex-quota-reset-notifier` | 0 | 无许可 | 思路可借鉴（Bark 推送），代码不可取用 |

自研的理由是**具名硬约束**：单栈覆盖 Win/Linux/macOS/Android/iOS + 零自建服务器 + 与自有客户端一致的 AGPL。
借鉴的两点：Bark/ntfy 推送通道、以及"读会话日志而不是猜额度"的做法。

注：`gh search code` 在本机环境对已知存在的符号也返回空（正向对照失败），所以"没有代码级先例"这个判断不成立，只是工具不可用。

## 手机怎么收到提醒

iOS 不允许 App 在后台随便定时唤醒，Apple 又要求推送必须走 APNs，所以**不靠手机常驻**：
桌面端跑引擎，命中提醒时同时 POST 到你填的 ntfy topic 或 Bark 地址（设置里 `push_url`，必须 https）。
手机装免费的 ntfy / Bark App 订阅即可，不需要服务器、不需要 Apple 开发者账号。

## 境内镜像（可选）

国内直连两个第三方站点不稳定，所以同一份数据做了**两个独立生产者**，读取端是同一个 Rust 解析器：

| 文件 | 作用 |
| --- | --- |
| `mirror/sync.py` | 抓两个源 → 解析 → 原子写 `data.json` + 内联数据的 `index.html`。只用标准库，跑完即退，无常驻进程 |
| `mirror/codex-reset-mirror.{service,timer}` | systemd oneshot + 10 分钟 timer |
| `mirror/patch-pingap-codex.py` | 幂等地给 pingap 加一条 `/codex` 静态目录 location（带 dry-run、备份、`pingap -t` 自检、两个变异对照） |
| `mirror-cf/src/index.js` | Cloudflare Worker：`scheduled` 每 10 分钟同步进 R2；提供 `/data.json` `/` `/health` `/sync?token=` |
| `mirror-cf/wrangler.toml` | 绑**独立**桶 `codex-reset-mirror`，不复用游戏资产桶 `stronghold-assets` |

### 实测

境内阿里云机（cn-shanghai）发起：`codexreset.org` 200 / 9.8 s、`codex-reset.com/api/status` 200 / 2.6 s、
`x.com` 直连失败。同步一轮 6.99 s，产出 `data.json` 7 062 B、`index.html` 10 986 B。

**`*.workers.dev` 在境内不可用（实测，不是听说）**：同一台境内机器访问
`https://codex-reset-mirror.<account>.workers.dev/data.json` → **20 秒超时**，
且该域名 DNS 被污染到 `2a03:2880::/29`（Facebook 段）。对照两项：`api.cloudflare.com` 能连通、
同为 Cloudflare 边缘的 `codexreset.org` 境内返回 200。结论是**不是 Cloudflare 被墙，
而是 `workers.dev` 这个后缀被专门污染**。

**境内可用的端点已经有了**（2026-10-11 做完并实测）：把镜像桶绑一个自己的域名，Worker 不动、
数据链路不变，只是多一条从 R2 直接读同一条对象的途径：

```
https://codexreset.lilyco42.top/data.json
```

境内阿里云机（cn-shanghai）→ **HTTP 200 / 6206 B**，正文 `generated_at_cst 2026-10-11 19:00:26`、
`events 38`、`errors []`、`status` 字段齐（sha256 `4ca2fc44b7c6…`）。
注意一件不对称的事：**这台开发机走代理去访问它会在 TLS 握手阶段失败（schannel），
而境内服务器拿到的是 200**——所以别用本机的失败去判断境内可用性。

绑法（用现有 `wrangler login` 的 OAuth token 就行，**不需要 `dns_records:edit`**，
Cloudflare 自己建记录）：

```bash
node <全局>/wrangler/bin/wrangler.js r2 bucket domain add codex-reset-mirror \
  --domain codexreset.lilyco42.top --zone-id "$(curl -s -H "Authorization: Bearer $TOK" \
  'https://api.cloudflare.com/client/v4/zones?name=lilyco42.top' | jq -r '.result[0].id')" -y
```

**代价要知道**：R2 自定义域名等于**整桶公开可读**。这个桶里只有 `data.json` 一个对象，
而它本来就被 Worker 的 `/data.json` 公开返回（CORS `*`），所以没有新增暴露面；
换桶之前要先把这一点核清楚。撤销是一条命令：`wrangler r2 bucket domain remove codex-reset-mirror --domain …`。

```bash
cd mirror-cf
npx wrangler r2 bucket create codex-reset-mirror                        # 只需一次
printf '%s' "$(openssl rand -hex 16)" | npx wrangler secret put SYNC_TOKEN
npx wrangler deploy
```

App 侧：设置里填 `mirror_url`（必须 https）即优先走镜像，一次请求同时拿到事件与状态；
境内可用的值就是上面那个 `https://codexreset.lilyco42.top/data.json`。
**镜像失败或数据超过 6 小时没更新会自动退回直连并在状态栏说明**——后者是防止同步任务悄悄停了、
App 却一直以为"没有新重置"。留空则行为与 v0.1.0 完全一致。

镜像这一路的超时是独立的 **6 秒**（直连仍是 20 秒），因为上面实测的失败形态是"连得上、不回包"
而不是"立刻拒绝"：按 20 秒等下去，一次检查光是等镜像就把大半轮询窗口烧光了。
另加 5 秒连接超时，最坏情况（镜像 6 s + 两个直连 20 s + 20 s = 46 s）仍小于被 clamp 到
1 分钟的最小轮询间隔。`blackholed_mirror_fails_inside_its_own_budget` 用本机黑洞服务验证超时真的会触发，
`a_responding_server_still_reads_through_the_same_path` 是同一条代码路径的正向对照。

两份夹具 `mirror_data.json`（Python 产出）与 `mirror_data_cf.json`（Cloudflare 产出）都是真实抓取结果；
测试 `both_producers_share_one_contract` 断言两个独立生产者的字段与节点集合完全一致，
写入端漂移会立刻让 CI 变红。

## 已知边界

- **Android 13+ 通知权限**：Tauri 的 Android 模板只声明 `INTERNET`，缺 `POST_NOTIFICATIONS` 时通知会被系统静默丢弃，所以 CI 在 `android init` 之后把该权限注入生成的 manifest（注入步骤自带幂等守卫与回读断言）。但**运行时授权弹窗**仍需真机验证：插件的 Rust `show()` 不保证会主动申请，首次装好可能要在系统设置里手动放行一次。
- **移动端读不到本机额度**：iOS/Android 没有 `~/.codex` 会话日志，界面会明确标注，只显示全局重置与服务状态；个人额度靠桌面端转发推送。
- **开机自启只在桌面端有效**：`tauri-plugin-autostart` 的 Rust API（`ManagerExt`/`MacosLauncher`/`init`）在 android/ios 目标上根本不存在，实测安卓编译会直接报错，所以移动端不注册该插件，设置页里的自启开关在手机上自动隐藏，而不是灰着骗人。同理 `WebviewWindow::minimize/unminimize` 也只存在于桌面端。
- **没有自建推送服务**：推送依赖 ntfy.sh / Bark 这类公共服务，且 iOS 推送最终仍经 APNs，公共免费通道有频率上限。
- **iOS 真机安装**：CI 产出的是**未签名 `.ipa`**（`tauri ios build --no-sign`），需用 SideStore/AltStore 以免费 Apple ID 重签后安装，免费账号 7 天过期需续签；付费开发者账号可延长到 1 年。
  已核对 v0.1.0 的 ipa 内容：可执行文件头是 `cffaedfe` + cputype `0100000c`（**arm64 真机，不是模拟器**），
  `Info.plist` 里 `CFBundleIdentifier=top.lain42.codexreset`、`DTPlatformName=iPhoneOS`、
  `UIRequiredDeviceCapabilities` 含 `arm64`，且不含 `embedded.mobileprovision`（未签名的预期结果）。
  复核方式：下载 ipa → `unzip` → 读 `Payload/*.app/<executable>` 前 8 字节与 `Info.plist` 字符串表。
- 追踪器是第三方页面，结构会变。`source-drift.yml` 每天跑一次探针，红了就说明解析器要改。

