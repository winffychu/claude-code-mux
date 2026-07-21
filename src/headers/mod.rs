// src/headers/mod.rs
// headers 透传与覆写模块
// 负责从入站请求提取客户端 headers，按规则处理后 merge 到出站请求

use axum::http::HeaderMap;

/// 安全过滤：永远不透传的 headers（协议级/安全级）
const BLOCK_LIST: &[&str] = &[
    "host",                // 反向代理不应透传
    "content-length",      // reqwest 自动计算
    "transfer-encoding",   // hop-by-hop
    "connection",          // hop-by-hop
    "upgrade",             // hop-by-hop
    "cookie",              // 安全风险
    "set-cookie",          // 安全风险
    "proxy-authorization", // 代理凭据
    "x-provider",          // CCM 内部路由，不应透传
    "x-forwarded-for",     // P2 预留：保护真实 IP
    "x-real-ip",           // P2 预留：保护真实 IP
    "via",                 // 代理泄露
    "forwarded",           // 代理泄露
    // Auth headers consumed by CCM gateways — never forward to upstreams.
    // Prevents client CCM-auth credentials from leaking to / conflicting with
    // provider credentials (provider sets its own auth in anthropic_compatible).
    "x-api-key",       // LLM client auth (Anthropic convention)
    "authorization",   // LLM client auth (OpenAI Bearer convention)
    "x-ccm-admin-key", // admin UI/API auth (CCM-specific)
];

/// 从入站 HeaderMap 提取可透传的 headers
/// 返回 Vec<(小写key, 值)>，已排除 BLOCK_LIST
pub fn extract_client_forward_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            let key = name.as_str().to_lowercase();
            if BLOCK_LIST.iter().any(|b| *b == key) {
                return None;
            }
            value.to_str().ok().map(|v| (key, v.to_string()))
        })
        .collect()
}

