//! Message tracing for debugging
//!
//! Architecture (since P6): an in-memory ring buffer is the primary display
//! source for `/api/logs` and `/api/logs/stream`.  A new entry is pushed to
//! the ring buffer and broadcast to any subscribed SSE listeners immediately —
//! no file I/O, no BufWriter flush timing, no mtime polling.
//!
//! File output (`config.path`) is optional and orthogonal: when active, full
//! traces (including the messages body) are appended to a JSONL file for
//! `tail -f trace.jsonl` CLI debugging.  The web UI never reads the file.
//!
//! Both sources are gated by `config.enabled`.  When tracing is disabled, the
//! tracer holds no buffer/sender/writer and every trace method is a no-op.

use crate::cli::TracingConfig;
use crate::models::{AnthropicRequest, RouteType};
use crate::providers::ProviderResponse;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::sync::broadcast;
use uuid::Uuid;

/// Minimum ring-buffer capacity (guards against misconfiguration).
const MIN_MAX_ENTRIES: usize = 10;
/// Broadcast channel capacity.  A slow SSE subscriber that falls behind loses
/// old entries (Lagged) — acceptable for a live log feed, not a message bus.
const BROADCAST_CAPACITY: usize = 128;

/// Slim, serializable log entry — the API view consumed by `/api/logs`.
///
/// Deliberately excludes the full `messages`/`content` bodies (privacy + size).
/// The web UI only ever renders these fields.
///
/// Re-exported so `logs.rs` shares the exact same definition the tracer writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Full request trace written to file (omitted from the ring buffer view).
#[derive(Serialize)]
struct RequestTrace {
    ts: DateTime<Utc>,
    dir: &'static str,
    id: String,
    model: String,
    provider: String,
    route_type: String,
    is_stream: bool,
    tool_count: usize,
    messages: serde_json::Value,
}

/// Full response trace written to file.
#[derive(Serialize)]
struct ResponseTrace {
    ts: DateTime<Utc>,
    dir: &'static str,
    id: String,
    latency_ms: u64,
    stop_reason: String,
    input_tokens: u32,
    output_tokens: u32,
    content: serde_json::Value,
}

/// Full error trace written to file.
#[derive(Serialize)]
struct ErrorTrace {
    ts: DateTime<Utc>,
    dir: &'static str,
    id: String,
    error: String,
}

/// In-memory ring buffer storing the most recent `max_entries` log entries.
/// Entries are appended at the back; when full the oldest is popped from the
/// front.  `Vec` slices for pagination are taken newest-first (reverse order).
struct RingBuffer {
    entries: VecDeque<LogEntry>,
    cap: usize,
}

impl RingBuffer {
    fn new(cap: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(cap),
            cap,
        }
    }

    fn push(&mut self, entry: LogEntry) {
        if self.cap == 0 {
            return;
        }
        if self.entries.len() >= self.cap {
            self.entries.pop_front();
        }
        self.entries.push_back(entry);
    }
}

/// Message tracer — writes traces to an in-memory ring buffer (primary) and,
/// optionally, to a JSONL file (for `tail -f` debugging).
///
/// All three storage slots are `Option` and `None` when tracing is disabled, so
/// disabled tracing costs zero memory and every `trace_*` call returns early.
pub struct MessageTracer {
    enabled: bool,
    /// Drop system prompts from file traces (privacy/size). Stored from config
    /// so trace_request does not need to re-read TracingConfig.
    omit: bool,
    // Primary display source for /api/logs
    buffer: Option<Mutex<RingBuffer>>,
    // Live push to /api/logs/stream subscribers
    tx: Option<broadcast::Sender<LogEntry>>,
    // Optional file persistence (full traces incl. messages body)
    writer: Option<Mutex<BufWriter<File>>>,
}

impl MessageTracer {
    /// Create a new tracer from config.
    pub fn new(config: TracingConfig) -> Self {
        if !config.enabled {
            return Self {
                enabled: false,
                omit: config.omit_system_prompt,
                buffer: None,
                tx: None,
                writer: None,
            };
        }

        let omit = config.omit_system_prompt;
        let cap = config.max_entries.max(MIN_MAX_ENTRIES);
        let (buffer, tx) = (
            Some(Mutex::new(RingBuffer::new(cap))),
            Some(broadcast::channel::<LogEntry>(BROADCAST_CAPACITY).0),
        );

        // Optional file persistence.
        let path = expand_tilde(&config.path);
        let writer = open_file_writer(&path).map(|file| Mutex::new(BufWriter::new(file)));
        if writer.is_some() {
            tracing::info!("📝 Message tracing enabled (mem:{}, file:{})", cap, path.display());
        } else {
            tracing::info!("📝 Message tracing enabled (mem:{}, file:off)", cap);
        }

        Self {
            enabled: true,
            omit,
            buffer,
            tx,
            writer,
        }
    }

