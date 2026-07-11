//! /api/logs —— trace.jsonl 日志查看端点
//!
//! 两个端点：
//!   GET  /api/logs        —— 分页读取历史日志（最新在前）
//!   GET  /api/logs/stream —— SSE 实时流（轮询文件 mtime）

use crate::server::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use chrono::{DateTime, Utc};
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tokio::time::Duration;

/// 日志查询参数
#[derive(Deserialize)]
pub struct LogsQuery {
    /// 返回条数上限（默认 100，上限 500）
    #[serde(default = "default_limit")]
    pub limit: usize,
    /// 跳过尾部最近 N 条（偏移量，用于向前翻页）
    #[serde(default)]
    pub offset: usize,
    /// 按 dir 过滤（req/res/err），不填=全部
    pub dir: Option<String>,
    /// 按 trace id 过滤
    pub id: Option<String>,
}

fn default_limit() -> usize {
    100
}

/// 单条日志的 API 视图——精简，不含完整 messages 体（隐私+体积）
/// 现有 RequestTrace 另有 tool_count/messages 2 个字段，LogEntry 不暴露
#[derive(Serialize, Clone)]
pub struct LogEntry {
    pub ts: DateTime<Utc>,
    pub dir: String,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 分页响应
#[derive(Serialize)]
pub struct LogsResponse {
    pub entries: Vec<LogEntry>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    /// tracing 是否启用（未启用时前端提示）
    pub tracing_enabled: bool,
}

/// 从一行 JSON 解析出 LogEntry（宽进宽出：字段缺失跳过，整个行损坏跳过）
fn parse_line(line: &str) -> Option<LogEntry> {
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let dir = v.get("dir")?.as_str()?.to_string();
    let id = v.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
    let ts = v
        .get("ts")
        .and_then(|t| serde_json::from_value(t.clone()).ok())
        .unwrap_or_else(Utc::now);

    let (model, provider, route_type, is_stream) = if dir == "req" {
        (
            v.get("model").map(|m| m.as_str().unwrap_or("").to_string()),
            v.get("provider").map(|p| p.as_str().unwrap_or("").to_string()),
            v.get("route_type").map(|r| r.as_str().unwrap_or("").to_string()),
            v.get("is_stream").and_then(|s| s.as_bool()),
        )
    } else {
        (None, None, None, None)
    };

    let (latency_ms, stop_reason, input_tokens, output_tokens) = if dir == "res" {
        (
            v.get("latency_ms").and_then(|x| x.as_u64()),
            v.get("stop_reason").map(|s| s.as_str().unwrap_or("").to_string()),
            v.get("input_tokens").and_then(|x| x.as_u64()).map(|x| x as u32),
            v.get("output_tokens").and_then(|x| x.as_u64()).map(|x| x as u32),
        )
    } else {
        (None, None, None, None)
    };

    let error = if dir == "err" {
        v.get("error").map(|e| e.as_str().unwrap_or("").to_string())
    } else {
        None
    };

    Some(LogEntry {
        ts,
        dir,
        id,
        model,
        provider,
        route_type,
        is_stream,
        latency_ms,
        stop_reason,
        input_tokens,
        output_tokens,
        error,
    })
}

/// 按 dir/id 过滤
fn matches_filters(entry: &LogEntry, q: &LogsQuery) -> bool {
    if let Some(ref d) = q.dir {
        if entry.dir != *d {
            return false;
        }
    }
    if let Some(ref id) = q.id {
        if entry.id != *id {
            return false;
        }
    }
    true
}

/// 读取整个 trace 文件，返回 (所有解析成功的条目, 总行数)
fn read_all(path: &PathBuf) -> (Vec<LogEntry>, usize) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return (Vec::new(), 0),
    };
    let mut entries = Vec::new();
    let mut total = 0usize;
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        total += 1;
        if let Some(e) = parse_line(line) {
            entries.push(e);
        }
    }
    (entries, total)
}

/// GET /api/logs —— 分页读取历史日志（最新在前）
pub async fn get_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LogsQuery>,
) -> Result<Json<LogsResponse>, (StatusCode, String)> {
    let limit = q.limit.min(500);

    let Some(path) = state.message_tracer.trace_path() else {
        return Ok(Json(LogsResponse {
            entries: Vec::new(),
            total: 0,
            limit,
            offset: q.offset,
            tracing_enabled: false,
        }));
    };

    let (mut entries, _total_lines) = read_all(&path);

    // 过滤
    if q.dir.is_some() || q.id.is_some() {
        entries.retain(|e| matches_filters(e, &q));
    }

    let filtered_total = entries.len();

    // 从尾部向前取（offset 跳过最近 N 条），最新在前
    let start = q.offset;
    let end = (start + limit).min(filtered_total);
    let entries: Vec<LogEntry> = if start >= filtered_total {
        Vec::new()
    } else {
        entries[start..end].iter().rev().cloned().collect()
    };

    Ok(Json(LogsResponse {
        entries,
        total: filtered_total,
        limit,
        offset: q.offset,
        tracing_enabled: true,
    }))
}

