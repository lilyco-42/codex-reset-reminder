#!/usr/bin/env python3
"""把 Codex 额度重置的两个第三方数据源同步成境内可达的静态文件。

设计约束（都是实测出来的，不要随意改）：
- 只用标准库：这台机 2 核 / 可用内存约 719 MB，不装任何包，跑完就退出。
- 输出是静态文件，由 pingap 的 directory 插件直出，不引入常驻进程。
- 原子写：先写 .tmp 再 rename，避免访问者读到写了一半的文件。
- 任一数据源失败时保留上一次的成功数据，只在 JSON 里记录错误，
  绝不把"抓取失败"写成"一切正常"。

用法：
  python3 sync.py --out /var/www/codex-mirror/codex
  python3 sync.py --out DIR --dry-run     # 只打印不写盘
"""

from __future__ import annotations

import argparse
import gzip
import json
import os
import re
import sys
import time
import urllib.request
from datetime import datetime, timedelta, timezone

TRACKER_URL = "https://codexreset.org"
STATUS_URL = "https://codex-reset.com/api/status"
UA = "codex-reset-mirror/1.0 (+https://github.com/lilyco-42/codex-reset-reminder)"
TIMEOUT = 30

# 与 Rust 侧 source_org.rs 对齐的标记；两边结构变了要一起改。
NODE_MARK = 'data-testid="reset-timeline-item"'
CST = timezone(timedelta(hours=8))


def fetch(url: str) -> str:
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept-Encoding": "gzip"})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        raw = resp.read()
        if resp.headers.get("Content-Encoding") == "gzip":
            raw = gzip.decompress(raw)
    return raw.decode("utf-8", errors="replace")


def attr(tag: str, name: str):
    m = re.search(name + r'="([^"]*)"', tag)
    if not m:
        return None
    v = m.group(1)
    v = (
        v.replace("&amp;", "&")
        .replace("&quot;", '"')
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
    )
    return v or None


def parse_tracker(html: str):
    """取服务端渲染的时间线节点，与 App 内 Rust 解析器同一套判据。"""
    events = []
    for m in re.finditer(re.escape(NODE_MARK), html):
        start = html.rfind("<", 0, m.start())
        end = html.find(">", m.end())
        if start < 0 or end < 0:
            continue
        tag = html[start:end]
        dt = attr(tag, "data-datetime")
        kind = attr(tag, "data-kind")
        if not dt or not kind:
            continue
        try:
            ts = datetime.fromisoformat(dt.replace("Z", "+00:00"))
        except ValueError:
            continue
        events.append(
            {
                "date_time": dt,
                "ts_ms": int(ts.timestamp() * 1000),
                "kind": kind,
                "source_url": attr(tag, "data-source-url"),
            }
        )
    events.sort(key=lambda e: e["ts_ms"], reverse=True)
    return events


def parse_status(text: str):
    """实测限流时会返回 200 + {"error","retry_after"}，必须当失败处理。"""
    data = json.loads(text)
    if isinstance(data, dict) and "error" in data:
        raise RuntimeError(f"状态接口拒绝：{data['error']}（{data.get('retry_after')} 秒后重试）")
    if not isinstance(data, dict) or ("indicator" not in data and "codex" not in data):
        raise RuntimeError("状态接口结构变化：既没有 indicator 也没有 codex")
    keep = ("indicator", "description", "codex", "degraded", "incident_count", "checked_at")
    out = {k: data.get(k) for k in keep}
    out["in_trouble"] = bool(
        data.get("degraded")
        or (data.get("incident_count") or 0) > 0
        or (data.get("codex") not in (None, "operational"))
    )
    return out


