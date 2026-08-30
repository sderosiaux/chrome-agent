use std::borrow::Cow;

/// Truncate to at most `max_chars` characters, appending `suffix` if truncated.
/// Counts characters, not bytes, so it never splits a multi-byte sequence.
pub fn truncate_str<'a>(s: &'a str, max_chars: usize, suffix: &str) -> Cow<'a, str> {
    if s.chars().count() <= max_chars {
        return Cow::Borrowed(s);
    }
    let byte_idx = s.char_indices().nth(max_chars).map_or(s.len(), |(i, _)| i);
    Cow::Owned(format!("{}{suffix}", &s[..byte_idx]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bug_truncate_ascii() {
        assert_eq!(truncate_str("hello world", 5, "..."), "hello...");
        assert_eq!(truncate_str("short", 10, "..."), "short");
    }

    #[test]
    fn bug_truncate_utf8_multibyte() {
        // Each char is 3 bytes; byte slicing would panic.
        let japanese = "日本語テストデータ";
        let result = truncate_str(japanese, 3, "...");
        assert_eq!(result, "日本語...");
    }

    #[test]
    fn bug_truncate_mixed_utf8() {
        let mixed = "café résumé über";
        let result = truncate_str(mixed, 6, "...");
        assert_eq!(result, "café r...");
    }

    #[test]
    fn bug_truncate_emoji() {
        let emoji = "Hello 🌍🌍🌍 World";
        let result = truncate_str(emoji, 8, "...");
        assert_eq!(result, "Hello 🌍🌍...");
    }

    #[test]
    fn bug_truncate_zero() {
        assert_eq!(truncate_str("hello", 0, "..."), "...");
    }

    #[test]
    fn bug_truncate_empty() {
        assert_eq!(truncate_str("", 5, "..."), "");
    }
}
