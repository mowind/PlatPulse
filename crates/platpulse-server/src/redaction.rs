//! Structured redaction for diagnostics and logs.
//!
//! This helper is intentionally conservative: it removes values following
//! common credential/secret keys and masks cookie/authorization-like tokens.

use sha2::{Digest, Sha256};

pub fn redact_json_value(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::String(value) => serde_json::Value::String(redact_sensitive(value)),
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(redact_json_value).collect())
        }
        serde_json::Value::Object(values) => serde_json::Value::Object(
            values
                .iter()
                .map(|(key, value)| {
                    let value = if sensitive_json_key(key) {
                        serde_json::Value::String("[REDACTED]".to_owned())
                    } else {
                        redact_json_value(value)
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        value => value.clone(),
    }
}

fn sensitive_json_key(key: &str) -> bool {
    let normalized = key
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .map(|character| character.to_ascii_lowercase())
        .collect::<String>();
    matches!(
        normalized.as_str(),
        "password"
            | "passwordhash"
            | "credential"
            | "credentialdigest"
            | "token"
            | "accesstoken"
            | "refreshtoken"
            | "authtoken"
            | "csrftoken"
            | "secret"
            | "clientsecret"
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

/// Redact a Peer identity without collapsing distinct untrusted identities.
///
/// Plain Peer IDs remain readable for diagnostics; identities that contain
/// sensitive material receive a deterministic, non-reversible fingerprint so
/// current-state primary keys and presence comparisons cannot collide.
pub fn redact_peer_identity(input: &str) -> String {
    let redacted = redact_sensitive(input);
    if redacted == input {
        return input.to_owned();
    }
    let digest = format!("{:x}", Sha256::digest(input.as_bytes()));
    format!("[REDACTED_PEER_{}]", &digest[..16])
}

pub fn redact_sensitive(input: &str) -> String {
    let mut output = input.to_owned();
    for key in [
        "password",
        "password_hash",
        "passwordhash",
        "credential",
        "credential_digest",
        "credentialdigest",
        "token",
        "access_token",
        "accesstoken",
        "refresh_token",
        "refreshtoken",
        "auth_token",
        "authtoken",
        "csrf_token",
        "csrftoken",
        "secret",
        "client_secret",
        "clientsecret",
        "cookie",
        "csrf",
        "authorization",
        "rpc_url",
        "rpcurl",
        "rpc_endpoint",
        "rpcendpoint",
        "api_key",
        "apikey",
        "private_key",
        "privatekey",
        "key",
    ] {
        output = redact_key_value(&output, key);
    }
    redact_url_credentials(&redact_ip_literals(&output))
}

fn redact_url_credentials(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    let mut search = 0;
    while let Some(relative) = input[search..].find("://") {
        let scheme_end = search + relative;
        let authority_start = scheme_end + 3;
        let authority_end = input[authority_start..]
            .find(['/', '?', '#', ' ', '\n', '\r', '\t'])
            .map(|offset| authority_start + offset)
            .unwrap_or(input.len());
        if let Some(at_relative) = input[authority_start..authority_end].find('@') {
            let at = authority_start + at_relative;
            output.push_str(&input[cursor..authority_start]);
            output.push_str("[REDACTED]@");
            cursor = at + 1;
        }
        search = authority_end;
    }
    output.push_str(&input[cursor..]);
    output
}

fn is_ip_candidate(byte: u8) -> bool {
    byte.is_ascii_hexdigit() || matches!(byte, b'.' | b':' | b'%')
}

fn redact_ip_literals(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    let mut index = 0;
    while index < bytes.len() {
        if !is_ip_candidate(bytes[index]) {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && is_ip_candidate(bytes[index]) {
            index += 1;
        }
        let candidate = &input[start..index];
        let Some(ip_end) = parse_ip_prefix(candidate) else {
            // Try each byte so an IP adjacent to an alphanumeric prefix is
            // still masked instead of being hidden inside one failed token.
            index = start + 1;
            continue;
        };
        let absolute_end = start + ip_end;
        output.push_str(&input[cursor..start]);
        output.push_str("[REDACTED_IP]");
        cursor = absolute_end;
        index = absolute_end;
    }
    output.push_str(&input[cursor..]);
    output
}

fn parse_ip_prefix(candidate: &str) -> Option<usize> {
    for end in (1..=candidate.len()).rev() {
        let prefix = &candidate[..end];
        let address = prefix.split('%').next().unwrap_or(prefix);
        if address.parse::<std::net::IpAddr>().is_ok() {
            return Some(address.len());
        }
    }
    None
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

fn redact_key_value(input: &str, key: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let bytes = input.as_bytes();
    let mut output = String::with_capacity(input.len());
    let mut cursor = 0;
    while let Some(relative) = lower[cursor..].find(key) {
        let start = cursor + relative;
        let key_end = start + key.len();
        if (start > 0 && is_word_byte(bytes[start - 1]))
            || (key_end < bytes.len() && is_word_byte(bytes[key_end]))
        {
            output.push_str(&input[cursor..key_end]);
            cursor = key_end;
            continue;
        }

        let mut value_start = key_end;
        if bytes.get(value_start) == Some(&b'"') {
            value_start += 1;
        }
        let whitespace_start = value_start;
        while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
            value_start += 1;
        }
        let has_whitespace = value_start > whitespace_start;
        let has_separator = bytes
            .get(value_start)
            .is_some_and(|byte| matches!(byte, b'=' | b':'));
        if has_separator {
            value_start += 1;
            while value_start < bytes.len() && bytes[value_start].is_ascii_whitespace() {
                value_start += 1;
            }
        } else if !has_whitespace
            || bytes
                .get(value_start)
                .is_none_or(|byte| matches!(byte, b'"' | b',' | b'}' | b'&'))
        {
            output.push_str(&input[cursor..key_end]);
            cursor = key_end;
            continue;
        }

        let quoted = bytes.get(value_start) == Some(&b'"');
        let value_end = if quoted {
            let content_start = value_start + 1;
            input[content_start..]
                .find('"')
                .map(|offset| content_start + offset + 1)
                .unwrap_or(bytes.len())
        } else {
            let mut end = value_start;
            while end < bytes.len()
                && !matches!(
                    bytes[end],
                    b' ' | b'\t' | b',' | b'}' | b'&' | b'\n' | b'\r'
                )
            {
                end += 1;
            }
            end
        };
        if value_end <= value_start {
            output.push_str(&input[cursor..key_end]);
            cursor = key_end;
            continue;
        }
        output.push_str(&input[cursor..value_start]);
        if quoted {
            output.push_str("\"[REDACTED]\"");
        } else {
            output.push_str("[REDACTED]");
        }
        cursor = value_end;
    }
    output.push_str(&input[cursor..]);
    output
}

#[cfg(test)]
mod tests {
    use super::{redact_json_value, redact_peer_identity, redact_sensitive};

    #[test]
    fn redacts_peer_identities_without_collisions() {
        let first = redact_peer_identity("8.8.8.8");
        let second = redact_peer_identity("1.1.1.1");
        assert_ne!(first, second);
        assert!(!first.contains("8.8.8.8"));
        assert!(!second.contains("1.1.1.1"));
        assert_eq!(redact_peer_identity("peer-a"), "peer-a");
    }
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
    fn redacts_camel_case_credential_keys() {
        let value = serde_json::json!({
            "accessToken": "access-secret",
            "apiKey": "api-secret",
            "rpcEndpoint": "wss://user:secret@203.0.113.7:443",
        });
        let redacted = redact_json_value(&value);
        assert_eq!(redacted["accessToken"], "[REDACTED]");
        assert_eq!(redacted["apiKey"], "[REDACTED]");
        assert_eq!(redacted["rpcEndpoint"], "[REDACTED]");
        let text = redact_sensitive("accessToken=access-secret apiKey=api-secret");
        assert!(!text.contains("access-secret"));
        assert!(!text.contains("api-secret"));
    }

    #[test]
    fn redacts_unkeyed_ipv4_and_ipv6_literals() {
        let redacted =
            redact_sensitive("peer=203.0.113.7 endpoint=127.0.0.1:8080 v6=[2001:db8::7]:443");
        assert!(!redacted.contains("203.0.113.7"));
        assert!(!redacted.contains("127.0.0.1"));
        assert!(!redacted.contains("2001:db8::7"));
        assert!(redacted.contains("[REDACTED_IP]"));
    }
    #[test]
    fn redacts_sentence_final_and_space_separated_secrets_without_overmatching_keys() {
        let redacted = redact_sensitive(
            "peer 203.0.113.7. credential abc123 password hunter2 networkKey=stable",
        );
        assert!(!redacted.contains("203.0.113.7"));
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("hunter2"));
        assert!(redacted.contains("networkKey=stable"));
    }
    #[test]
    fn redacts_json_values_without_breaking_json_shape() {
        let value = serde_json::json!({
            "rpc_url": "wss://203.0.113.7/?token=secret",
            "message": "peer 198.51.100.4 failed",
        });
        let redacted = redact_json_value(&value);
        assert!(
            !redacted["rpc_url"]
                .as_str()
                .unwrap()
                .contains("203.0.113.7")
        );
        assert!(
            !redacted["message"]
                .as_str()
                .unwrap()
                .contains("198.51.100.4")
        );
    }
    #[test]
    fn redacts_url_userinfo_and_enode_values() {
        let redacted = redact_sensitive(
            "RPC failed at wss://user:password@203.0.113.7:443 and enode://pubkey@198.51.100.4:30303",
        );
        assert!(!redacted.contains("user:password"));
        assert!(!redacted.contains("203.0.113.7"));
        assert!(!redacted.contains("198.51.100.4"));
    }

    #[test]
    fn leaves_non_sensitive_context_and_masks_raw_stack_values_when_keyed() {
        let redacted = redact_sensitive("error=invalid_report stack=panic at src/lib.rs:1");
        assert!(redacted.contains("error=invalid_report"));
        assert!(redacted.contains("stack=panic"));
    }
}
