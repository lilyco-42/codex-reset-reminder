/**
 * Codex 额度重置镜像（Cloudflare Worker + R2）。
 *
 * 与 mirror/sync.py 同一套判据，只是搬到边缘上跑，好处是不碰任何生产服务器。
 * 三条硬规矩（都是踩过坑得来的）：
 *  1. 抓取失败时保留上一次的成功数据，只把错误写进 errors，绝不写成"一切正常"；
 *  2. 状态接口会 200 + {"error","retry_after"} 限流，必须当失败处理；
 *  3. indicator 可以是 minor 而服务其实正常，故障判据不看它。
 */

const NODE_MARK = 'data-testid="reset-timeline-item"';

const CORS = {
  "Access-Control-Allow-Origin": "*",
  "Access-Control-Allow-Methods": "GET, OPTIONS",
};

function json(body, status = 200, extra = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "Content-Type": "application/json; charset=utf-8",
      "Cache-Control": "public, max-age=60",
      ...CORS,
      ...extra,
    },
  });
}

function unescapeAttr(s) {
  return s
    .replace(/&amp;/g, "&")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">");
}

function attr(tag, name) {
  const m = new RegExp(`${name}="([^"]*)"`).exec(tag);
  if (!m) return null;
  const v = unescapeAttr(m[1]);
  return v === "" ? null : v;
}

/** 与 Rust source_org.rs / Python sync.py 同一判据：只认服务端渲染的 data-* 节点。 */
export function parseTracker(html) {
  const events = [];
  let from = 0;
  for (;;) {
    const rel = html.indexOf(NODE_MARK, from);
    if (rel < 0) break;
    const start = html.lastIndexOf("<", rel);
    const end = html.indexOf(">", rel + NODE_MARK.length);
    if (start >= 0 && end > 0) {
      const tag = html.slice(start, end);
      const dt = attr(tag, "data-datetime");
      const kind = attr(tag, "data-kind");
      if (dt && kind) {
        const ms = Date.parse(dt);
        if (Number.isFinite(ms)) {
          events.push({
            date_time: dt,
            ts_ms: ms,
            kind,
            source_url: attr(tag, "data-source-url"),
          });
        }
      }
    }
    from = rel + NODE_MARK.length;
  }
  events.sort((a, b) => b.ts_ms - a.ts_ms);
  return events;
}

/** 限流/结构变化都要抛错，让上层沿用旧数据。 */
export function parseStatus(text) {
  const d = JSON.parse(text);
  if (d && typeof d === "object" && d.error) {
    throw new Error(`状态接口拒绝：${d.error}（${d.retry_after ?? "?"} 秒后重试）`);
  }
  if (!d || (!("indicator" in d) && !("codex" in d))) {
    throw new Error("状态接口结构变化：既没有 indicator 也没有 codex");
  }
  return {
    indicator: d.indicator ?? "",
    description: d.description ?? "",
    codex: d.codex ?? "",
    degraded: !!d.degraded,
    incident_count: d.incident_count ?? 0,
    checked_at: d.checked_at ?? null,
    in_trouble:
      !!d.degraded || (d.incident_count ?? 0) > 0 || (d.codex && d.codex !== "operational"),
  };
}

async function fetchText(url, env) {
  const res = await fetch(url, {
    headers: { "user-agent": "codex-reset-mirror/1.0", "accept-encoding": "gzip, br" },
    cf: { cacheTtl: 60, cacheKey: url },
  });
  if (!res.ok) throw new Error(`${url} -> HTTP ${res.status}`);
  return await res.text();
}

