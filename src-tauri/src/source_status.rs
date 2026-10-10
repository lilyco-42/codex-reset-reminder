use crate::error::{AppError, Result};
use crate::model::StatusInfo;

/// codex-reset.com/api/status 是实测可用的 JSON 接口。
/// 注意一：indicator 可以是 "minor" 同时 degraded=false、incident_count=0，
/// 所以 in_trouble() 不看 indicator。
/// 注意二：实测该接口会限流，返回 200 + `{"error":...,"retry_after":...}`。
/// 如果直接按结构体反序列化，所有字段都会走 serde default，
/// 于是"被限流"会被读成"服务正常"，所以这里先按原始 JSON 判形状。
pub fn parse_status(json: &str) -> Result<StatusInfo> {
    if json.trim().is_empty() {
        return Err(AppError::Parse("状态接口返回空内容".into()));
    }
    let v: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| AppError::Parse(format!("状态接口结构变化：{e}")))?;
    if let Some(err) = v.get("error") {
        let retry = v.get("retry_after").and_then(|r| r.as_i64()).unwrap_or(0);
        return Err(AppError::Http(format!(
            "状态接口拒绝：{err}（约 {retry} 秒后可重试）"
        )));
    }
    if v.get("indicator").is_none() && v.get("codex").is_none() {
        return Err(AppError::Parse(
            "状态接口结构变化：既没有 indicator 也没有 codex".into(),
        ));
    }
    serde_json::from_value::<StatusInfo>(v)
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

    #[test]
    fn rate_limited_error_body_is_never_read_as_healthy() {
        // 实测：限流时返回 200 + {"error":...,"retry_after":...}。
        // 若按结构体直接解，serde default 会让它变成"一切正常"。
        let e = parse_status(r#"{"error":"too many requests","retry_after":118}"#)
            .expect_err("限流响应必须报错，不能当成正常");
        assert!(
            e.to_string().contains("too many requests"),
            "要带上原因：{e}"
        );
        assert!(e.to_string().contains("118"), "要带上重试时间：{e}");
    }

    #[test]
    fn payload_without_expected_fields_is_rejected_not_defaulted() {
        assert!(parse_status(r#"{"something_else":1}"#).is_err());
        assert!(
            parse_status(r#"{"indicator":"none"}"#).is_ok(),
            "只要有 indicator 就能解"
        );
    }
}
