use ody_protocol::config_types::DesignAuditLevel;
use regex_lite::Regex;
use std::sync::OnceLock;

pub fn parse_design_audit_level_command(user_message: &str) -> Option<DesignAuditLevel> {
    static SWITCH_RE: OnceLock<Regex> = OnceLock::new();
    let switch_re = SWITCH_RE.get_or_init(|| {
        Regex::new(r"(?i)^\s*/design-tier\s+(basic|standard|deep)\s*$")
            .expect("design-tier regex should compile")
    });

    switch_re
        .captures(user_message)
        .and_then(|cap| match &cap[1].to_lowercase()[..] {
            "basic" => Some(DesignAuditLevel::Basic),
            "standard" => Some(DesignAuditLevel::Standard),
            "deep" => Some(DesignAuditLevel::Deep),
            _ => None,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_design_audit_level_command_matches() {
        assert_eq!(
            parse_design_audit_level_command("/design-tier basic"),
            Some(DesignAuditLevel::Basic)
        );
        assert_eq!(
            parse_design_audit_level_command("/design-tier standard"),
            Some(DesignAuditLevel::Standard)
        );
        assert_eq!(
            parse_design_audit_level_command("  /design-tier  deep  "),
            Some(DesignAuditLevel::Deep)
        );
    }

    #[test]
    fn parse_design_audit_level_command_ignores_non_commands() {
        assert_eq!(
            parse_design_audit_level_command("Please /design-tier standard now"),
            None
        );
        assert_eq!(
            parse_design_audit_level_command("/design-tier unknown"),
            None
        );
        assert_eq!(
            parse_design_audit_level_command("/design-tier deep extra"),
            None
        );
    }
}