def load_previous(path: str):
    try:
        with open(path, "r", encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def build_page(payload: dict) -> str:
    """把数据直接内联进 HTML：省一次请求，也避开跨域。"""
    data = json.dumps(payload, ensure_ascii=False).replace("</", "<\\/")
    return PAGE_TMPL.replace("__DATA__", data)


PAGE_TMPL = r"""<!doctype html>
<html lang="zh-CN">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <title>Codex 额度重置 · 境内镜像</title>
    <style>
      :root { color-scheme: dark; }
      body { margin:0; padding:18px; background:#0f1115; color:#e8ecf3;
             font:14px/1.6 system-ui,-apple-system,"Segoe UI","Microsoft YaHei",sans-serif; }
      .wrap { max-width: 720px; margin: 0 auto; }
      h1 { font-size:17px; margin:0 0 4px; }
      .sub { color:#98a2b3; font-size:12.5px; margin:0 0 18px; }
      .card { background:#171a21; border:1px solid #262b36; border-radius:12px;
              padding:14px 15px; margin-bottom:12px; }
      h2 { font-size:11.5px; text-transform:uppercase; letter-spacing:.09em;
           color:#98a2b3; margin:0 0 10px; font-weight:600; }
      .big { font-size:21px; font-variant-numeric:tabular-nums; margin:0; }
      .ok { color:#4ade80; } .warn { color:#fbbf24; }
      ul { list-style:none; padding:0; margin:8px 0 0; }
      li { display:flex; gap:9px; align-items:baseline; padding:4px 0; font-variant-numeric:tabular-nums; }
      .kind { font-size:10.5px; padding:2px 6px; border-radius:5px; background:#222733; color:#98a2b3; }
      .kind.confirmed { background:rgba(74,222,128,.16); color:#4ade80; }
      a { color:#7dd3c0; text-decoration:none; } a:hover { text-decoration:underline; }
      .err { color:#f87171; font-size:12.5px; }
      .foot { color:#98a2b3; font-size:12px; margin-top:16px; }
      code { background:#222733; padding:1px 5px; border-radius:4px; font-size:12px; }
    </style>
  </head>
  <body>
    <div class="wrap">
      <h1>Codex 额度重置 · 境内镜像</h1>
      <p class="sub">同步自 codexreset.org 与 codex-reset.com，每 10 分钟一次。机器地址 <code>/codex/data.json</code></p>
      <div class="card">
        <h2>最近一次确认的全局重置</h2>
        <p class="big" id="last">—</p>
        <p class="sub" id="ago"></p>
      </div>
      <div class="card">
        <h2>服务状态</h2>
        <p class="big" id="status">—</p>
        <p class="sub" id="statusmeta"></p>
      </div>
      <div class="card">
        <h2>时间线（最近 12 条）</h2>
        <ul id="timeline"></ul>
      </div>
      <p class="err" id="errors"></p>
      <p class="foot" id="foot"></p>
    </div>
    <script>
      const DATA = __DATA__;
      const CST = (ms) => new Date(ms).toLocaleString("zh-CN", {
        timeZone: "Asia/Shanghai", month: "2-digit", day: "2-digit",
        hour: "2-digit", minute: "2-digit", hour12: false,
      });
      const ago = (ms) => {
        const m = Math.max(0, Math.round((Date.now() - ms) / 60000));
        if (m < 60) return m + " 分钟前";
        if (m < 1440) return Math.round(m / 60) + " 小时前";
        return Math.round(m / 1440) + " 天前";
      };
      const last = DATA.latest_confirmed;
      if (last) {
        document.getElementById("last").textContent = "北京时间 " + CST(last.ts_ms);
        document.getElementById("last").className = "big ok";
        document.getElementById("ago").textContent = "距今 " + ago(last.ts_ms) + " · 共 " + DATA.events.length + " 条记录";
      } else {
        document.getElementById("last").textContent = "暂无确认记录";
      }
      const st = DATA.status;
      if (st) {
        const el = document.getElementById("status");
        el.textContent = st.in_trouble ? "需要关注" : "正常";
        el.className = "big " + (st.in_trouble ? "warn" : "ok");
        document.getElementById("statusmeta").textContent =
          (st.description || st.indicator || "") + " · codex " + (st.codex || "未知") +
          " · 事件 " + (st.incident_count || 0) + " 条";
      } else {
        document.getElementById("status").textContent = "未取到";
        document.getElementById("status").className = "big warn";
      }
      document.getElementById("timeline").innerHTML = DATA.events.slice(0, 12).map((e) =>
        '<li><span class="kind ' + e.kind + '">' + (e.kind === "confirmed" ? "已确认" : "动态") +
        "</span><span>" + CST(e.ts_ms) + "</span>" +
        (e.source_url ? ' · <a href="' + e.source_url + '" target="_blank" rel="noreferrer">原帖</a>' : "") +
        "</li>"
      ).join("");
      document.getElementById("errors").textContent =
        (DATA.errors || []).length ? "同步告警：" + DATA.errors.join("；") : "";
      document.getElementById("foot").textContent =
        "生成于 北京时间 " + CST(DATA.generated_at_ms) + " · 数据版权归原站点";
    </script>
  </body>
</html>
"""


def atomic_write(path: str, content: str) -> None:
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8", newline="\n") as f:
        f.write(content)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True, help="输出目录（页面与 data.json 都写这里）")
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    errors = []
    prev = load_previous(os.path.join(args.out, "data.json"))

    events = (prev or {}).get("events", [])
    try:
        html = fetch(TRACKER_URL)
        parsed = parse_tracker(html)
        if parsed:
            events = parsed
        else:
            errors.append("追踪器解析到 0 个节点，沿用上一次数据")
    except Exception as e:  # noqa: BLE001 - 同步脚本必须落盘而不是崩
        errors.append(f"追踪器：{e}")

    status = (prev or {}).get("status")
    try:
        status = parse_status(fetch(STATUS_URL))
    except Exception as e:  # noqa: BLE001
        errors.append(f"状态接口：{e}")

    confirmed = [e for e in events if e.get("kind") == "confirmed"]
    payload = {
        "generated_at_ms": int(time.time() * 1000),
        "generated_at_cst": datetime.now(CST).strftime("%Y-%m-%d %H:%M:%S"),
        "sources": {"tracker": TRACKER_URL, "status": STATUS_URL},
        "events": events,
        "latest_confirmed": confirmed[0] if confirmed else None,
        "confirmed_count": len(confirmed),
        "status": status,
        "errors": errors,
    }

    if args.dry_run:
        print(json.dumps(payload, ensure_ascii=False, indent=1)[:1200])
        print(f"\n事件 {len(events)} 条 / confirmed {len(confirmed)} 条 / 错误 {errors}")
        return 0 if not errors else 1

    os.makedirs(args.out, exist_ok=True)
    atomic_write(os.path.join(args.out, "data.json"), json.dumps(payload, ensure_ascii=False, indent=1))
    atomic_write(os.path.join(args.out, "index.html"), build_page(payload))
    print(f"写入 {args.out}：事件 {len(events)}，confirmed {len(confirmed)}，错误 {len(errors)}")
    for e in errors:
        print("  告警:", e)
    # 有错误也退出 0：数据已沿用上一次，不该让 timer 显示失败刷屏。
    return 0


if __name__ == "__main__":
    sys.exit(main())