/** 一轮同步。返回写进 R2 的 payload；任何一侧失败都沿用旧值。 */
export async function sync(env, existing) {
  const errors = [];
  let events = existing?.events ?? [];
  let status = existing?.status ?? null;

  try {
    const html = await fetchText(env.TRACKER_URL, env);
    const parsed = parseTracker(html);
    if (parsed.length === 0) {
      errors.push("追踪器解析到 0 个节点，沿用上一次数据");
    } else {
      events = parsed;
    }
  } catch (e) {
    errors.push(`追踪器：${e.message}`);
  }

  try {
    status = parseStatus(await fetchText(env.STATUS_URL, env));
  } catch (e) {
    errors.push(`状态接口：${e.message}`);
  }

  const confirmed = events.filter((e) => e.kind === "confirmed");
  const now = Date.now();
  return {
    generated_at_ms: now,
    // 与 mirror/sync.py 保持同一套顶层键：同一份数据不能有两种 schema。
    generated_at_iso: new Date(now).toISOString().replace(/\.\d{3}Z$/, "Z"),
    generated_at_cst: new Date(now + 8 * 3600_000).toISOString().replace("T", " ").slice(0, 19),
    sources: { tracker: env.TRACKER_URL, status: env.STATUS_URL },
    events,
    latest_confirmed: confirmed[0] ?? null,
    confirmed_count: confirmed.length,
    status,
    errors,
  };
}

async function loadPayload(env) {
  const obj = await env.MIRROR.get("data.json");
  if (!obj) return null;
  try {
    return await obj.json();
  } catch {
    return null;
  }
}

async function doSync(env) {
  const existing = await loadPayload(env);
  const payload = await sync(env, existing);
  await env.MIRROR.put("data.json", JSON.stringify(payload), {
    httpMetadata: { contentType: "application/json; charset=utf-8", cacheControl: "public, max-age=60" },
  });
  return payload;
}

function staleMs(env) {
  const n = Number(env.STALE_MS);
  return Number.isFinite(n) && n > 0 ? n : 6 * 3600_000;
}

