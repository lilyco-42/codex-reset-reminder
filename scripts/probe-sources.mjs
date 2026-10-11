// 线上数据源探针：**只负责"结构有没有变"**，不负责"上游此刻活着没"。
// 之前把两件事混在一起，导致第三方偶发 503 就让车道变红；
// 红了的车道迟早会被忽略，所以这里拆开：
//   结构断言失败        -> 退出 1（这是本车道唯一该报警的事）
//   某个源不可用/5xx    -> 记录 degraded 警告，退出 0（除非所有源都拿不到数据）
// 用法：node scripts/probe-sources.mjs
const TRACKER = process.env.CRR_TRACKER || "https://codexreset.org";
const STATUS = process.env.CRR_STATUS || "https://codex-reset.com/api/status";
const MIRROR = process.env.CRR_MIRROR || "https://codex-reset-mirror.m15178682641.workers.dev";
const UA = "codex-reset-reminder-probe/0.1";

const structural = []; // 结构断言：失败就该红
const degraded = []; // 可用性问题：只记录
const notes = [];

function check(ok, label, detail) {
  if (ok) notes.push(`PASS ${label}${detail ? ` (${detail})` : ""}`);
  else structural.push(`FAIL ${label}${detail ? ` (${detail})` : ""}`);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/** 只在传输层/HTTP 错误上重试；返回 {text,status} 或 {text:null,status}。 */
async function getText(url, attempts = 4) {
  let lastStatus = 0;
  for (let i = 0; i < attempts; i++) {
    try {
      const res = await fetch(url, { headers: { "user-agent": UA } });
      lastStatus = res.status;
      if (res.ok) return { text: await res.text(), status: res.status };
      // 4xx 是稳定的（说明结构/路径问题），不再重试；5xx 可能是瞬时。
      if (res.status < 500) return { text: null, status: res.status };
    } catch (e) {
      lastStatus = 0;
      if (i === attempts - 1) {
        degraded.push(`网络：${url} 失败 ${e.message}`);
      }
    }
    if (i < attempts - 1) await sleep(1500 * (i + 1));
  }
  return { text: null, status: lastStatus };
}

function unescapeAttr(s) {
  return s
    .replace(/&amp;/g, "&")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">");
}

function parseTimeline(html) {
  const items = [];
  for (const m of html.matchAll(/data-testid="reset-timeline-item"/g)) {
    const start = html.lastIndexOf("<", m.index);
    const end = html.indexOf(">", m.index);
    if (start < 0 || end < 0) continue;
    const tag = html.slice(start, end);
    const g = (n) => {
      const r = new RegExp(`${n}="([^"]*)"`).exec(tag);
      return r ? unescapeAttr(r[1]) : null;
    };
    const dt = g("data-datetime");
    const kind = g("data-kind");
    if (dt && kind) items.push({ dt, kind, ms: Date.parse(dt), url: g("data-source-url") });
  }
  return items;
}

let trackerOk = false;
let statusOk = false;
let mirrorOk = false;

// ---------- 1) 追踪器 HTML ----------
const tr = await getText(TRACKER);
if (tr.text === null) {
  degraded.push(`追踪器不可用：HTTP ${tr.status}（结构检查跳过，不计失败）`);
} else {
  trackerOk = true;
  const nodes = (tr.text.match(/data-testid="reset-timeline-item"/g) || []).length;
  check(nodes > 0, "tracker 有 reset-timeline-item 节点", `${nodes} 个`);
  const items = parseTimeline(tr.text);
  check(items.length === nodes, "每个节点都能取到 data-datetime 与 data-kind", `${items.length}/${nodes}`);
  check(items.every((i) => Number.isFinite(i.ms)), "全部 datetime 可解析为时间戳");
  const confirmed = items.filter((i) => i.kind === "confirmed").sort((a, b) => b.ms - a.ms);
  check(confirmed.length > 0, "存在 confirmed 类别", `${confirmed.length} 条`);
  check(
    !!confirmed[0] && !!confirmed[0].url && /^https?:\/\//.test(confirmed[0].url),
    "最新 confirmed 带可跳转来源链接",
    confirmed[0] ? confirmed[0].url : "无"
  );
  var newestConfirmed = confirmed[0] || null; // eslint-disable-line no-var
}

