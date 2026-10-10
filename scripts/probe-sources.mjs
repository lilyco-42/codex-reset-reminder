// 线上数据源探针：不依赖应用本身，直接断言解析器赖以工作的字段仍然存在。
// 用法：node scripts/probe-sources.mjs
import { writeFileSync } from "node:fs";

const TRACKER = process.env.CRR_TRACKER || "https://codexreset.org";
const STATUS = process.env.CRR_STATUS || "https://codex-reset.com/api/status";
const UA = "codex-reset-reminder-probe/0.1";

const failures = [];
const notes = [];

function check(ok, label, detail) {
  if (ok) notes.push(`PASS ${label}${detail ? ` (${detail})` : ""}`);
  else failures.push(`FAIL ${label}${detail ? ` (${detail})` : ""}`);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// 单次网络抖动不该把车道判红（本机代理还会伪造故障），但结构变化必须稳定判红，
// 所以只重试传输层错误，不重试解析断言。
async function getText(url, attempts = 3) {
  let lastErr = null;
  for (let i = 0; i < attempts; i++) {
    try {
      const res = await fetch(url, { headers: { "user-agent": UA } });
      if (!res.ok) throw new Error(`${url} -> HTTP ${res.status}`);
      return await res.text();
    } catch (e) {
      lastErr = e;
      if (i < attempts - 1) await sleep(2000 * (i + 1));
    }
  }
  throw lastErr;
}

// 1) 追踪器：必须有带 data-datetime / data-kind 的时间线节点，且含 confirmed
const html = await getText(TRACKER);
const nodes = [...html.matchAll(/data-testid="reset-timeline-item"/g)].length;
check(nodes > 0, "tracker 有 reset-timeline-item 节点", `${nodes} 个`);

const items = [];
for (const m of html.matchAll(/data-testid="reset-timeline-item"/g)) {
  const start = html.lastIndexOf("<", m.index);
  const end = html.indexOf(">", m.index);
  if (start < 0 || end < 0) continue;
  const tag = html.slice(start, end);
  const attr = (n) => {
    const r = new RegExp(`${n}="([^"]*)"`).exec(tag);
    return r ? r[1] : null;
  };
  const dt = attr("data-datetime");
  const kind = attr("data-kind");
  if (dt && kind) items.push({ dt, kind, ms: Date.parse(dt), url: attr("data-source-url") });
}
check(items.length === nodes, "每个节点都能取到 data-datetime 与 data-kind", `${items.length}/${nodes}`);
check(items.every((i) => Number.isFinite(i.ms)), "全部 datetime 可解析为时间戳");
const confirmed = items.filter((i) => i.kind === "confirmed").sort((a, b) => b.ms - a.ms);
check(confirmed.length > 0, "存在 confirmed 类别", `${confirmed.length} 条`);
const newest = confirmed[0];
check(
  newest && newest.url && /^https?:\/\//.test(newest.url),
  "最新 confirmed 带可跳转来源链接",
  newest ? newest.url : "无"
);

// 2) 状态接口：必须是 JSON 且含解析器用到的键
const statusRaw = await getText(STATUS);
let status = null;
try {
  status = JSON.parse(statusRaw);
} catch (e) {
  check(false, "status 返回合法 JSON", String(e.message));
}
if (status) {
  for (const key of ["indicator", "codex", "degraded", "incident_count"]) {
    check(Object.prototype.hasOwnProperty.call(status, key), `status 含字段 ${key}`, `${key}=${JSON.stringify(status[key])}`);
  }
  check(typeof status.degraded === "boolean", "status.degraded 是布尔值", typeof status.degraded);
  check(typeof status.incident_count === "number", "status.incident_count 是数字", status.incident_count);
}

const summary = {
  at: new Date().toISOString(),
  tracker_nodes: nodes,
  tracker_parsed: items.length,
  newest_confirmed: newest ? { datetime: newest.dt, ms: newest.ms, url: newest.url } : null,
  status: status
    ? {
        indicator: status.indicator,
        codex: status.codex,
        degraded: status.degraded,
        incident_count: status.incident_count,
        checked_at: status.checked_at ?? null,
      }
    : null,
  failures,
};
writeFileSync(new URL("../probe-report.json", import.meta.url), JSON.stringify(summary, null, 2));

console.log(notes.join("\n"));
console.log(JSON.stringify(summary, null, 2));
if (failures.length) {
  console.error(`\n${failures.length} 项失败：\n${failures.join("\n")}`);
  process.exit(1);
}
console.log("\n全部数据源探针通过");
