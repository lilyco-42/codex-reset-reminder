use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};
use crate::model::{RunState, Settings};

/// 沿用 new-api 客户端的做法：配置目录下的纯 JSON，不引入数据库。
/// Unix 上落盘为 0600。
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(bytes)?;
        f.flush()?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)?;
    }
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match std::fs::read(path) {
        Ok(b) => serde_json::from_slice::<T>(&b)
            .map(Some)
            .map_err(|e| AppError::Parse(format!("{} 解析失败：{e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppError::Io(format!("{} 读取失败：{e}", path.display()))),
    }
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn settings_path(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    pub fn load_settings(&self) -> Settings {
        let loaded = read_json::<Settings>(&self.settings_path()).unwrap_or(None);
        match loaded {
            Some(mut s) => {
                // 兜住手改文件造成的非法值。
                if s.poll_minutes < 1 {
                    s.poll_minutes = 1;
                }
                if s.poll_minutes > 180 {
                    s.poll_minutes = 180;
                }
                if !(0.0..=100.0).contains(&s.warn_percent) {
                    s.warn_percent = 80.0;
                }
                if s.critical_percent < s.warn_percent {
                    s.critical_percent = (s.warn_percent + 5.0).min(100.0);
                }
                s.reset_soon_minutes = s.reset_soon_minutes.clamp(1, 240);
                s.quiet_start %= 24;
                s.quiet_end %= 24;
                s
            }
            None => Settings::default(),
        }
    }

    pub fn save_settings(&self, s: &Settings) -> Result<()> {
        write_private(
            &self.settings_path(),
            serde_json::to_vec_pretty(s)?.as_slice(),
        )
    }

    pub fn load_state(&self) -> RunState {
        read_json::<RunState>(&self.state_path())
            .unwrap_or(None)
            .unwrap_or_default()
    }

    pub fn save_state(&self, s: &RunState) -> Result<()> {
        write_private(&self.state_path(), serde_json::to_vec(s)?.as_slice())
    }

    pub fn clear_state(&self) -> Result<()> {
        match std::fs::remove_file(self.state_path()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::Io(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "crr-store-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).expect("临时目录应可创建");
        p
    }

    #[test]
    fn settings_round_trip_and_clamping() {
        let dir = tmp("settings");
        let store = Store::new(dir.clone());
        assert_eq!(
            store.load_settings(),
            Settings::default(),
            "缺文件应回落到默认值"
        );

        let mut s = Settings::default();
        s.poll_minutes = 0;
        s.warn_percent = 250.0;
        s.critical_percent = 10.0;
        s.quiet_start = 30;
        store.save_settings(&s).expect("保存应成功");
        let back = store.load_settings();
        assert_eq!(back.poll_minutes, 1);
        assert_eq!(back.warn_percent, 80.0, "非法百分比应回默认");
        assert!(
            back.critical_percent >= back.warn_percent,
            "临界值不应低于警告值"
        );
        assert!(back.quiet_start < 24);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn state_round_trip_keeps_dedupe_keys() {
        let dir = tmp("state");
        let store = Store::new(dir.clone());
        let mut st = RunState::default();
        st.seeded = true;
        st.mark_notified("warn:每周:123:8", 1_791_635_211_807);
        st.seen_tracker
            .push("confirmed:2026-10-07T03:35:09.000Z".into());
        store.save_state(&st).expect("状态应可落盘");
        let back = store.load_state();
        assert!(back.seeded);
        assert!(back.was_notified("warn:每周:123:8"), "去重键必须跨进程保留");
        assert_eq!(back.seen_tracker.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_state_is_reported_then_replaced_by_default() {
        let dir = tmp("corrupt");
        let store = Store::new(dir.clone());
        std::fs::write(store.state_path(), b"{ not json").unwrap();
        assert!(
            read_json::<RunState>(&store.state_path()).is_err(),
            "损坏文件必须报错，不能静默当成空"
        );
        assert_eq!(store.load_state(), RunState::default(), "上层应能继续运行");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clear_state_is_idempotent() {
        let dir = tmp("clear");
        let store = Store::new(dir.clone());
        store.clear_state().expect("文件不存在也应成功");
        store.save_state(&RunState::default()).unwrap();
        store.clear_state().unwrap();
        assert!(!store.state_path().exists());
        std::fs::remove_dir_all(&dir).ok();
    }
}