/// GET /api/logs/stream —— SSE 实时流（轮询文件 mtime，500ms 间隔）
pub async fn stream_logs(
    State(state): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let path = state
        .message_tracer
        .trace_path()
        .unwrap_or_else(|| PathBuf::from("/dev/null"));

    // 从当前文件末尾开始（只推送新行）
    let initial_pos = std::fs::metadata(&path)
        .map(|m| m.len())
        .unwrap_or(0);

    let stream = stream::unfold(
        (path, initial_pos, SystemTime::now()),
        |(path, pos, mut last_mtime)| async move {
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let metadata = match std::fs::metadata(&path) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let mtime = metadata.modified().unwrap_or(SystemTime::now());
                if mtime == last_mtime {
                    continue;
                }
                let content = match std::fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let bytes = content.as_bytes();
                if (pos as usize) >= bytes.len() {
                    last_mtime = mtime;
                    continue;
                }
                let new_content = &content[pos as usize..];
                let new_pos = bytes.len() as u64;
                let lines: Vec<&str> = new_content.lines().collect();
                let event = Event::default().data(
                    serde_json::json!({ "lines": lines }).to_string(),
                );
                return Some((Ok(event), (path, new_pos, mtime)));
            }
        },
    );

    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_line_req() {
        let line = r#"{"ts":"2026-07-11T05:00:00Z","dir":"req","id":"abc12345","model":"claude-sonnet-4","provider":"nvidia","route_type":"long-context","is_stream":true,"tool_count":0,"messages":[]}"#;
        let e = parse_line(line).expect("req line should parse");
        assert_eq!(e.dir, "req");
        assert_eq!(e.id, "abc12345");
        assert_eq!(e.model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(e.route_type.as_deref(), Some("long-context"));
        assert_eq!(e.is_stream, Some(true));
        assert!(e.latency_ms.is_none()); // req 不含 latency
    }

    #[test]
    fn test_parse_line_res() {
        let line = r#"{"ts":"2026-07-11T05:00:01Z","dir":"res","id":"abc12345","latency_ms":234,"stop_reason":"end_turn","input_tokens":100,"output_tokens":50,"content":[]}"#;
        let e = parse_line(line).expect("res line should parse");
        assert_eq!(e.dir, "res");
        assert_eq!(e.latency_ms, Some(234));
        assert_eq!(e.stop_reason.as_deref(), Some("end_turn"));
        assert_eq!(e.input_tokens, Some(100));
        assert_eq!(e.output_tokens, Some(50));
        assert!(e.model.is_none()); // res 不含 model
    }

    #[test]
    fn test_parse_line_err_and_corrupt() {
        let err_line = r#"{"ts":"2026-07-11T05:00:02Z","dir":"err","id":"abc12345","error":"connection refused"}"#;
        let e = parse_line(err_line).expect("err line should parse");
        assert_eq!(e.dir, "err");
        assert_eq!(e.error.as_deref(), Some("connection refused"));

        // 损坏行应返回 None
        assert!(parse_line("not json").is_none());
        assert!(parse_line(r#"{"no_dir":"x"}"#).is_none());
    }

    #[test]
    fn test_matches_filters() {
        let q_all = LogsQuery {
            limit: 100,
            offset: 0,
            dir: None,
            id: None,
        };
        let q_req = LogsQuery {
            limit: 100,
            offset: 0,
            dir: Some("req".to_string()),
            id: None,
        };
        let q_id = LogsQuery {
            limit: 100,
            offset: 0,
            dir: None,
            id: Some("abc12345".to_string()),
        };
        let req_entry = LogEntry {
            ts: Utc::now(),
            dir: "req".to_string(),
            id: "abc12345".to_string(),
            model: None,
            provider: None,
            route_type: None,
            is_stream: None,
            latency_ms: None,
            stop_reason: None,
            input_tokens: None,
            output_tokens: None,
            error: None,
        };
        let res_entry = LogEntry {
            ts: Utc::now(),
            dir: "res".to_string(),
            id: "xyz".to_string(),
            model: None,
            provider: None,
            route_type: None,
            is_stream: None,
            latency_ms: None,
            stop_reason: None,
            input_tokens: None,
            output_tokens: None,
            error: None,
        };
        assert!(matches_filters(&req_entry, &q_all));
        assert!(matches_filters(&req_entry, &q_req));
        assert!(!matches_filters(&res_entry, &q_req));
        assert!(matches_filters(&req_entry, &q_id));
        assert!(!matches_filters(&res_entry, &q_id));
    }
}
