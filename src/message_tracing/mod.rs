//! Message tracing for debugging
//!
//! Logs full request/response messages to a JSONL file for debugging purposes.

use crate::cli::TracingConfig;
use crate::models::{AnthropicRequest, RouteType};
use crate::providers::ProviderResponse;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use uuid::Uuid;

/// Message tracer that writes to JSONL file.
///
/// Uses a `BufWriter` to amortize syscall cost — each `writeln!` goes to
/// an in-memory buffer that is flushed lazily (when the buffer fills or
/// the tracer is dropped).  This keeps per-request overhead to a single
/// `Mutex::lock` + memcpy, avoiding blocking the tokio worker thread on
/// disk I/O for every request.
pub struct MessageTracer {
    config: TracingConfig,
    writer: Option<Mutex<BufWriter<File>>>,
}

/// A trace entry for a request
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

/// A trace entry for a response
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

/// A trace entry for an error
#[derive(Serialize)]
struct ErrorTrace {
    ts: DateTime<Utc>,
    dir: &'static str,
    id: String,
    error: String,
}

impl MessageTracer {
    /// Create a new tracer from config
    pub fn new(config: TracingConfig) -> Self {
        if !config.enabled {
            return Self { config, writer: None };
        }

        // Expand ~ in path
        let path = expand_tilde(&config.path);

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                tracing::error!("Failed to create tracing directory: {}", e);
                return Self { config, writer: None };
            }
        }

        // Open file for appending
        match OpenOptions::new().create(true).append(true).open(&path) {
            Ok(file) => {
                tracing::info!("📝 Message tracing enabled: {}", path.display());
                Self {
                    config,
                    writer: Some(Mutex::new(BufWriter::new(file))),
                }
            }
            Err(e) => {
                tracing::error!("Failed to open trace file: {}", e);
                Self { config, writer: None }
            }
        }
    }

    /// Generate a new trace ID
    pub fn new_trace_id(&self) -> String {
        if self.writer.is_some() {
            Uuid::new_v4().to_string()[..8].to_string()
        } else {
            String::new()
        }
    }

    /// 返回 trace 文件路径（tracing 未启用时返回 None）
    /// 供 /api/logs 端点读取日志用
    pub fn trace_path(&self) -> Option<PathBuf> {
        if self.writer.is_some() {
            // Flush BufWriter so /api/logs can see recent entries
            if let Some(ref writer_mutex) = self.writer {
                if let Ok(mut writer) = writer_mutex.lock() {
                    let _ = writer.flush();
                }
            }
            Some(expand_tilde(&self.config.path))
        } else {
            None
        }
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
        let Some(ref file_mutex) = self.writer else {
            return;
        };

        // Build messages JSON, optionally omitting system prompt
        let messages = if self.config.omit_system_prompt {
            // Clone request and clear system prompt
            let mut req_clone = request.clone();
            req_clone.system = None;
            serde_json::to_value(&req_clone.messages).unwrap_or_default()
        } else {
            serde_json::to_value(&request.messages).unwrap_or_default()
        };

        let trace = RequestTrace {
            ts: Utc::now(),
            dir: "req",
            id: id.to_string(),
            model: request.model.clone(),
            provider: provider.to_string(),
            route_type: route_type.to_string(),
            is_stream,
            tool_count: request.tools.as_ref().map_or(0, |t| t.len()),
            messages,
        };

        self.write_trace(&trace, file_mutex);
    }

    /// Trace a response
    pub fn trace_response(
        &self,
        id: &str,
        response: &ProviderResponse,
        latency_ms: u64,
    ) {
        let Some(ref file_mutex) = self.writer else {
            return;
        };

        let trace = ResponseTrace {
            ts: Utc::now(),
            dir: "res",
            id: id.to_string(),
            latency_ms,
            stop_reason: response.stop_reason.clone().unwrap_or_default(),
            input_tokens: response.usage.input_tokens,
            output_tokens: response.usage.output_tokens,
            content: serde_json::to_value(&response.content).unwrap_or_default(),
        };

        self.write_trace(&trace, file_mutex);
    }

    /// Trace an error
    pub fn trace_error(&self, id: &str, error: &str) {
        let Some(ref file_mutex) = self.writer else {
            return;
        };

        let trace = ErrorTrace {
            ts: Utc::now(),
            dir: "err",
            id: id.to_string(),
            error: error.to_string(),
        };

        self.write_trace(&trace, file_mutex);
    }

    fn write_trace<T: Serialize>(&self, trace: &T, file_mutex: &Mutex<BufWriter<File>>) {
        let Ok(json) = serde_json::to_string(trace) else {
            return;
        };

        if let Ok(mut writer) = file_mutex.lock() {
            let _ = writeln!(writer, "{}", json);
            // Don't flush every write — BufWriter flushes at 8KB boundary.
            // This amortizes syscall cost across many trace entries.
            // /api/logs reads the file directly (not via the writer), so
            // entries become visible when the buffer fills or on drop.
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
