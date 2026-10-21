//! Agent-side structured redaction for diagnostics.

pub fn redact_sensitive(input: &str) -> String {
    let mut value = redact_auth_scheme_tokens(input);
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
    redact_url_userinfo(&value)
}

/// Mask the credential that follows an HTTP auth scheme word
/// (`Bearer <token>`, `Basic <b64>`). Keyed redaction stops at the first space,
/// so `authorization=Bearer abc` would otherwise keep `abc`.
fn redact_auth_scheme_tokens(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    let mut search = 0;
    loop {
        let next = ["bearer ", "basic "]
            .iter()
            .filter_map(|scheme| {
                lower[search..]
                    .find(scheme)
                    .map(|at| (search + at, scheme.len()))
            })
            .min_by_key(|(at, _)| *at);
        let Some((start, scheme_len)) = next else {
            break;
        };
        // Only a scheme in header position counts: at the start, or after
        // `=`, `:` or a quote. Prose such as "basic auth" is left alone.
        let header_position = input[..start]
            .trim_end_matches(' ')
            .as_bytes()
            .last()
            .is_none_or(|byte| matches!(byte, b'=' | b':' | b'"' | b'\''));
        if !header_position {
            search = start + scheme_len;
            continue;
        }
        let mut token_start = start + scheme_len;
        while token_start < bytes.len() && bytes[token_start] == b' ' {
            token_start += 1;
        }
        let mut token_end = token_start;
        while token_end < bytes.len()
            && !matches!(
                bytes[token_end],
                b' ' | b'\t' | b'\n' | b'\r' | b',' | b'"' | b'}' | b'&'
            )
        {
            token_end += 1;
        }
        let token = &input[token_start..token_end];
        // `Basic` carries base64 credentials; prose such as "Basic config"
        // is not one, so only a base64-shaped token of credential length is.
        let credential_shaped = scheme_len != "basic ".len()
            || (token.len() >= 8
                && token.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'=')
                }));
        if !token.is_empty() && token != "[REDACTED]" && credential_shaped {
            output.push_str(&input[cursor..token_start]);
            output.push_str("[REDACTED]");
            cursor = token_end;
        }
        search = token_end.max(start + scheme_len);
    }
    output.push_str(&input[cursor..]);
    output
}

/// Whether a structured log field *name* is a credential key, so the log
/// egress replaces its value whatever its type.
pub fn is_sensitive_field(name: &str) -> bool {
    let normalized = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "password"
            | "credential"
            | "token"
            | "accesstoken"
            | "authtoken"
            | "secret"
            | "cookie"
            | "csrf"
            | "authorization"
            | "rpcurl"
            | "rpcendpoint"
            | "apikey"
            | "privatekey"
            | "key"
    )
}

/// Mask the userinfo of every URL (`wss://user:pass@host` -> `wss://[REDACTED]@host`):
/// an operator may embed RPC or proxy credentials in an endpoint.
fn redact_url_userinfo(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    let mut search = 0;
    while let Some(relative) = input[search..].find("://") {
        let authority_start = search + relative + 3;
        let authority_end = input[authority_start..]
            .find(['/', '?', '#', ' ', '\n', '\r', '\t', '"'])
            .map(|offset| authority_start + offset)
            .unwrap_or(input.len());
        if let Some(at) = input[authority_start..authority_end].rfind('@') {
            output.push_str(&input[cursor..authority_start]);
            output.push_str("[REDACTED]@");
            cursor = authority_start + at + 1;
        }
        search = authority_end;
    }
    output.push_str(&input[cursor..]);
    output
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
    fn masks_url_userinfo_but_keeps_the_host() {
        let result =
            super::redact_sensitive("dial wss://user:p@ss@node:8546/ws and http://plain:80");
        assert_eq!(
            result,
            "dial wss://[REDACTED]@node:8546/ws and http://plain:80"
        );
    }

    #[test]
    fn redacts_transport_credentials_and_tokens() {
        let result = super::redact_sensitive("credential=abc token=xyz rpc_url=wss://x?secret=y");
        assert!(!result.contains("abc"));
        assert!(!result.contains("xyz"));
        assert!(!result.contains("wss://x"));
    }
}