// ---------- 2) 状态接口 ----------
let status = null;
const st = await getText(STATUS);
if (st.text === null) {
  degraded.push(`状态接口不可用：HTTP ${st.status}（不计结构失败）`);
} else {
  try {
    status = JSON.parse(st.text);
    statusOk = true;
  } catch (e) {
    check(false, "status 返回合法 JSON", String(e.message));
  }
}
if (status) {
  if (status.error) {
    degraded.push(`状态接口限流：${JSON.stringify(status).slice(0, 80)}`);
  } else {
    for (const key of ["indicator", "codex", "degraded", "incident_count"]) {
      check(
        Object.prototype.hasOwnProperty.call(status, key),
        `status 含字段 ${key}`,
        `${key}=${JSON.stringify(status[key])}`
      );
    }
    check(typeof status.degraded === "boolean", "status.degraded 是布尔值", typeof status.degraded);
    check(typeof status.incident_count === "number", "status.incident_count 是数字", status.incident_count);
  }
}

// ---------- 3) 镜像（第三份独立证据） ----------
const mi = await getText(`${MIRROR}/data.json`);
let mirror = null;
if (mi.text === null) {
  degraded.push(`镜像不可用：HTTP ${mi.status}`);
} else {
  try {
    mirror = JSON.parse(mi.text);
    mirrorOk = true;
  } catch (e) {
    check(false, "镜像返回合法 JSON", String(e.message));
  }
}
if (mirror) {
  for (const key of ["generated_at_ms", "events", "latest_confirmed", "status"]) {
    check(Object.prototype.hasOwnProperty.call(mirror, key), `镜像含字段 ${key}`);
  }
  check(Array.isArray(mirror.events) && mirror.events.length > 0, "镜像 events 非空", `${(mirror.events || []).length} 条`);
  if (newestConfirmed && mirror.latest_confirmed) {
    check(
      mirror.latest_confirmed.date_time === newestConfirmed.dt,
      "镜像与源站的最新 confirmed 一致",
      `${mirror.latest_confirmed.date_time} vs ${newestConfirmed.dt}`
    );
  }
}

// 一个源都拿不到时，本车道没有意义，必须红（而不是假绿）。
if (!trackerOk && !statusOk && !mirrorOk) {
  structural.push("FAIL 三个源全部不可用，探针无法判断结构，车道必须红");
}

const summary = {
  at: new Date().toISOString(),
  availability: {
    tracker: trackerOk ? "ok" : "unavailable",
    status: statusOk ? "ok" : "unavailable",
    mirror: mirrorOk ? "ok" : "unavailable",
  },
  tracker_nodes: trackerOk ? (tr.text.match(/data-testid="reset-timeline-item"/g) || []).length : null,
  newest_confirmed: newestConfirmed ? { datetime: newestConfirmed.dt, ms: newestConfirmed.ms, url: newestConfirmed.url } : null,
  status: status && !status.error ? { indicator: status.indicator, codex: status.codex, degraded: status.degraded, incident_count: status.incident_count } : null,
  mirror_events: mirror ? mirror.events.length : null,
  degraded,
  structural_failures: structural,
};

try {
  const { writeFileSync } = await import("node:fs");
  writeFileSync(new URL("../probe-report.json", import.meta.url), JSON.stringify(summary, null, 2));
} catch (e) {
  console.error("写 probe-report.json 失败:", e.message);
}

console.log(notes.join("\n"));
console.log("\n可用性: " + JSON.stringify(summary.availability));
for (const d of degraded) console.log("WARN(可用性) " + d);
console.log(JSON.stringify(summary, null, 2));

if (structural.length) {
  console.error(`\n${structural.length} 项结构检查失败：\n${structural.join("\n")}`);
  process.exit(1);
}
console.log(`\n结构检查全部通过（可用性告警 ${degraded.length} 条，不判红）`);