/// 将 forward_headers merge 到 reqwest RequestBuilder
/// existing_keys: provider 内部已设置的 header key 列表（小写）
/// 同名 header 不覆盖 provider 已设值
pub fn merge_forward_headers(
    req_builder: reqwest::RequestBuilder,
    forward_headers: &[(String, String)],
    existing_keys: &[&str],
) -> reqwest::RequestBuilder {
    let existing_lower: Vec<String> = existing_keys.iter().map(|k| k.to_lowercase()).collect();
    forward_headers.iter().fold(req_builder, |rb, (key, value)| {
        if existing_lower.contains(&key.to_lowercase()) {
            rb
        } else {
            rb.header(key.as_str(), value.as_str())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderMap;

    #[test]
    fn test_extract_client_forward_headers_block_list() {
        let mut headers = HeaderMap::new();
        headers.insert("host", "example.com".parse().unwrap());
        headers.insert("cookie", "a=1".parse().unwrap());
        headers.insert("user-agent", "test-agent".parse().unwrap());
        headers.insert("anthropic-beta", "prompt-caching".parse().unwrap());

        let result = extract_client_forward_headers(&headers);
        let keys: Vec<&str> = result.iter().map(|(k, _)| k.as_str()).collect();
        assert!(!keys.contains(&"host"));
        assert!(!keys.contains(&"cookie"));
        assert!(keys.contains(&"user-agent"));
        assert!(keys.contains(&"anthropic-beta"));
    }

    #[test]
    fn test_merge_forward_headers_skip_existing() {
        let forward = vec![
            ("authorization".to_string(), "Bearer x".to_string()),
            ("x-custom".to_string(), "val".to_string()),
        ];
        let client = reqwest::Client::new();
        let rb = client.post("http://localhost");
        let rb = merge_forward_headers(rb, &forward, &["authorization", "content-type"]);
        let req = rb.build().unwrap();
        let headers = req.headers();
        assert!(headers.get("authorization").is_none());
        assert_eq!(headers.get("x-custom").unwrap().to_str().unwrap(), "val");
    }

    #[test]
    fn test_merge_forward_headers_pass_new() {
        let forward = vec![
            ("x-trace".to_string(), "abc123".to_string()),
            ("user-agent".to_string(), "myapp/1.0".to_string()),
        ];
        let client = reqwest::Client::new();
        let rb = client.post("http://localhost");
        let rb = merge_forward_headers(rb, &forward, &["content-type"]);
        let req = rb.build().unwrap();
        let headers = req.headers();
        assert_eq!(headers.get("x-trace").unwrap().to_str().unwrap(), "abc123");
        assert_eq!(
            headers.get("user-agent").unwrap().to_str().unwrap(),
            "myapp/1.0"
        );
    }

    // ===== P1: BLOCK_LIST full coverage =====

    #[test]
    fn test_extract_client_forward_headers_block_list_all() {
        // Test every single BLOCK_LIST entry is filtered out
        let mut headers = HeaderMap::new();
        let blocked = [
            "host",
            "content-length",
            "transfer-encoding",
            "connection",
            "upgrade",
            "cookie",
            "set-cookie",
            "proxy-authorization",
            "x-provider",
            "x-forwarded-for",
            "x-real-ip",
            "via",
            "forwarded",
            "x-api-key",
            "authorization",
            "x-ccm-admin-key",
        ];
        for key in &blocked {
            headers.insert(
                axum::http::HeaderName::from_lowercase(key.as_bytes()).unwrap(),
                "value".parse().unwrap(),
            );
        }
        // Also add a non-blocked header to ensure it passes through
        headers.insert("x-custom-header", "keep-me".parse().unwrap());

        let result = extract_client_forward_headers(&headers);
        let keys: Vec<&str> = result.iter().map(|(k, _)| k.as_str()).collect();

        // None of the BLOCK_LIST entries should appear
        for key in &blocked {
            assert!(
                !keys.contains(key),
                "BLOCK_LIST entry '{}' should have been filtered, but it passed through",
                key
            );
        }
        // The non-blocked custom header should pass
        assert!(keys.contains(&"x-custom-header"));
    }

    #[test]
    fn test_extract_client_forward_headers_empty() {
        // Empty HeaderMap → empty result
        let headers = HeaderMap::new();
        let result = extract_client_forward_headers(&headers);
        assert!(result.is_empty());
    }

    #[test]
    fn test_extract_client_forward_headers_all_allowed() {
        // Mix of common allowed headers → all pass through
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "test/1.0".parse().unwrap());
        headers.insert("anthropic-beta", "prompt-caching".parse().unwrap());
        headers.insert("x-request-id", "req-123".parse().unwrap());
        headers.insert("accept", "application/json".parse().unwrap());

        let result = extract_client_forward_headers(&headers);
        let keys: Vec<&str> = result.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys.len(), 4);
        assert!(keys.contains(&"user-agent"));
        assert!(keys.contains(&"anthropic-beta"));
        assert!(keys.contains(&"x-request-id"));
        assert!(keys.contains(&"accept"));
    }

    #[test]
    fn test_extract_client_forward_headers_case_insensitive() {
        // Headers are case-insensitive in HTTP; extract_client_forward_headers
        // lowercases the key for BLOCK_LIST comparison
        let mut headers = HeaderMap::new();
        headers.insert("Host", "example.com".parse().unwrap()); // Capital H
        headers.insert("COOKIE", "a=1".parse().unwrap()); // All caps
        headers.insert("X-Custom", "val".parse().unwrap()); // Mixed

        let result = extract_client_forward_headers(&headers);
        let keys: Vec<&str> = result.iter().map(|(k, _)| k.as_str()).collect();
        // Both Host and COOKIE should be filtered (case-insensitive BLOCK_LIST match)
        // The keys in result are already lowercased
        assert!(!keys.contains(&"host"));
        assert!(!keys.contains(&"cookie"));
        // X-Custom should pass through as "x-custom"
        assert!(keys.contains(&"x-custom"));
    }

    // ===== P1: merge_forward_headers edge cases =====

    #[test]
    fn test_merge_forward_headers_empty_forward() {
        // Empty forward_headers → no headers added, existing untouched
        let forward: Vec<(String, String)> = vec![];
        let client = reqwest::Client::new();
        let rb = client.post("http://localhost");
        let rb = merge_forward_headers(rb, &forward, &["content-type"]);
        let req = rb.build().unwrap();
        // content-type set by reqwest default (or none), no crash
        let headers = req.headers();
        // No custom headers should have been added
        assert!(headers.get("x-custom").is_none());
        assert!(headers.get("x-trace").is_none());
    }

    #[test]
    fn test_merge_forward_headers_empty_existing_keys() {
        // Empty existing_keys → all forward headers pass through
        let forward = vec![
            ("authorization".to_string(), "Bearer test".to_string()),
            ("x-api-key".to_string(), "test-key".to_string()),
            ("x-custom".to_string(), "val".to_string()),
        ];
        let client = reqwest::Client::new();
        let rb = client.post("http://localhost");
        let rb = merge_forward_headers(rb, &forward, &[]); // empty existing_keys
        let req = rb.build().unwrap();
        let headers = req.headers();
        // All forward headers should be present (nothing protected)
        assert_eq!(
            headers.get("authorization").unwrap().to_str().unwrap(),
            "Bearer test"
        );
        assert_eq!(
            headers.get("x-api-key").unwrap().to_str().unwrap(),
            "test-key"
        );
        assert_eq!(headers.get("x-custom").unwrap().to_str().unwrap(), "val");
    }

    #[test]
    fn test_merge_forward_headers_both_empty() {
        // Both forward_headers and existing_keys empty → no-op
        let forward: Vec<(String, String)> = vec![];
        let client = reqwest::Client::new();
        let rb = client.post("http://localhost");
        let rb = merge_forward_headers(rb, &forward, &[]);
        let req = rb.build().unwrap();
        // Should not crash, req should be buildable
        assert_eq!(req.method(), reqwest::Method::POST);
    }

    #[test]
    fn test_merge_forward_headers_existing_keys_case_insensitive() {
        // existing_keys uses lowercase, forward key is uppercase → should still be protected
        let forward = vec![("Authorization".to_string(), "Bearer x".to_string())];
        let client = reqwest::Client::new();
        let rb = client.post("http://localhost");
        let rb = merge_forward_headers(rb, &forward, &["authorization"]);
        let req = rb.build().unwrap();
        let headers = req.headers();
        // "Authorization" in forward should be skipped because existing_keys contains "authorization"
        // (comparison is case-insensitive)
        assert!(headers.get("authorization").is_none());
        assert!(headers.get("Authorization").is_none());
    }
}
