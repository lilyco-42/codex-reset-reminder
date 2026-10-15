import * as React from "react";
import { toast } from "sonner";
import { RefreshCw, Settings2, EyeOff, ExternalLink } from "lucide-react";

import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Progress } from "@/components/ui/progress";
import { ScrollArea } from "@/components/ui/scroll-area";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Separator } from "@/components/ui/separator";
import { Skeleton } from "@/components/ui/skeleton";
import { Slider } from "@/components/ui/slider";
import { Switch } from "@/components/ui/switch";
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table";
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from "@/components/ui/tooltip";
import { Toaster } from "@/components/toaster";
import { countdown, fmt, humanBytes, safeUrl, tzOffsetMinutes } from "@/lib/format";
import { cn } from "@/lib/utils";
import {
  crrCheck,
  crrHide,
  crrLocal,
  crrPlatform,
  crrResetHistory,
  crrSetSettings,
  isPreview,
  isRealHost,
  onTrayCheck,
} from "@/lib/ipc";
// 提醒记录的类型在这里叫 ReminderAlert：组件那边已经占了 Alert 这个名字（同名会让 tsc 报 TS2300）
import type { Alert as ReminderAlert, CheckResult, LocalSnapshot, PlatformInfo, Settings, WindowLimit } from "@/lib/types";

type Tone = "" | "ok" | "warn" | "crit" | "quiet";

// Alert 只有 default / destructive 两个变体，这里用 token 色补出 ok / warn / quiet 三种左色条
const TONE_ALERT: Record<Tone, string> = {
  "": "border-l-muted-foreground text-muted-foreground",
  ok: "border-l-ok text-ok",
  warn: "border-l-warn text-warn",
  crit: "border-l-crit text-crit",
  quiet: "border-l-primary text-primary",
};


const DEFAULTS: Settings = {
  poll_minutes: 15,
  warn_percent: 80,
  critical_percent: 95,
  reset_soon_minutes: 15,
  quiet_hours: true,
  quiet_start: 23,
  quiet_end: 7,
  notify_local: true,
  notify_global_reset: true,
  notify_tracker_post: true,
  notify_status_incident: true,
  autostart: false,
  push_url: "",
  mirror_url: "",
};

// 旧版把 80/95 写死在这里，而设置里同名的 warn_percent/critical_percent 并不参与着色。
// 这次重写按"行为不变"处理：仍然写死，差异单独报给用户，不趁换框架偷偷改语义。
const WARN_PCT = 80;
const CRIT_PCT = 95;

function WindowRow({ win, name }: { win: WindowLimit; name: string }) {
  const used = Math.max(0, Math.min(100, Number(win.used_percent) || 0));
  const resetMs = Number(win.resets_at) * 1000;
  const soon = resetMs > Date.now() && resetMs - Date.now() <= 15 * 60000;
  const local = fmt(resetMs);
  const bj = fmt(resetMs, "Asia/Shanghai");
  const when = local === bj ? `${local}（北京时间）` : `本机 ${local} · 北京 ${bj}`;
  const pctTone = used >= CRIT_PCT ? "text-crit" : used >= WARN_PCT ? "text-warn" : "text-foreground";
  return (
    <div className="py-2.5 first:pt-0">
      <div className="flex items-baseline justify-between gap-2">
        <span className="text-[13px]">{name}</span>
        <span className={`tnum text-[17px] font-semibold ${pctTone}`}>{used.toFixed(0)}%</span>
      </div>
      <Progress value={used} className="my-2 h-1.5" aria-label={`${name} 已用百分比`} />
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
        <span className="tnum">{countdown(resetMs)}</span>
        <span className="text-muted-foreground">{when}</span>
      </div>
      {soon && (
        <Badge variant="secondary" className="mt-1.5 text-[11px]">
          即将重置
        </Badge>
      )}
    </div>
  );
}

