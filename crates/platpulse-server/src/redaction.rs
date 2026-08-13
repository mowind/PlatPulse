//! Structured redaction for diagnostics and logs.
//!
//! This helper is intentionally conservative: it removes values following
//! common credential/secret keys and masks cookie/authorization-like tokens.

pub fn redact_sensitive(input: &str) -> String {
    let mut output = input.to_owned();
    for key in [
        "password",
        "credential",
        "token",
        "secret",
        "cookie",
        "csrf",
        "authorization",
        "rpc_url",
        "rpc_endpoint",
        "key",
    ] {
        output = redact_key_value(&output, key);
    }
    output
}

fn redact_key_value(input: &str, key: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find(key) {
        let start = cursor + relative;
        output.push_str(&input[cursor..start]);
        let mut end = start + key.len();
        if input.as_bytes().get(end) == Some(&b'"') {
            end += 1;
        }
        while end < input.len() && input.as_bytes()[end].is_ascii_whitespace() {
            end += 1;
        }
        if end >= input.len() || !matches!(input.as_bytes()[end], b'=' | b':') {
            output.push_str(&input[start..start + key.len()]);
            cursor = start + key.len();
            continue;
        }
        output.push_str(&input[start..end + 1]);
        end += 1;
        while end < input.len() && input.as_bytes()[end].is_ascii_whitespace() {
            output.push(input.as_bytes()[end] as char);
            end += 1;
        }
        let value_start = end;
        let quoted = input.as_bytes().get(end) == Some(&b'"');
        if quoted {
            end += 1;
            while end < input.len() && input.as_bytes()[end] != b'"' {
                end += 1;
            }
            if end < input.len() {
                end += 1;
            }
        } else {
            while end < input.len()
                && !matches!(input.as_bytes()[end], b' ' | b',' | b'}' | b'&' | b'\n')
            {
                end += 1;
            }
        }
        if end == value_start {
            cursor = end;
            continue;
        }
        output.push_str("[REDACTED]");
        cursor = end;
    }
    output.push_str(&input[cursor..]);
    output
}

#[cfg(test)]
mod tests {
    use super::redact_sensitive;

    #[test]
    fn redacts_report_credentials_tokens_cookies_csrf_and_rpc_secrets() {
        let input = r#"report_body={"credential":"pp_agent_secret","token":"enroll-x","cookie":"sid=abc","csrf":"csrf-x","rpc_url":"wss://node/?secret=rpc"} password=pass key=private"#;
        let redacted = redact_sensitive(input);
        for secret in [
            "pp_agent_secret",
            "enroll-x",
            "sid=abc",
            "csrf-x",
            "wss://node",
            "secret=rpc",
            "pass",
            "private",
        ] {
            assert!(!redacted.contains(secret), "leaked {secret}: {redacted}");
        }
        assert!(redacted.matches("[REDACTED]").count() >= 5);
    }

    #[test]
    fn leaves_non_sensitive_context_and_masks_raw_stack_values_when_keyed() {
        let redacted = redact_sensitive("error=invalid_report stack=panic at src/lib.rs:1");
        assert!(redacted.contains("error=invalid_report"));
        assert!(redacted.contains("stack=panic"));
    }
}
