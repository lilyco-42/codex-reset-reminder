// 第三方字符串会进 DOM，链接必须先过白名单；这套判断是从旧 app.js 原样搬来的，没放宽任何条件。
export const safeUrl = (u: unknown) =>
  typeof u === "string" && /^https?:\/\/[^\s"'<>]+$/i.test(u) ? u : "";

export const fmt = (ms: number | null | undefined, timeZone?: string) => {
  if (ms === null || ms === undefined) return "—";
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "—";
  const o: Intl.DateTimeFormatOptions = {
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hour12: false,
  };
  if (timeZone) o.timeZone = timeZone;
  return new Intl.DateTimeFormat("zh-CN", o).format(d);
};

export const countdown = (targetMs: number) => {
  const left = targetMs - Date.now();
  if (!(left > 0)) return "已到点";
  const m = Math.floor(left / 60000);
  if (m < 60) return `还有 ${m} 分`;
  if (m < 1440) return `还有 ${Math.floor(m / 60)} 时 ${m % 60} 分`;
  return `还有 ${Math.floor(m / 1440)} 天 ${Math.floor((m % 1440) / 60)} 时`;
};

// 后端只做整数运算，时区一律取设备当前值
export const tzOffsetMinutes = () => -new Date().getTimezoneOffset();

export const humanBytes = (bytes: number) =>
  bytes > 1048576 ? `${(bytes / 1048576).toFixed(0)} MB` : `${bytes} B`;
