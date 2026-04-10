use serde_json::Value;

/// Parse a JSON array of commands from stdin input.
pub fn parse_commands(input: &str) -> Result<Vec<Value>, crate::BoxError> {
    let parsed: Value = serde_json::from_str(input.trim())
        .map_err(|e| format!("Invalid JSON: {e}"))?;
    let arr = parsed.as_array()
        .ok_or("batch: expected a JSON array of commands")?;
    if arr.is_empty() {
        return Err("batch: empty command array".into());
    }
    Ok(arr.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_array() {
        let input = r#"[{"cmd":"inspect"},{"cmd":"click","uid":"n1"}]"#;
        let cmds = parse_commands(input).unwrap();
        assert_eq!(cmds.len(), 2);
    }

    #[test]
    fn parse_empty_array_errors() {
        assert!(parse_commands("[]").is_err());
    }

    #[test]
    fn parse_invalid_json_errors() {
        assert!(parse_commands("not json").is_err());
    }

    #[test]
    fn parse_non_array_errors() {
        assert!(parse_commands(r#"{"cmd":"inspect"}"#).is_err());
    }
}
