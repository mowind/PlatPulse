//! Agent-side structured redaction for diagnostics.

pub fn redact_sensitive(input: &str) -> String {
    let mut value = input.to_owned();
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
        value = redact_key(&value, key);
    }
    value
}

fn redact_key(input: &str, key: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let mut out = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find(key) {
        let start = cursor + relative;
        out.push_str(&input[cursor..start]);
        let mut end = start + key.len();
        while end < input.len() && input.as_bytes()[end].is_ascii_whitespace() {
            end += 1;
        }
        if end >= input.len() || !matches!(input.as_bytes()[end], b'=' | b':') {
            out.push_str(&input[start..start + key.len()]);
            cursor = start + key.len();
            continue;
        }
        out.push_str(&input[start..=end]);
        end += 1;
        while end < input.len() && input.as_bytes()[end].is_ascii_whitespace() {
            end += 1;
        }
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
        out.push_str("[REDACTED]");
        cursor = end;
    }
    out.push_str(&input[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn redacts_transport_credentials_and_tokens() {
        let result = super::redact_sensitive("credential=abc token=xyz rpc_url=wss://x?secret=y");
        assert!(!result.contains("abc"));
        assert!(!result.contains("xyz"));
        assert!(!result.contains("wss://x"));
    }
}