    /// Whether tracing is active (has a ring buffer).
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Generate a new 8-char trace ID, or an empty string when tracing is off.
    pub fn new_trace_id(&self) -> String {
        if self.enabled {
            Uuid::new_v4().to_string()[..8].to_string()
        } else {
            String::new()
        }
    }

    /// Subscribe to the live broadcast channel for SSE streaming.
    /// Returns `None` when tracing is disabled.
    pub fn subscribe(&self) -> Option<broadcast::Receiver<LogEntry>> {
        self.tx.as_ref().map(|s| s.subscribe())
    }

    /// Read entries from the ring buffer, newest-first, with optional filters
    /// and pagination.  `offset` skips the newest N entries; `limit` caps the
    /// returned slice.  Zero file I/O.
    pub fn read_recent(
        &self,
        limit: usize,
        offset: usize,
        dir: Option<&str>,
        id: Option<&str>,
    ) -> (Vec<LogEntry>, usize) {
        let Some(ref buffer) = self.buffer else {
            return (Vec::new(), 0);
        };
        let guard = match buffer.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        // Collect newest-first.
        let mut filtered: Vec<&LogEntry> = guard
            .entries
            .iter()
            .rev()
            .filter(|e| {
                if let Some(d) = dir {
                    if e.dir != d {
                        return false;
                    }
                }
                if let Some(i) = id {
                    if e.id != i {
                        return false;
                    }
                }
                true
            })
            .collect();
        let total = filtered.len();
        let start = offset.min(total);
        let end = (start + limit).min(total);
        filtered = filtered[start..end].to_vec();
        (
            filtered.into_iter().cloned().collect(),
            total,
        )
    }

    /// Trace an incoming request
    pub fn trace_request(
        &self,
        id: &str,
        request: &AnthropicRequest,
        provider: &str,
        route_type: &RouteType,
        is_stream: bool,
    ) {
        if !self.enabled {
            return;
        }
        let messages = if self.omit {
            let mut req_clone = request.clone();
            req_clone.system = None;
            serde_json::to_value(&req_clone.messages).unwrap_or_default()
        } else {
            serde_json::to_value(&request.messages).unwrap_or_default()
        };
        let now = Utc::now();
        let full = RequestTrace {
            ts: now,
            dir: "req",
            id: id.to_string(),
            model: request.model.clone(),
            provider: provider.to_string(),
            route_type: route_type.to_string(),
            is_stream,
            tool_count: request.tools.as_ref().map_or(0, |t| t.len()),
            messages,
        };
        let entry = LogEntry {
            ts: now,
            dir: "req".to_string(),
            id: id.to_string(),
            model: Some(request.model.clone()),
            provider: Some(provider.to_string()),
            route_type: Some(route_type.to_string()),
            is_stream: Some(is_stream),
            latency_ms: None,
            stop_reason: None,
            input_tokens: None,
            output_tokens: None,
            error: None,
        };
        self.emit(&full, entry);
    }

    /// Trace a response
    pub fn trace_response(
        &self,
        id: &str,
        response: &ProviderResponse,
        latency_ms: u64,
    ) {
        if !self.enabled {
            return;
        }
        let now = Utc::now();
        let full = ResponseTrace {
            ts: now,
            dir: "res",
            id: id.to_string(),
            latency_ms,
            stop_reason: response.stop_reason.clone().unwrap_or_default(),
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
            content: serde_json::to_value(&response.content).unwrap_or_default(),
        };
        let entry = LogEntry {
            ts: now,
            dir: "res".to_string(),
            id: id.to_string(),
            model: None,
            provider: None,
            route_type: None,
            is_stream: None,
            latency_ms: Some(latency_ms),
            stop_reason: response.stop_reason.clone(),
            input_tokens: Some(response.usage.input_tokens),
            output_tokens: Some(response.usage.output_tokens),
            error: None,
        };
        self.emit(&full, entry);
    }

    /// Trace an error
    pub fn trace_error(&self, id: &str, error: &str) {
        if !self.enabled {
            return;
        }
        let now = Utc::now();
        let full = ErrorTrace {
            ts: now,
            dir: "err",
            id: id.to_string(),
            error: error.to_string(),
        };
        let entry = LogEntry {
            ts: now,
            dir: "err".to_string(),
            id: id.to_string(),
            model: None,
            provider: None,
            route_type: None,
            is_stream: None,
            latency_ms: None,
            stop_reason: None,
            input_tokens: None,
            output_tokens: None,
            error: Some(error.to_string()),
        };
        self.emit(&full, entry);
    }

