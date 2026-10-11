// 核对镜像本身是否活着：cron 停了 / R2 读不到 / 上游解析出 0 条，都要在这里变红，
// 而不是等 App 一直显示"没有新重置"。
// 用法：node scripts/check-mirror.mjs [url]
const MIRROR =
  process.argv[2] || process.env.CRR_MIRROR_URL || "https://codex-reset-mirror.m15178682641.workers.dev";

const failures = [];
const notes = [];
function check(ok, label, detail) {
  if (ok) notes.push(`PASS ${label}${detail ? ` (${detail})` : ""}`);
  else failures.push(`FAIL ${label}${detail ? ` (${detail})` : ""}`);
}

async function getJSON(path) {
  const res = await fetch(MIRROR + path, { headers: { "user-agent": "crr-mirror-check/1.0" } });
  return { res, body: await res.text() };
}

// 1) /health：由 Worker 自己算时效，超 6 小时返回 500
const h = await getJSON("/health");
let hv = null;
try {
  hv = JSON.parse(h.body);
} catch {
  /* 保留下面的断言去报错 */
}
check(h.res.status === 200, "/health 返回 200", `http=${h.res.status}`);
check(!!hv && hv.ok === true, "镜像未过期", hv ? `age=${hv.age_minutes}分钟 stale=${hv.stale}` : "响应不是 JSON");
check(!!hv && (hv.events ?? 0) > 20, "节点数量合理", hv ? `${hv.events} 条` : "-");
check(!!hv && (hv.confirmed ?? 0) > 10, "confirmed 数量合理", hv ? `${hv.confirmed} 条` : "-");
check(!!hv && Array.isArray(hv.errors) && hv.errors.length === 0, "同步无内部告警", hv ? JSON.stringify(hv.errors) : "-");

// 2) /data.json：契约字段必须齐，且 latest_confirmed 是 confirmed 而不是别的
const d = await getJSON("/data.json");
let dv = null;
try {
  dv = JSON.parse(d.body);
} catch {
  /* 同上 */
}
check(d.res.status === 200, "/data.json 返回 200", `http=${d.res.status}`);
for (const k of ["generated_at_ms", "events", "latest_confirmed", "status", "errors"]) {
  check(!!dv && Object.prototype.hasOwnProperty.call(dv, k), `data.json 含字段 ${k}`);
}
check(!!dv && Array.isArray(dv.events) && dv.events.length > 20, "events 是非空数组", dv ? `${dv.events.length}` : "-");
check(!!dv && dv.events.every((e) => e.date_time && e.kind && Number.isFinite(e.ts_ms)), "每条事件字段完整");
check(!!dv && dv.events.every((e, i) => i === 0 || dv.events[i - 1].ts_ms >= e.ts_ms), "events 按时间倒序");
check(!!dv && dv.latest_confirmed && dv.latest_confirmed.kind === "confirmed", "latest_confirmed 确实是 confirmed");
check(
  !!dv && dv.latest_confirmed && Number.isFinite(Date.parse(dv.latest_confirmed.date_time)),
  "latest_confirmed 时间可解析",
  dv && dv.latest_confirmed ? dv.latest_confirmed.date_time : "-"
);
check(!!dv && dv.status && typeof dv.status.in_trouble === "boolean", "status.in_trouble 是布尔");

// 3) 页面
const p = await getJSON("/");
check(p.res.status === 200, "页面返回 200", `http=${p.res.status}`);
check(p.res.headers.get("content-type")?.includes("text/html"), "页面 content-type 是 HTML");
check(p.body.includes("data.json"), "页面确实去取 data.json");

// 4) CORS：App 的镜像模式要能跨源读
const cors = await fetch(MIRROR + "/data.json", {
  method: "OPTIONS",
  headers: { Origin: "https://tauri.localhost" },
});
check(
  cors.headers.get("access-control-allow-origin") === "*",
  "允许跨源读 data.json",
  `acao=${cors.headers.get("access-control-allow-origin")}`
);

console.log(notes.join("\n"));
if (failures.length) {
  console.error(`\n${failures.length} 项失败：\n${failures.join("\n")}`);
  process.exit(1);
}
console.log(`\n镜像健康，共 ${notes.length} 项检查通过`);
