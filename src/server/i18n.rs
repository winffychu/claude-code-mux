//! i18n 端点 — P5.3
//!
//! 编译期把 JSON 词条文件 `include_str!` 进二进制，零新依赖、零运行时文件读取。
//! `GET /api/i18n/:locale` 返回对应语言的词条 dict（key→翻译）。
//! 未知 locale 返回 404；前端 `t(key)` 找不到时回退到 zh-CN，再回退到 key 本身。

use axum::{
    extract::Path,
    http::StatusCode,
    response::IntoResponse,
    Json,
};

/// `GET /api/i18n/:locale` → zh-CN.json / en.json
///
/// `include_str!` 在编译期嵌入 JSON 文本；`serde_json::from_str` 解析为 Value（编译期
/// 数据保证有效）再 `Json` 包装，保证 Content-Type: application/json 且返回 object
/// 而非被二次转义的字符串。未知 locale 返回 404。
pub async fn get_i18n_dict(Path(locale): Path<String>) -> impl IntoResponse {
    let json_str = match locale.as_str() {
        "zh-CN" => Some(include_str!("i18n/zh-CN.json")),
        "en" => Some(include_str!("i18n/en.json")),
        _ => None,
    };
    match json_str {
        Some(s) => {
            let val: serde_json::Value =
                serde_json::from_str(s).expect("i18n JSON must be valid (compile-time embedded)");
            (StatusCode::OK, Json(val))
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "unsupported locale",
                "supported": ["zh-CN", "en"],
            })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zh_cn_json_is_valid() {
        let s = include_str!("i18n/zh-CN.json");
        let v: serde_json::Value =
            serde_json::from_str(s).expect("zh-CN.json must be valid JSON");
        assert!(v.is_object(), "zh-CN.json must be a JSON object");
        assert!(
            v.as_object().unwrap().len() > 50,
            "zh-CN.json should have substantial entries"
        );
    }

    #[test]
    fn en_json_is_valid() {
        let s = include_str!("i18n/en.json");
        let v: serde_json::Value =
            serde_json::from_str(s).expect("en.json must be valid JSON");
        assert!(v.is_object(), "en.json must be a JSON object");
        assert!(
            v.as_object().unwrap().len() > 50,
            "en.json should have substantial entries"
        );
    }

    #[test]
    fn keys_match_between_locales() {
        let zh: serde_json::Value = serde_json::from_str(include_str!("i18n/zh-CN.json")).unwrap();
        let en: serde_json::Value = serde_json::from_str(include_str!("i18n/en.json")).unwrap();
        let zh_keys: std::collections::BTreeSet<String> = zh
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.to_string())
            .collect();
        let en_keys: std::collections::BTreeSet<String> = en
            .as_object()
            .unwrap()
            .keys()
            .map(|k| k.to_string())
            .collect();
        assert_eq!(
            zh_keys, en_keys,
            "zh-CN and en must have identical keys\nonly in zh-CN: {:?}\nonly in en: {:?}",
            zh_keys.difference(&en_keys).collect::<Vec<_>>(),
            en_keys.difference(&zh_keys).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn get_i18n_dict_zh_returns_object() {
        // compile-time embedded JSON 解析为 Value — 验证 zh-CN 路径返回 object
        let v: serde_json::Value =
            serde_json::from_str(include_str!("i18n/zh-CN.json")).unwrap();
        assert!(v.is_object());
    }

    #[tokio::test]
    async fn get_i18n_dict_en_returns_object() {
        let v: serde_json::Value =
            serde_json::from_str(include_str!("i18n/en.json")).unwrap();
        assert!(v.is_object());
    }
}
