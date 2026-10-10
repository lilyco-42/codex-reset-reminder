(function () {
  "use strict";

  const core = window.__TAURI && window.__TAURI.core;
  const ev = window.__TAURI && window.__TAURI.event;
  const $ = (id) => document.getElementById(id);

  if (!core) {
    $("statusline").textContent = "未运行在 Tauri 环境中：请用 tauri dev 或打包后的应用打开。";
    return;
  }
  const invoke = core.invoke;

  let last = null;
  let timer = null;

  // 后端只做整数运算，时区一律取设备当前值。
  const tzOffsetMinutes = () => -new Date().getTimezoneOffset();

  const fmt = (ms, timeZone) => {
    if (ms === null || ms === undefined) return "—";
    const d = new Date(ms);
    if (Number.isNaN(d.getTime())) return "—";
    const o = { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false };
    if (timeZone) o.timeZone = timeZone;
    return new Intl.DateTimeFormat("zh-CN", o).format(d);
  };

  const countdown = (targetMs) => {
    const left = targetMs - Date.now();
    if (!(left > 0)) return "已到点";
    const m = Math.floor(left / 60000);
    if (m < 60) return `还有 ${m} 分`;
    if (m < 1440) return `还有 ${Math.floor(m / 60)} 时 ${m % 60} 分`;
    return `还有 ${Math.floor(m / 1440)} 天 ${Math.floor((m % 1440) / 60)} 时`;
  };

  // 追踪器与状态接口的字符串来自第三方，进 innerHTML 前必须转义。
  const esc = (v) =>
    String(v === null || v === undefined ? "" : v)
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;")
      .replace(/"/g, "&quot;")
      .replace(/'/g, "&#39;");

  const safeUrl = (u) => (typeof u === "string" && /^https?:\/\/[^\s"'<>]+$/i.test(u) ? u : "");

  const link = (u, text) => {
    const s = safeUrl(u);
    return s ? `<a href="${esc(s)}" target="_blank" rel="noreferrer">${esc(text)}</a>` : "";
  };

  function setStatus(text, cls) {
    $("statusline").textContent = text;
    $("statusline").className = "statusline" + (cls ? " " + cls : "");
    $("pulse").className = "dot" + (cls ? " " + cls : "");
  }

  function windowCard(w, name) {
    const used = Math.max(0, Math.min(100, Number(w.used_percent) || 0));
    const resetMs = Number(w.resets_at) * 1000;
    const soon = resetMs > Date.now() && resetMs - Date.now() <= 15 * 60000;
    const local = fmt(resetMs);
    const bj = fmt(resetMs, "Asia/Shanghai");
    // 设备本身就是东八区时，不必把同一个时间印两遍。
    const when = local === bj ? `${local}（北京时间）` : `本机 ${local} · 北京 ${bj}`;
    return `
      <div class="win">
        <div class="win-top">
          <span class="win-name">${esc(name)}</span>
          <span class="win-pct ${used >= 95 ? "crit" : used >= 80 ? "warn" : ""}">${used.toFixed(0)}%</span>
        </div>
        <div class="bar"><i style="width:${used}%"></i></div>
        <div class="win-foot">
          <span data-reset="${resetMs}">${countdown(resetMs)}</span>
          <span class="meta">${when}</span>
        </div>
        ${soon ? '<p class="tag">即将重置</p>' : ""}
      </div>`;
  }

  function renderWindows(snap) {
    const host = $("windows");
    const rl = snap && snap.limits;
    if (!rl) {
      host.innerHTML = "";
      return;
    }
    const items = [];
    if (rl.primary) items.push([rl.primary, "5 小时窗口"]);
    if (rl.secondary) items.push([rl.secondary, "每周窗口"]);
    host.innerHTML = items.length
      ? items.map(([w, n]) => windowCard(w, n)).join("")
      : '<p class="hint">该账号没有窗口额度信息。</p>';
  }

  function renderMeta(snap) {
    if (!snap) return;
    const note = $("local-note");
    if (snap.source === "local") {
      const bytes = Number(snap.file_bytes) || 0;
      const size = bytes > 1048576 ? (bytes / 1048576).toFixed(0) + " MB" : bytes + " B";
      note.textContent = `取自 ${size} 的会话日志尾部 · 捕获于 ${fmt(snap.captured_at_ms)}`;
    } else {
      note.textContent = snap.reason || "本机额度不可用";
    }
    const c = snap.limits && snap.limits.credits;
    $("credits").textContent = c
      ? c.has_credits || c.unlimited
        ? `积分：${c.balance || "0"}${c.unlimited ? "（不限量）" : ""}`
        : "当前计划没有额外积分额度"
      : "";
  }

  function renderGlobal(r) {
    const e = r.last_global_reset;
    $("last-reset").textContent = e ? `北京时间 ${fmt(e.ts_ms, "Asia/Shanghai")}` : "暂无确认记录";
    $("last-reset-local").textContent = e
      ? `本机时间 ${fmt(e.ts_ms)} · 共 ${r.tracker.length} 条追踪记录`
      : "";
    $("timeline").innerHTML = r.tracker
      .slice(0, 6)
      .map(
        (x) => `
        <li>
          <span class="kind ${esc(x.kind)}">${x.kind === "confirmed" ? "已确认" : "动态"}</span>
          <span class="t">${fmt(x.ts_ms, "Asia/Shanghai")}</span>
          ${link(x.source_url, "原帖")}
        </li>`
      )
      .join("");
  }

  function renderStatus(s) {
    if (!s) {
      $("status-text").textContent = "未取到";
      $("status-text").className = "big warn";
      $("status-meta").textContent = "状态接口不可用";
      return;
    }
    const trouble = s.degraded || s.incident_count > 0 || (s.codex && s.codex !== "operational");
    $("status-text").textContent = trouble ? (s.degraded ? "服务降级" : "需要关注") : "正常";
    $("status-text").className = "big " + (trouble ? "warn" : "ok");
    $("status-meta").textContent = `${s.description || s.indicator || "无描述"} · codex ${
      s.codex || "未知"
    } · 事件 ${s.incident_count || 0} 条`;
  }

  function renderAlerts(r) {
    const fired = new Set((r.fired || []).map((a) => a.key));
    const list = (r.history || []).slice().reverse().slice(0, 20);
    $("alerts").innerHTML = list.length
      ? list
          .map(
            (a) => `
        <li class="${fired.has(a.key) ? "new" : ""} ${esc(a.level)}">
          <b>${esc(a.title)}</b>
          <span class="meta">${fmt(a.created_ms)} · ${esc(a.body)}</span>
          ${link(a.url, "来源")}
        </li>`
          )
          .join("")
      : '<li class="empty">还没有触发过提醒。</li>';
    if (r.suppressed_in_quiet > 0) {
      setStatus(`静默时段压住 ${r.suppressed_in_quiet} 条，出时段后会补发。`, "quiet");
    }
  }

  function render(r) {
    last = r;
    const srcLabel = r.source_used === "mirror" ? "数据源：镜像" : "数据源：直连";
    renderWindows(r.local);
    renderMeta(r.local);
    renderGlobal(r);
    renderStatus(r.status);
    renderAlerts(r);
    applySettingsToForm(r.settings);
    $("local-note").className = r.local && r.local.source === "local" ? "hint" : "hint warn";
    const err = (r.errors || [])[0];
    if (err) setStatus(`最近一次检查有错误（${srcLabel}）：${err}`, "warn");
    else if (r.fired && r.fired.length)
      setStatus(`已发出 ${r.fired.length} 条提醒 · ${srcLabel} · ${fmt(r.fetched_at_ms)}`, "ok");
    else setStatus(`最近检查 ${fmt(r.fetched_at_ms)} · ${srcLabel} · 无新提醒`, "ok");
    schedule(r.settings);
  }

  function tickCountdowns() {
    document.querySelectorAll("[data-reset]").forEach((el) => {
      el.textContent = countdown(Number(el.getAttribute("data-reset")));
    });
  }

  function schedule(settings) {
    if (timer) clearInterval(timer);
    if (!settings || !settings.poll_minutes) return;
    timer = setInterval(check, Math.max(1, settings.poll_minutes) * 60000);
  }

  async function check() {
    try {
      const r = await invoke("crr_check", {
        nowMs: Date.now(),
        tzOffsetMinutes: tzOffsetMinutes(),
      });
      render(r);
    } catch (e) {
      // 网络失败时仍要刷新本机额度，但不能把失败说成正常。
      setStatus(`检查失败：${e}`, "crit");
      try {
        const local = await invoke("crr_local");
        renderWindows(local);
        renderMeta(local);
        tickCountdowns();
      } catch (inner) {
        setStatus(`检查失败：${e}；本机额度也读不到：${inner}`, "crit");
      }
    }
  }

  function applySettingsToForm(s) {
    if (!s) return;
    $("s-poll").value = s.poll_minutes;
    $("s-warn").value = Math.round(s.warn_percent);
    $("s-crit").value = Math.round(s.critical_percent);
    $("s-soon").value = s.reset_soon_minutes;
    $("s-qs").value = s.quiet_start;
    $("s-qe").value = s.quiet_end;
    $("s-quiet").checked = !!s.quiet_hours;
    $("s-local").checked = !!s.notify_local;
    $("s-global").checked = !!s.notify_global_reset;
    $("s-post").checked = !!s.notify_tracker_post;
    $("s-status").checked = !!s.notify_status_incident;
    $("s-autostart").checked = !!s.autostart;
    $("s-push").value = s.push_url || "";
    $("s-mirror").value = s.mirror_url || "";
  }

  function formToSettings() {
    const n = (id, d) => {
      const v = Number($("s-" + id).value);
      return Number.isFinite(v) ? Math.round(v) : d;
    };
    return {
      poll_minutes: n("poll", 15),
      warn_percent: n("warn", 80),
      critical_percent: n("crit", 95),
      reset_soon_minutes: n("soon", 15),
      quiet_hours: $("s-quiet").checked,
      quiet_start: n("qs", 23),
      quiet_end: n("qe", 7),
      notify_local: $("s-local").checked,
      notify_global_reset: $("s-global").checked,
      notify_tracker_post: $("s-post").checked,
      notify_status_incident: $("s-status").checked,
      autostart: $("s-autostart").checked,
      push_url: $("s-push").value.trim(),
      mirror_url: $("s-mirror").value.trim(),
    };
  }

  function wire() {
    $("btn-check").addEventListener("click", check);
    $("btn-hide").addEventListener("click", () => {
      invoke("crr_window", { action: "hide" }).catch((e) => setStatus(`隐藏失败：${e}`, "crit"));
    });
    $("btn-settings").addEventListener("click", () => {
      const panel = $("panel-settings");
      panel.hidden = !panel.hidden;
      $("btn-settings").setAttribute("aria-expanded", String(!panel.hidden));
    });
    $("form").addEventListener("submit", async (e) => {
      e.preventDefault();
      $("settings-note").textContent = "保存中…";
      try {
        const saved = await invoke("crr_set_settings", { settings: formToSettings() });
        applySettingsToForm(saved);
        $("settings-note").textContent = `已保存并回读校验 · 间隔 ${saved.poll_minutes} 分钟`;
        schedule(saved);
      } catch (err) {
        $("settings-note").textContent = `保存失败：${err}`;
      }
    });
    $("s-reset").addEventListener("click", async () => {
      try {
        await invoke("crr_reset_history");
        if (last) last.history = [];
        $("alerts").innerHTML = '<li class="empty">记录已清空。</li>';
        setStatus("提醒记录已清空，下次检查重新开始判断。", "");
      } catch (e) {
        setStatus(`清空失败：${e}`, "crit");
      }
    });
    if (ev && ev.listen) {
      ev.listen("crr-tray-check", () => check()).catch(() => {});
    }
    document.addEventListener("visibilitychange", () => {
      if (!document.hidden) check();
    });
  }

  async function boot() {
    wire();
    setInterval(tickCountdowns, 1000);
    try {
      const p = await invoke("crr_platform");
      $("platform").textContent =
        `${p.os}/${p.arch}${p.mobile ? " · 移动端" : ""} · 配置目录 ${p.config_dir}`;
      if (p.mobile) {
        // 移动端没有开机自启这套 API，留着开关只会让人以为已经生效。
        const lbl = $("lbl-autostart");
        if (lbl) lbl.hidden = true;
      }
      const local = await invoke("crr_local");
      renderWindows(local);
      renderMeta(local);
    } catch (e) {
      $("platform").textContent = `环境信息读取失败：${e}`;
    }
    await check();
  }

  boot();
})();
