use crate::error::{AppError, Result};
use crate::model::StatusInfo;

/// codex-reset.com/api/status 是实测可用的 JSON 接口。
/// 注意：indicator 可以是 "minor" 同时 degraded=false、incident_count=0，
/// 因此 in_trouble() 不看 indicator。
pub fn parse_status(json: &str) -> Result<StatusInfo> {
    if json.trim().is_empty() {
        return Err(AppError::Parse("状态接口返回空内容".into()));
    }
    serde_json::from_str::<StatusInfo>(json)
        .map_err(|e| AppError::Parse(format!("状态接口结构变化：{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const REAL: &str = r#"{"indicator":"minor","description":"Partial System Degradation","codex":"operational","codex_components":5,"surfaces":[{"id":"web","label":"Codex Web","status":"operational"}],"degraded":false,"active_incident":null,"incident_count":0,"checked_at":"2026-10-10T12:30:03.000Z"}"#;

    #[test]
    fn parses_the_measured_payload() {
        let s = parse_status(REAL).expect("实测 payload 应可解析");
        assert_eq!(s.indicator, "minor");
        assert_eq!(s.codex, "operational");
        assert!(!s.degraded);
        assert_eq!(s.incident_count, 0);
        assert_eq!(s.checked_at.as_deref(), Some("2026-10-10T12:30:03.000Z"));
    }

    #[test]
    fn minor_indicator_alone_is_not_an_incident() {
        let s = parse_status(REAL).unwrap();
        assert!(!s.in_trouble(), "实测当时服务正常，不应报警");
    }

    #[test]
    fn degraded_or_non_operational_codex_is_an_incident() {
        let a = parse_status(
            r#"{"indicator":"major","degraded":true,"codex":"operational","incident_count":0}"#,
        )
        .unwrap();
        assert!(a.in_trouble());
        let b = parse_status(
            r#"{"indicator":"none","degraded":false,"codex":"degraded","incident_count":0}"#,
        )
        .unwrap();
        assert!(b.in_trouble());
        let c = parse_status(
            r#"{"indicator":"none","degraded":false,"codex":"operational","incident_count":2}"#,
        )
        .unwrap();
        assert!(c.in_trouble());
    }

    #[test]
    fn unknown_extra_fields_are_ignored() {
        let s = parse_status(r#"{"indicator":"none","future_field":{"a":1}}"#).unwrap();
        assert_eq!(s.indicator, "none");
        assert_eq!(s.incident_count, 0, "缺字段应有默认值");
    }

    #[test]
    fn empty_body_is_reported_as_parse_failure_not_silence() {
        assert!(parse_status("").is_err());
        assert!(parse_status("<html>502</html>").is_err());
    }
}
