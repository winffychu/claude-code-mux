//! /api/logs —— trace 日志查看端点
//!
//! 两个端点：
//!   GET  /api/logs        —— 分页读取历史日志（最新在前），来源为内存环形缓冲
//!   GET  /api/logs/stream —— SSE 实时流，来源为 broadcast channel（写一条推一条）
//!
//! Architecture (P6): the display source is the in-memory ring buffer in
//! `MessageTracer`, not the trace file.  File output is orthogonal and only
//! exists for `tail -f` CLI debugging — this module never touches the file.

use crate::message_tracing::LogEntry;
use crate::server::AppState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::sse::{Event, KeepAlive, Sse},
    Json,
};
use serde::Deserialize;
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;

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

/// 分页响应
#[derive(serde::Serialize)]
pub struct LogsResponse {
    pub entries: Vec<LogEntry>,
    pub total: usize,
    pub limit: usize,
    pub offset: usize,
    /// tracing 是否启用（未启用时前端提示）
    pub tracing_enabled: bool,
}

/// GET /api/logs —— 从内存环形缓冲分页读取历史日志（最新在前）
pub async fn get_logs(
    State(state): State<Arc<AppState>>,
    Query(q): Query<LogsQuery>,
) -> Result<Json<LogsResponse>, (StatusCode, String)> {
    let limit = q.limit.min(500);
    let inner = state.snapshot();
    let tracer = &inner.message_tracer;

    if !tracer.is_enabled() {
        return Ok(Json(LogsResponse {
            entries: Vec::new(),
            total: 0,
            limit,
            offset: q.offset,
            tracing_enabled: false,
        }));
    }

    let (entries, total) = tracer.read_recent(limit, q.offset, q.dir.as_deref(), q.id.as_deref());

    Ok(Json(LogsResponse {
        entries,
        total,
        limit,
        offset: q.offset,
        tracing_enabled: true,
    }))
}

/// The number of most-recent entries a newly-connected SSE subscriber receives
/// as an initial backlog before switching to live broadcast events.
const SSE_BACKLOG: usize = 100;

/// GET /api/logs/stream —— SSE 实时流，来源为 broadcast channel。
///
/// 先从内存环形缓冲补发最近 `SSE_BACKLOG` 条历史，再订阅 broadcast channel
/// 接收实时新条目。无文件轮询、无 mtime 检查，零延迟。
/// tracing 未启用时只发 keep-alive，不发任何 data event。
pub async fn stream_logs(
    State(state): State<Arc<AppState>>,
) -> Sse<futures::stream::BoxStream<'static, Result<Event, Infallible>>> {
    use futures::stream::StreamExt;

    let inner = state.snapshot();
    let tracer = inner.message_tracer.clone();

    let combined = match tracer.subscribe() {
        Some(rx) => {
            // Initial backlog: most recent entries, oldest-first so the client
            // sees them chronologically before live events arrive.
            let (mut backlog, _) = tracer.read_recent(SSE_BACKLOG, 0, None, None);
            backlog.reverse();

            let backlog_stream = futures::stream::iter(backlog).map(|entry| {
                let payload = serde_json::to_string(&entry).unwrap_or_default();
                Ok::<Event, Infallible>(Event::default().data(payload))
            });

            // Live events from the broadcast channel. Lagged receivers miss
            // dropped entries — acceptable for a live feed.
            let live = BroadcastStream::new(rx)
                .filter_map(|res| {
                    std::future::ready(res.ok().map(|entry| {
                        let payload = serde_json::to_string(&entry).unwrap_or_default();
                        Event::default().data(payload)
                    }))
                })
                .map(Ok::<Event, Infallible>);

            backlog_stream.chain(live).boxed()
        }
        None => futures::stream::empty().boxed(),
    };

    Sse::new(combined).keep_alive(KeepAlive::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_logsquery_defaults() {
        let q = LogsQuery {
            limit: 0, // serde default replaces this, but we test the struct directly
            offset: 0,
            dir: None,
            id: None,
        };
        assert_eq!(q.offset, 0);
        assert!(q.dir.is_none());
        assert!(q.id.is_none());
    }

    #[test]
    fn test_default_limit() {
        assert_eq!(default_limit(), 100);
    }

    #[test]
    fn test_logentry_roundtrips() {
        let entry = LogEntry {
            ts: Utc::now(),
            dir: "req".to_string(),
            id: "abc123".to_string(),
            model: Some("claude-sonnet-4".to_string()),
            provider: Some("nvidia".to_string()),
            route_type: Some("long-context".to_string()),
            is_stream: Some(true),
            latency_ms: None,
            stop_reason: None,
            input_tokens: None,
            output_tokens: None,
            error: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.dir, "req");
        assert_eq!(back.id, "abc123");
        assert_eq!(back.model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(back.route_type.as_deref(), Some("long-context"));
        assert_eq!(back.is_stream, Some(true));
        assert!(back.latency_ms.is_none());
    }
}
