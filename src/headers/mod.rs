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
}