const PAGE = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Codex 额度重置镜像</title>
<style>:root{color-scheme:dark}body{margin:0;padding:18px;background:#0f1115;color:#e8ecf3;font:14px/1.6 system-ui,-apple-system,"Segoe UI","Microsoft YaHei",sans-serif}
.wrap{max-width:720px;margin:0 auto}h1{font-size:17px;margin:0 0 4px}.sub{color:#98a2b3;font-size:12.5px;margin:0 0 18px}
.card{background:#171a21;border:1px solid #262b36;border-radius:12px;padding:14px 15px;margin-bottom:12px}
h2{font-size:11.5px;text-transform:uppercase;letter-spacing:.09em;color:#98a2b3;margin:0 0 10px;font-weight:600}
.big{font-size:21px;margin:0;font-variant-numeric:tabular-nums}.ok{color:#4ade80}.warn{color:#fbbf24}
ul{list-style:none;padding:0;margin:8px 0 0}li{display:flex;gap:9px;align-items:baseline;padding:4px 0;font-variant-numeric:tabular-nums}
.kind{font-size:10.5px;padding:2px 6px;border-radius:5px;background:#222733;color:#98a2b3}
.kind.confirmed{background:rgba(74,222,128,.16);color:#4ade80}
a{color:#7dd3c0;text-decoration:none}a:hover{text-decoration:underline}.err{color:#f87171;font-size:12.5px}
.foot{color:#98a2b3;font-size:12px;margin-top:16px}code{background:#222733;padding:1px 5px;border-radius:4px;font-size:12px}</style></head>
<body><div class="wrap"><h1>Codex 额度重置 · 边缘镜像</h1>
<p class="sub">同步自 codexreset.org 与 codex-reset.com，每 10 分钟一次。数据接口 <code>/data.json</code></p>
<div class="card"><h2>最近一次确认的全局重置</h2><p class="big" id="last">加载中…</p><p class="sub" id="ago"></p></div>
<div class="card"><h2>服务状态</h2><p class="big" id="status">—</p><p class="sub" id="statusmeta"></p></div>
<div class="card"><h2>时间线（最近 12 条）</h2><ul id="timeline"></ul></div>
<p class="err" id="errors"></p><p class="foot" id="foot"></p></div>
<script>
const CST=(ms)=>new Date(ms).toLocaleString("zh-CN",{timeZone:"Asia/Shanghai",month:"2-digit",day:"2-digit",hour:"2-digit",minute:"2-digit",hour12:false});
const ago=(ms)=>{const m=Math.max(0,Math.round((Date.now()-ms)/6e4));if(m<60)return m+" 分钟前";if(m<1440)return Math.round(m/60)+" 小时前";return Math.round(m/1440)+" 天前"};
function esc(s){return String(s==null?"":s).replace(/[&<>"']/g,(c)=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]))}
fetch("data.json").then(r=>r.json()).then((D)=>{
  const L=D.latest_confirmed;
  if(L){const el=document.getElementById("last");el.textContent="北京时间 "+CST(L.ts_ms);el.className="big ok";
    document.getElementById("ago").textContent="距今 "+ago(L.ts_ms)+" · 共 "+D.events.length+" 条记录";}
  else document.getElementById("last").textContent="暂无确认记录";
  const s=D.status,st=document.getElementById("status");
  if(s){st.textContent=s.in_trouble?"需要关注":"正常";st.className="big "+(s.in_trouble?"warn":"ok");
    document.getElementById("statusmeta").textContent=(s.description||s.indicator||"")+" · codex "+(s.codex||"未知")+" · 事件 "+(s.incident_count||0)+" 条";}
  else {st.textContent="未取到";st.className="big warn";}
  document.getElementById("timeline").innerHTML=D.events.slice(0,12).map((e)=>'<li><span class="kind '+esc(e.kind)+'">'+(e.kind==="confirmed"?"已确认":"动态")+"</span><span>"+CST(e.ts_ms)+"</span>"+(e.source_url?' · <a href="'+esc(e.source_url)+'" target="_blank" rel="noreferrer">原帖</a>':"")+"</li>").join("");
  document.getElementById("errors").textContent=(D.errors||[]).length?"同步告警："+D.errors.join("；"):"";
  document.getElementById("foot").textContent="生成于 北京时间 "+CST(D.generated_at_ms)+" · 数据版权归原站点";
}).catch((e)=>{document.getElementById("last").textContent="读取 data.json 失败";document.getElementById("errors").textContent=String(e)});
</script></body></html>`;

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    if (req.method === "OPTIONS") return new Response(null, { status: 204, headers: CORS });

    if (url.pathname === "/" || url.pathname === "/index.html") {
      return new Response(PAGE, {
        headers: {
          "Content-Type": "text/html; charset=utf-8",
          "Cache-Control": "public, max-age=300",
          ...CORS,
        },
      });
    }

    if (url.pathname === "/data.json") {
      const payload = await loadPayload(env);
      if (!payload) return json({ error: "镜像还没有数据，等下一次 cron" }, 503);
      return json(payload);
    }

    if (url.pathname === "/health") {
      const payload = await loadPayload(env);
      if (!payload) return json({ ok: false, error: "还没有数据" }, 503);
      const age = Date.now() - payload.generated_at_ms;
      const stale = age > staleMs(env);
      return json(
        {
          ok: !stale,
          stale,
          age_minutes: Math.round(age / 60000),
          events: payload.events.length,
          confirmed: payload.confirmed_count,
          errors: payload.errors,
        },
        stale ? 500 : 200
      );
    }

    // 手动触发一次同步；没设 SYNC_TOKEN 就禁用，避免变成公开的写入口。
    if (url.pathname === "/sync") {
      if (!env.SYNC_TOKEN) return json({ error: "未设置 SYNC_TOKEN，只能用 cron" }, 503);
      if (url.searchParams.get("token") !== env.SYNC_TOKEN) return json({ error: "token 不对" }, 403);
      const payload = await doSync(env);
      return json({ ok: true, events: payload.events.length, errors: payload.errors });
    }

    return new Response("not found", { status: 404, headers: CORS });
  },

  async scheduled(_event, env) {
    await doSync(env);
  },
};