    /// Push one entry to all active sinks (memory ring buffer, broadcast
    /// channel, optional file).  Order: memory → broadcast → file.  Each sink
    /// is independent; a failure in one does not affect the others.
    fn emit<T: Serialize>(&self, full: &T, entry: LogEntry) {
        // 1. Ring buffer (primary display source)
        if let Some(ref buffer) = self.buffer {
            if let Ok(mut guard) = buffer.lock() {
                guard.push(entry.clone());
            }
        }
        // 2. Broadcast (live SSE subscribers).  send errors when there are no
        //    receivers — harmless.  Lagged receivers lose old entries.
        if let Some(ref tx) = self.tx {
            let _ = tx.send(entry);
        }
        // 3. Optional file (full trace for tail -f)
        if let Some(ref writer) = self.writer {
            if let Ok(json) = serde_json::to_string(full) {
                if let Ok(mut w) = writer.lock() {
                    let _ = writeln!(w, "{json}");
                    // Flush so `tail -f` sees entries promptly; the write
                    // volume is low and this is an opt-in debug sink.
                    let _ = w.flush();
                }
            }
        }
    }
}

/// Expand ~ to home directory
fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
}

/// Open (or create) the trace file for appending.  Returns `None` on failure
/// (the tracer degrades to memory-only without erroring).
fn open_file_writer(path: &PathBuf) -> Option<File> {
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return None;
        }
    }
    OpenOptions::new().create(true).append(true).open(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dir: &str, id: &str, model: Option<&str>) -> LogEntry {
        LogEntry {
            ts: Utc::now(),
            dir: dir.to_string(),
            id: id.to_string(),
            model: model.map(|s| s.to_string()),
            provider: None,
            route_type: None,
            is_stream: None,
            latency_ms: None,
            stop_reason: None,
            input_tokens: None,
            output_tokens: None,
            error: None,
        }
    }

    #[test]
    fn ring_buffer_evicts_oldest_when_full() {
        let mut rb = RingBuffer::new(3);
        rb.push(entry("req", "1", None));
        rb.push(entry("req", "2", None));
        rb.push(entry("req", "3", None));
        rb.push(entry("req", "4", None)); // should evict "1"
        assert_eq!(rb.entries.len(), 3);
        // oldest (front) is now "2", newest (back) is "4"
        assert_eq!(rb.entries.front().unwrap().id, "2");
        assert_eq!(rb.entries.back().unwrap().id, "4");
    }

    #[test]
    fn ring_buffer_zero_capacity_drops_all() {
        let mut rb = RingBuffer::new(0);
        rb.push(entry("req", "1", None));
        assert_eq!(rb.entries.len(), 0);
    }

    fn enabled_tracer(cap: usize) -> MessageTracer {
        MessageTracer::new(TracingConfig {
            enabled: true,
            max_entries: cap,
            // path intentionally bogus so the file writer is None (memory-only)
            path: "/dev/null/does-not-exist-dir/trace.jsonl".to_string(),
            omit_system_prompt: true,
        })
    }

    #[test]
    fn read_recent_returns_newest_first_with_pagination() {
        let tracer = enabled_tracer(100);
        tracer.trace_error("id1", "e1");
        tracer.trace_error("id2", "e2");
        tracer.trace_error("id3", "e3");

        let (all, total) = tracer.read_recent(100, 0, None, None);
        assert_eq!(total, 3);
        assert_eq!(all.len(), 3);
        // newest first
        assert_eq!(all[0].id, "id3");
        assert_eq!(all[2].id, "id1");

        // offset skips newest 1, limit 1 → only id2
        let (page, _) = tracer.read_recent(1, 1, None, None);
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, "id2");

        // offset beyond total → empty
        let (beyond, _) = tracer.read_recent(10, 10, None, None);
        assert!(beyond.is_empty());
    }

    #[test]
    fn read_recent_filters_by_id() {
        let tracer = enabled_tracer(100);
        tracer.trace_error("err1", "boom");
        tracer.trace_error("err2", "boom2");
        tracer.trace_error("err3", "boom3");

        let (all, _) = tracer.read_recent(100, 0, Some("err"), None);
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|e| e.dir == "err"));

        let (one, _) = tracer.read_recent(100, 0, None, Some("err2"));
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].id, "err2");
    }

    #[test]
    fn disabled_tracer_is_noop() {
        let tracer = MessageTracer::new(TracingConfig::default()); // enabled=false
        assert!(!tracer.is_enabled());
        assert!(tracer.new_trace_id().is_empty());
        assert!(tracer.subscribe().is_none());
        tracer.trace_error("x", "y");
        let (entries, _) = tracer.read_recent(10, 0, None, None);
        assert!(entries.is_empty());
    }

    #[test]
    fn broadcast_delivers_to_subscriber() {
        let tracer = enabled_tracer(100);
        let mut rx = tracer.subscribe().expect("enabled tracer has a channel");
        tracer.trace_error("live1", "boom");
        // broadcast::Receiver::recv is blocking; in a test that's fine.
        let received = rx.blocking_recv();
        assert!(received.is_ok());
        assert_eq!(received.unwrap().id, "live1");
    }
}
