export interface WindowLimit {
  used_percent: number;
  resets_at: number;
}

export interface Credits {
  has_credits?: boolean;
  unlimited?: boolean;
  balance?: string | number | null;
}

export interface LocalSnapshot {
  source?: string;
  reason?: string;
  file_bytes?: number;
  captured_at_ms?: number;
  limits?: { primary?: WindowLimit | null; secondary?: WindowLimit | null; credits?: Credits | null } | null;
}

export interface TrackerEntry {
  kind: string;
  ts_ms: number;
  source_url?: string | null;
}

export interface StatusInfo {
  degraded?: boolean;
  incident_count?: number;
  description?: string | null;
  indicator?: string | null;
  codex?: string | null;
}

export interface Alert {
  key: string;
  title: string;
  body: string;
  level: string;
  created_ms: number;
  url?: string | null;
}

export interface Settings {
  poll_minutes: number;
  warn_percent: number;
  critical_percent: number;
  reset_soon_minutes: number;
  quiet_hours: boolean;
  quiet_start: number;
  quiet_end: number;
  notify_local: boolean;
  notify_global_reset: boolean;
  notify_tracker_post: boolean;
  notify_status_incident: boolean;
  autostart: boolean;
  push_url: string;
  mirror_url: string;
}

export interface CheckResult {
  local?: LocalSnapshot | null;
  last_global_reset?: { ts_ms: number } | null;
  tracker: TrackerEntry[];
  status?: StatusInfo | null;
  fired?: Alert[];
  history?: Alert[];
  settings?: Settings | null;
  errors?: string[];
  source_used?: string;
  fetched_at_ms?: number;
  suppressed_in_quiet?: number;
}

export interface PlatformInfo {
  os: string;
  arch: string;
  mobile: boolean;
  config_dir: string;
}