export default function App() {
  const [result, setResult] = React.useState<CheckResult | null>(null);
  const [local, setLocal] = React.useState<LocalSnapshot | null>(null);
  const [platform, setPlatform] = React.useState<PlatformInfo | null>(null);
  const [status, setStatus] = React.useState<{ text: string; tone: Tone }>({ text: "启动中…", tone: "" });
  const [settings, setSettings] = React.useState<Settings>(DEFAULTS);
  const [settingsOpen, setSettingsOpen] = React.useState(false);
  const [note, setNote] = React.useState("");
  const [busy, setBusy] = React.useState(false);
  const [, setTick] = React.useState(0);
  const timerRef = React.useRef<ReturnType<typeof setInterval> | null>(null);
  const resultRef = React.useRef<CheckResult | null>(null);

  const schedule = React.useCallback((s: Settings) => {
    if (timerRef.current) clearInterval(timerRef.current);
    if (!s.poll_minutes) return;
    timerRef.current = setInterval(() => void check(), Math.max(1, s.poll_minutes) * 60000);
  }, []);

  async function check() {
    setBusy(true);
    try {
      const r = await crrCheck(Date.now(), tzOffsetMinutes());
      resultRef.current = r;
      setResult(r);
      setLocal(r.local ?? null);
      if (r.settings) {
        setSettings(r.settings);
        schedule(r.settings);
      }
      const srcLabel = r.source_used === "mirror" ? "数据源：镜像" : "数据源：直连";
      const err = (r.errors || [])[0];
      if (err) setStatus({ text: `最近一次检查有错误（${srcLabel}）：${err}`, tone: "crit" });
      else if (r.fired?.length)
        setStatus({ text: `已发出 ${r.fired.length} 条提醒 · ${srcLabel} · ${fmt(r.fetched_at_ms)}`, tone: "ok" });
      else setStatus({ text: `最近检查 ${fmt(r.fetched_at_ms)} · ${srcLabel} · 无新提醒`, tone: "ok" });
      if ((r.suppressed_in_quiet || 0) > 0)
        setStatus({ text: `静默时段压住 ${r.suppressed_in_quiet} 条，出时段后会补发。`, tone: "quiet" });
    } catch (e) {
      // 网络失败时仍要刷新本机额度，但不能把失败说成正常
      setStatus({ text: `检查失败：${e}`, tone: "crit" });
      try {
        setLocal(await crrLocal());
      } catch (inner) {
        setStatus({ text: `检查失败：${e}；本机额度也读不到：${inner}`, tone: "crit" });
      }
    } finally {
      setBusy(false);
    }
  }

  React.useEffect(() => {
    let alive = true;
    (async () => {
      if (!isRealHost && !isPreview) {
        setStatus({ text: "未运行在 Tauri 环境中：请用 tauri dev 或打包后的应用打开（或加 ?mock=1 看布局）。", tone: "warn" });
        return;
      }
      try {
        const p = await crrPlatform();
        if (alive) setPlatform(p);
      } catch (e) {
        if (alive) setStatus({ text: `环境信息读取失败：${e}`, tone: "crit" });
      }
      try {
        if (alive) setLocal(await crrLocal());
      } catch {
        /* 本机读不到时 check() 那条路径会说明原因 */
      }
      await check();
    })();
    const sec = setInterval(() => setTick((n) => n + 1), 1000);
    const vis = () => {
      if (!document.hidden) void check();
    };
    document.addEventListener("visibilitychange", vis);
    void onTrayCheck(() => void check());
    return () => {
      alive = false;
      clearInterval(sec);
      if (timerRef.current) clearInterval(timerRef.current);
      document.removeEventListener("visibilitychange", vis);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  async function save() {
    setNote("保存中…");
    try {
      const saved = await crrSetSettings(settings);
      setSettings(saved);
      setNote(`已保存并回读校验 · 间隔 ${saved.poll_minutes} 分钟`);
      schedule(saved);
      toast.success("设置已保存");
    } catch (e) {
      setNote(`保存失败：${e}`);
      toast.error(`保存失败：${e}`);
    }
  }

  async function clearHistory() {
    try {
      await crrResetHistory();
      if (resultRef.current) {
        resultRef.current = { ...resultRef.current, history: [] };
        setResult(resultRef.current);
      }
      setStatus({ text: "提醒记录已清空，下次检查重新开始判断。", tone: "" });
    } catch (e) {
      setStatus({ text: `清空失败：${e}`, tone: "crit" });
    }
  }

  const rl = local?.limits;
  const windows: Array<[WindowLimit, string]> = [];
  if (rl?.primary) windows.push([rl.primary, "5 小时窗口"]);
  if (rl?.secondary) windows.push([rl.secondary, "每周窗口"]);
  const credits = rl?.credits;
  const history: ReminderAlert[] = (result?.history || []).slice().reverse().slice(0, 20);
  const firedKeys = new Set((result?.fired || []).map((a) => a.key));
  const st = result?.status;
  const trouble = !!st && (st.degraded || (st.incident_count || 0) > 0 || (!!st.codex && st.codex !== "operational"));
  const tracker = (result?.tracker || []).slice(0, 6);
  const lastReset = result?.last_global_reset;

  const num = (v: string, d: number) => (Number.isFinite(Number(v)) ? Math.round(Number(v)) : d);

  return (
    <TooltipProvider>
      <div className="flex min-h-screen flex-col bg-background text-foreground">
        <header className="sticky top-0 z-10 flex flex-wrap items-center justify-between gap-2 border-b bg-background/95 px-3.5 py-3 backdrop-blur">
          <div className="flex items-center gap-2">
            <Tooltip>
              <TooltipTrigger asChild>
                <span
                  className={`h-[9px] w-[9px] shrink-0 rounded-full ${
                    status.tone === "ok"
                      ? "bg-ok"
                      : status.tone === "warn"
                        ? "bg-warn"
                        : status.tone === "crit"
                          ? "bg-crit"
                          : status.tone === "quiet"
                            ? "bg-primary"
                            : "bg-muted-foreground"
                  }`}
                  aria-hidden
                />
              </TooltipTrigger>
              <TooltipContent side="bottom" className="max-w-[280px] text-xs">
                {status.text}
              </TooltipContent>
            </Tooltip>
            <h1 className="text-[15px] font-semibold tracking-wide">Codex 额度重置提醒</h1>
            {isPreview && <Badge variant="outline">演示数据</Badge>}
          </div>
          <div className="flex gap-1.5">
            <Button size="sm" onClick={() => void check()} disabled={busy}>
              <RefreshCw data-icon="inline-start" />
              立即检查
            </Button>
            <Button size="sm" variant="outline" onClick={() => setSettingsOpen(true)}>
              <Settings2 data-icon="inline-start" />
              设置
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void crrHide().catch((e) => setStatus({ text: `隐藏失败：${e}`, tone: "crit" }))}>
              <EyeOff data-icon="inline-start" />
              隐藏
            </Button>
          </div>
        </header>

        <Alert className={cn("mt-2 border-l-4 py-3", TONE_ALERT[status.tone])}>
          <AlertDescription className="tnum text-[12.5px]">{status.text}</AlertDescription>
        </Alert>

        <main className="grid flex-1 auto-rows-min gap-2.5 p-3.5 md:grid-cols-2">
          <Card className="md:col-span-2">
            <CardHeader>
              <CardTitle className="text-[11.5px] font-semibold uppercase tracking-widest text-muted-foreground">
                本机额度
              </CardTitle>
              <CardDescription className="text-xs">读 ~/.codex/sessions 尾部，不经网络</CardDescription>
            </CardHeader>
            <CardContent>
              {!local ? (
                <>
                  <Skeleton className="h-4 w-24" />
                  <Skeleton className="mt-3 h-1.5 w-full" />
                  <Skeleton className="mt-6 h-4 w-28" />
                  <Skeleton className="mt-3 h-1.5 w-full" />
                </>
              ) : !rl ? (
                <p className="text-xs text-muted-foreground">该账号没有窗口额度信息。</p>
              ) : windows.length ? (
                windows.map(([w, n]) => <WindowRow key={n} win={w} name={n} />)
              ) : (
                <p className="text-xs text-muted-foreground">该账号没有窗口额度信息。</p>
              )}
              <p className={`mt-2 text-xs ${local?.source === "local" ? "text-muted-foreground" : "text-warn"}`}>
                {local?.source === "local"
                  ? `取自 ${humanBytes(Number(local.file_bytes) || 0)} 的会话日志尾部 · 捕获于 ${fmt(local.captured_at_ms)}`
                  : local?.reason || "本机额度不可用"}
              </p>
              {credits && (
                <p className="mt-1 text-xs text-muted-foreground">
                  {credits.has_credits || credits.unlimited
                    ? `积分：${credits.balance || "0"}${credits.unlimited ? "（不限量）" : ""}`
                    : "当前计划没有额外积分额度"}
                </p>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-[11.5px] font-semibold uppercase tracking-widest text-muted-foreground">
                最近一次全局重置
              </CardTitle>
            </CardHeader>
            <CardContent>
              <p className="tnum mb-1 text-[19px]">
                {lastReset ? `北京时间 ${fmt(lastReset.ts_ms, "Asia/Shanghai")}` : "暂无确认记录"}
              </p>
              {lastReset && (
                <p className="tnum text-xs text-muted-foreground">
                  本机时间 {fmt(lastReset.ts_ms)} · 共 {(result?.tracker || []).length} 条追踪记录
                </p>
              )}
              <Separator className="my-3" />
              <ul className="flex flex-col gap-1.5">
                {tracker.map((x) => (
                  <li key={`${x.kind}-${x.ts_ms}`} className="tnum flex items-center gap-2 text-[12.5px]">
                    <Badge variant={x.kind === "confirmed" ? "default" : "secondary"} className="text-[10.5px]">
                      {x.kind === "confirmed" ? "已确认" : "动态"}
                    </Badge>
                    <span>{fmt(x.ts_ms, "Asia/Shanghai")}</span>
                    {safeUrl(x.source_url) && (
                      <a href={safeUrl(x.source_url)} target="_blank" rel="noreferrer" className="ml-auto inline-flex items-center gap-1">
                        原帖 <ExternalLink className="size-3" />
                      </a>
                    )}
                  </li>
                ))}
              </ul>
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle className="text-[11.5px] font-semibold uppercase tracking-widest text-muted-foreground">
                服务状态
              </CardTitle>
            </CardHeader>
            <CardContent>
              <p className={`tnum mb-1 text-[19px] ${st ? (trouble ? "text-warn" : "text-ok") : "text-warn"}`}>
                {st ? (trouble ? (st.degraded ? "服务降级" : "需要关注") : "正常") : "未取到"}
              </p>
              <p className="text-xs text-muted-foreground">
                {st
                  ? `${st.description || st.indicator || "无描述"} · codex ${st.codex || "未知"} · 事件 ${st.incident_count || 0} 条`
                  : "状态接口不可用"}
              </p>
            </CardContent>
          </Card>

          <Card className="md:col-span-2">
            <CardHeader>
              <CardTitle className="text-[11.5px] font-semibold uppercase tracking-widest text-muted-foreground">
                提醒记录
              </CardTitle>
              <CardDescription className="text-xs">最多显示 20 条，本次新触发的标"新"</CardDescription>
            </CardHeader>
            <CardContent>
              {history.length ? (
                <ScrollArea className="max-h-[260px] pr-3">
                <Table>
                  <TableHeader>
                    <TableRow>
                      <TableHead className="w-[52%]">提醒</TableHead>
                      <TableHead>时间</TableHead>
                      <TableHead className="hidden md:table-cell">详情</TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {history.map((a) => (
                      <TableRow key={a.key} className={a.level === "critical" ? "bg-crit/5" : a.level === "warn" ? "bg-warn/5" : ""}>
                        <TableCell>
                          <span className="font-semibold">{a.title}</span>
                          {firedKeys.has(a.key) && (
                            <Badge variant="outline" className="ml-1.5 text-[10px]">
                              新
                            </Badge>
                          )}
                        </TableCell>
                        <TableCell className="tnum whitespace-nowrap text-muted-foreground">{fmt(a.created_ms)}</TableCell>
                        <TableCell className="hidden md:table-cell">
                          <span className="text-muted-foreground">{a.body}</span>
                          {safeUrl(a.url) && (
                            <a href={safeUrl(a.url)} target="_blank" rel="noreferrer" className="ml-1.5">
                              来源
                            </a>
                          )}
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
                </ScrollArea>
              ) : (
                <p className="text-xs text-muted-foreground">还没有触发过提醒。</p>
              )}
            </CardContent>
          </Card>
        </main>

        <footer className="mt-3 border-t px-3.5 py-3 text-xs text-muted-foreground">
          <div>
            {platform
              ? `${platform.os}/${platform.arch}${platform.mobile ? " · 移动端" : ""} · 配置目录 ${platform.config_dir}`
              : "环境信息未取到"}
          </div>
          <div className="mt-1">
            数据来源：
            <a href="https://codexreset.org" target="_blank" rel="noreferrer" className="mx-1">
              codexreset.org
            </a>
            ·
            <a href="https://codex-reset.com" target="_blank" rel="noreferrer" className="mx-1">
              codex-reset.com
            </a>
            · 本机 ~/.codex/sessions
          </div>
        </footer>

        <Dialog open={settingsOpen} onOpenChange={setSettingsOpen}>
          <DialogContent className="max-h-[85vh] overflow-y-auto sm:max-w-[440px]">
            <DialogHeader>
              <DialogTitle>设置</DialogTitle>
              <DialogDescription className="text-xs">改动在下一次检查前生效；保存后会回读校验。</DialogDescription>
            </DialogHeader>

            <div className="grid grid-cols-2 gap-3">
              <div className="flex flex-col gap-1 text-xs">
                <Label htmlFor="s-poll">检查间隔（分）</Label>
                <Select
                  value={String(settings.poll_minutes)}
                  onValueChange={(v) => setSettings({ ...settings, poll_minutes: num(v, 15) })}
                >
                  <SelectTrigger id="s-poll" size="sm" className="w-full">
                    <SelectValue placeholder="选择间隔" />
                  </SelectTrigger>
                  <SelectContent>
                    {[1, 5, 10, 15, 30, 60].map((m) => (
                      <SelectItem key={m} value={String(m)}>
                        {m} 分钟
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
              <Label className="flex flex-col gap-1 text-xs">
                即将重置提醒（分）
                <Input
                  type="number"
                  min={1}
                  value={settings.reset_soon_minutes}
                  onChange={(e) => setSettings({ ...settings, reset_soon_minutes: num(e.target.value, 15) })}
                />
              </Label>
              <div className="flex flex-col gap-2 text-xs">
                <Label htmlFor="s-warn">提醒阈值 <span className="tnum">{settings.warn_percent}%</span></Label>
                <Slider
                  id="s-warn"
                  min={1}
                  max={100}
                  step={1}
                  value={[settings.warn_percent]}
                  onValueChange={([v]) => setSettings({ ...settings, warn_percent: v })}
                />
              </div>
              <div className="flex flex-col gap-2 text-xs">
                <Label htmlFor="s-crit">严重阈值 <span className="tnum">{settings.critical_percent}%</span></Label>
                <Slider
                  id="s-crit"
                  min={1}
                  max={100}
                  step={1}
                  value={[settings.critical_percent]}
                  onValueChange={([v]) => setSettings({ ...settings, critical_percent: v })}
                />
              </div>
              <Label className="flex flex-col gap-1 text-xs">
                静默开始（时）
                <Input
                  type="number"
                  min={0}
                  max={23}
                  value={settings.quiet_start}
                  onChange={(e) => setSettings({ ...settings, quiet_start: num(e.target.value, 23) })}
                />
              </Label>
              <Label className="flex flex-col gap-1 text-xs">
                静默结束（时）
                <Input
                  type="number"
                  min={0}
                  max={23}
                  value={settings.quiet_end}
                  onChange={(e) => setSettings({ ...settings, quiet_end: num(e.target.value, 7) })}
                />
              </Label>
            </div>

            <div className="grid grid-cols-2 gap-x-3 gap-y-2">
              {(
                [
                  ["quiet_hours", "启用静默时段"],
                  ["notify_local", "本机额度提醒"],
                  ["notify_global_reset", "全局重置提醒"],
                  ["notify_tracker_post", "追踪器动态提醒"],
                  ["notify_status_incident", "服务事件提醒"],
                ] as const
              ).map(([key, text]) => (
                <Label key={key} className="flex flex-row items-center gap-2 text-[13px]">
                  <Switch checked={Boolean(settings[key])} onCheckedChange={(v) => setSettings({ ...settings, [key]: v })} />
                  {text}
                </Label>
              ))}
              {!platform?.mobile && (
                <Label className="flex flex-row items-center gap-2 text-[13px]">
                  <Switch
                    checked={settings.autostart}
                    onCheckedChange={(v) => setSettings({ ...settings, autostart: v })}
                  />
                  开机自启
                </Label>
              )}
            </div>

            <Label className="flex flex-col gap-1 text-xs">
              推送地址（可选）
              <Input
                type="text"
                value={settings.push_url}
                placeholder="https://…"
                onChange={(e) => setSettings({ ...settings, push_url: e.target.value })}
              />
            </Label>
            <Label className="flex flex-col gap-1 text-xs">
              镜像地址（可选）
              <Input
                type="text"
                value={settings.mirror_url}
                placeholder="https://…/data.json"
                onChange={(e) => setSettings({ ...settings, mirror_url: e.target.value })}
              />
            </Label>

            <DialogFooter className="flex-col items-stretch gap-2 sm:flex-row sm:items-center">
              <span className="text-xs text-muted-foreground">{note}</span>
              <div className="flex gap-2 sm:ml-auto">
                <Button variant="outline" size="sm" onClick={() => void clearHistory()}>
                  清空提醒记录
                </Button>
                <Button size="sm" onClick={() => void save()}>
                  保存设置
                </Button>
              </div>
            </DialogFooter>
          </DialogContent>
        </Dialog>

        <Toaster />
      </div>
    </TooltipProvider>
  );
}
