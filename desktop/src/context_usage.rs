#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Normal,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Meter {
    pub fraction: f32,
    pub label: String,
    pub detail: String,
    pub severity: Severity,
}

pub fn meter(used: u64, window: u64) -> Option<Meter> {
    if used == 0 || window == 0 {
        return None;
    }
    let fraction = (used as f64 / window as f64).min(1.0) as f32;
    let severity = if fraction >= 0.9 {
        Severity::Error
    } else if fraction >= 0.75 {
        Severity::Warning
    } else {
        Severity::Normal
    };
    Some(Meter {
        fraction,
        label: format!("{} / {}", format_tokens(used), format_tokens(window)),
        detail: format!(
            "Context window: {used} of {window} tokens ({}%)",
            (fraction * 100.0).round() as u32
        ),
        severity,
    })
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        if tokens.is_multiple_of(1_000_000) {
            format!("{}M", tokens / 1_000_000)
        } else {
            format!("{:.1}M", tokens as f64 / 1_000_000.0)
        }
    } else if tokens >= 1_000 {
        if tokens.is_multiple_of(1_000) {
            format!("{}k", tokens / 1_000)
        } else {
            format!("{:.1}k", tokens as f64 / 1_000.0)
        }
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_context_usage_and_thresholds_like_the_established_client() {
        assert_eq!(meter(0, 272_000), None);
        let normal = meter(16_941, 272_000).unwrap();
        assert_eq!(normal.label, "16.9k / 272k");
        assert_eq!(normal.severity, Severity::Normal);
        assert_eq!(normal.detail, "Context window: 16941 of 272000 tokens (6%)");
        assert_eq!(meter(750, 1_000).unwrap().severity, Severity::Warning);
        assert_eq!(meter(900, 1_000).unwrap().severity, Severity::Error);
        assert_eq!(meter(2_000_000, 1_000_000).unwrap().fraction, 1.0);
    }
}
