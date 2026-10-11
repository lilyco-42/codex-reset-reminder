// 核对镜像是否真的活着：cron 停了 / R2 读不到 / 上游解析出 0 条，都要在这里变红，
// 而不是等 App 一直显示"没有新重置"。
//
// 两类端点分开查，因为它们能给的东西不一样：
//   Worker（workers.dev，境内被 DNS 污染）：/health、/data.json、/、CORS 头；
//   R2 自定义域名（境内可用，App 实际该填的那个）：**只有桶里的对象**，
//     没有 /health、没有目录页、也没有 CORS 头——拿 Worker 的断言去套它会全红，
//     那些 404 不代表镜像坏了。所以这里只按 data.json 的契约 + 新鲜度来判。
// 两者还做一次交叉比对：Worker 每 10 分钟重写同一条对象，所以两边的
// generated_at_ms 应该接近；差出一个重写周期以上就说明有一条路没在更新。
//
// 用法：node scripts/check-mirror.mjs [worker-url] [r2-url]
const MIRROR =
  process.argv[2] || process.env.CRR_MIRROR_URL || "https://codex-reset-mirror.m15178682641.workers.dev";
const R2 = process.argv[3] || process.env.CRR_R2_MIRROR_URL || "https://codexreset.lilyco42.top";

const failures = [];
const notes = [];
function check(ok, label, detail) {
  if (ok) notes.push(`PASS ${label}${detail ? ` (${detail})` : ""}`);
  else failures.push(`FAIL ${label}${detail ? ` (${detail})` : ""}`);
}

async function getText(base, path) {
  try {
    const res = await fetch(base + path, { headers: { "user-agent": "crr-mirror-check/1.0" } });
    return { res, body: await res.text() };
  } catch (e) {
    // 抓不到不等于镜像坏了：这台开发机走代理时对新建的 CF 域名会在 TLS 握手阶段失败，
    // 而境内服务器实测 200。所以这里只记录失败原因，别让整个报告跟着崩掉。
    return { res: { status: 0, headers: new Headers(), _err: String(e && e.message || e) }, body: "" };
  }
}

// App 的 STALE_MS 是 6 小时：探针用同一条界线，避免"探针绿、App 已经退回直连"。
const STALE_MS = 6 * 3600 * 1000;

function checkDataContract(tag, got) {
  let v = null;
  try {
    v = JSON.parse(got.body);
  } catch {
    /* 交给下面的断言去报错 */
  }
  check(
    got.res.status === 200,
    `${tag} /data.json 返回 200`,
    `http=${got.res.status}${got.res._err ? ` 抓取失败:${got.res._err}` : ""}`
  );
  for (const k of ["generated_at_ms", "events", "latest_confirmed", "status", "errors"]) {
    check(!!v && Object.prototype.hasOwnProperty.call(v, k), `${tag} 含字段 ${k}`);
  }
  check(!!v && Array.isArray(v.events) && v.events.length > 20, `${tag} events 是非空数组`, v ? `${v.events.length}` : "-");
  check(!!v && v.events.every((e) => e.date_time && e.kind && Number.isFinite(e.ts_ms)), `${tag} 每条事件字段完整`);
  check(
    !!v && v.events.every((e, i) => i === 0 || v.events[i - 1].ts_ms >= e.ts_ms),
    `${tag} events 按时间倒序`
  );
  check(!!v && v.latest_confirmed && v.latest_confirmed.kind === "confirmed", `${tag} latest_confirmed 确实是 confirmed`);
  check(
    !!v && v.latest_confirmed && Number.isFinite(Date.parse(v.latest_confirmed.date_time)),
    `${tag} latest_confirmed 时间可解析`,
    v && v.latest_confirmed ? v.latest_confirmed.date_time : "-"
  );
  check(!!v && v.status && typeof v.status.in_trouble === "boolean", `${tag} status.in_trouble 是布尔`);
  const age = v ? Date.now() - Number(v.generated_at_ms) : NaN;
  check(
    !!v && Number.isFinite(age) && age >= 0 && age < STALE_MS,
    `${tag} 数据没超过 6 小时`,
    Number.isFinite(age) ? `${Math.round(age / 60000)} 分钟前` : "-"
  );
  return v;
}

// ---- Worker 端点：全量 ----
const h = await getText(MIRROR, "/health");
let hv = null;
try {
  hv = JSON.parse(h.body);
} catch {
  /* 保留下面的断言去报错 */
}
check(h.res.status === 200, "worker /health 返回 200", `http=${h.res.status}`);
check(!!hv && hv.ok === true, "worker 镜像未过期", hv ? `age=${hv.age_minutes}分钟 stale=${hv.stale}` : "响应不是 JSON");
check(!!hv && (hv.events ?? 0) > 20, "worker 节点数量合理", hv ? `${hv.events} 条` : "-");
check(!!hv && (hv.confirmed ?? 0) > 10, "worker confirmed 数量合理", hv ? `${hv.confirmed} 条` : "-");
check(!!hv && Array.isArray(hv.errors) && hv.errors.length === 0, "worker 同步无内部告警", hv ? JSON.stringify(hv.errors) : "-");

const wData = await getText(MIRROR, "/data.json");
const wv = checkDataContract("worker", wData);

const p = await getText(MIRROR, "/");
check(p.res.status === 200, "worker 页面返回 200", `http=${p.res.status}`);
check(p.res.headers.get("content-type")?.includes("text/html"), "worker 页面 content-type 是 HTML");
check(p.body.includes("data.json"), "worker 页面确实去取 data.json");

let acao = "";
try {
  const cors = await fetch(MIRROR + "/data.json", {
    method: "OPTIONS",
    headers: { Origin: "https://tauri.localhost" },
  });
  acao = cors.headers.get("access-control-allow-origin") || "";
} catch (e) {
  acao = `抓取失败:${e && e.message}`;
}
check(acao === "*", "worker 允许跨源读 data.json", `acao=${acao}`);

// ---- R2 自定义域名：App 在境内真正该填的那个 ----
const rData = await getText(R2, "/data.json");
const rv = checkDataContract("r2", rData);
check(
  rData.res.headers.get("content-type")?.includes("application/json"),
  "r2 的 content-type 是 JSON",
  `ct=${rData.res.headers.get("content-type")}`
);

// ---- 两条路交叉比对 ----
if (wv && rv) {
  const gap = Math.abs(Number(wv.generated_at_ms) - Number(rv.generated_at_ms));
  // Worker 每 10 分钟重写，允许略大于一个周期；再大就说明其中一条没跟着更新。
  check(gap < 35 * 60 * 1000, "worker 与 r2 是同一条对象的近况", `相差 ${Math.round(gap / 60000)} 分钟`);
  check(
    wv.events.length === rv.events.length,
    "两条路的 events 条数一致",
    `worker=${wv.events.length} r2=${rv.events.length}`
  );
}

console.log(notes.join("\n"));
if (failures.length) {
  console.error(`\n${failures.length} 项失败：\n${failures.join("\n")}`);
  process.exit(1);
}
console.log(`\n镜像健康（worker + r2 双端点），共 ${notes.length} 项检查通过`);
