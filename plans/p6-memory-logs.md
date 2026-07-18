# P6: 内存日志架构 — 解耦文件存储

> 来源：用户反馈"现在网页日志是必须存文件的也不合理"
> 状态：方案确认（2026-07-16，用户拍板 A1+B1），待实施
> 关联：p5-web-refactor.md（日志页设计基础，§8 Q4 `trace.jsonl` 决策的前提被本方案改写）

---

## 1. 背景与问题

### 1.1 现状

`/api/logs`（`src/server/logs.rs:169`）与 `/api/logs/stream`（`src/server/logs.rs:217`）强耦合 `trace.jsonl` 文件：

- `get_logs` → `trace_path()` flush BufWriter → `read_all()` 用 `read_to_string` 把整个文件读进内存 → 过滤 + 分页
- `stream_logs` → 每 500ms 轮询文件 mtime，变了就 `read_to_string` 整文件，取增量行

### 1.2 三个问题

| # | 问题 | 根因 |
|---|------|------|
| 1 | **网页日志"必须存文件"不合理** | `/api/logs` 的唯一数据源是磁盘文件；即使只想在 admin UI 看几条请求，也必须开启 tracing→写文件→读文件 |
| 2 | **低流量不可见** | BufWriter 8KB 懒 flush，小数据停在内存 buffer；`trace_path()` 虽有 flush 但依赖调用时机（上一轮已加每次写后 flush 临时缓解，治标不治本） |
| 3 | **SSE 轮询笨重** | 每 500ms 全量 `read_to_string` 整文件只为检测增量，高频请求时文件越大 I/O 越重 |

### 1.3 关键事实（审计确认）

- **前端从不读完整 messages 体**：`logEntryHtml`（admin.html:2093）只展示精简字段（ts/dir/id/model/provider/route_type/is_stream/latency_ms/tokens/error），无详情展开端点。完整 messages 占一条 trace 90%+ 体积，却从不被网页消费。
- **精简 LogEntry 极轻量**：约 150-250 bytes/条。2000 条 ≈ 400KB 内存，完全可接受，推翻 p5 §3.2"环形缓冲占内存"的原顾虑。
- **文件另有手动调试价值**：README.md:940 示例 `tail -f trace.jsonl`，部分用户习惯 CLI 看完整 messages。故文件写入不废除，降级为可选。

---

## 2. 方案（用户拍板 A1+B1）

### 2.1 设计原则

- **内存环形缓冲为网页显示主源**：`/api/logs` 和 `/api/logs/stream` 直接读内存，零文件 I/O
- **文件写入保留为可选**：`tracing.enabled` + `path` 仍有效（默认关），给 `tail -f` 调试者；与内存缓冲并行写（是双写，不是二选一）
- **broadcast channel 推 SSE**：去掉 mtime 轮询，写一条推一条
- **容量可配**：新增 `tracing.max_entries`，默认 2000

### 2.2 数据流

```
请求进入 → trace_request/response/error
          ├─（若 file_enabled）─→ BufWriter → trace.jsonl   [可选，默认关]
          └─→ 环形缓冲 push + broadcast::send               [始终，若 tracing 开启]

GET /api/logs        ─→ 读环形缓冲 VM → filter + paginate → LogEntry[]
GET /api/logs/stream ─→ 订阅 broadcast channel → 实时推新条目
```

注意：**内存缓冲的开关跟随 `tracing.enabled`**——tracing 关时无内存缓冲（省内存），与当前 "tracing 关=无日志" 语义一致。文件写入单独有 `path` 配置（有 path 就是"持久化"意图）。即：
- `enabled=false`：无内存、无文件（现状）
- `enabled=true`，`path` 有效：**内存 + 文件双写**
- `enabled=true`，`path` 为空/无效：仅内存

### 2.3 配置变更（`src/cli/mod.rs` TracingConfig）

```rust
pub struct TracingConfig {
    pub enabled: bool,           // 不变，默认 false

    /// 环形缓冲容量（新增）。默认 2000。tracing 关时无缓冲。
    #[serde(default = "default_max_entries")]
    pub max_entries: usize,

    /// 可选：文件持久化路径。为空则不落磁盘（纯内存）。
    /// 仍支持 ~ 展开。默认值指向 ~/.claude-code-mux/trace.jsonl（向后兼容旧配置）。
    #[serde(default = "default_tracing_path")]
    pub path: String,

    pub omit_system_prompt: bool,  // 不变，默认 true
}
```

`default_max_entries()` → 2000。旧配置无 `max_entries` 字段时 serde default 填充，向后兼容。

### 2.4 MessageTracer 重构（`src/message_tracing/mod.rs`）

```rust
pub struct MessageTracer {
    config: TracingConfig,
    enabled: bool,
    // 内存缓冲主源
    buffer: Option<RingBuffer<LogEntry>>,              // Mutex<VecDeque<LogEntry>> + cap
    // SSE 实时推送
    tx: Option<broadcast::Sender<LogEntry>>,           // capacity=32 容清 backlog
    // 可选文件落盘
    writer: Option<Mutex<BufWriter<File>>>,
}
```

- `trace_request/response/error`：
  1. 构建 `RequestTrace`/`ResponseTrace`/`ErrorTrace`（完整体，给文件用）
  2. 构建精简 `LogEntry`（复用 rifles.rs 逻辑）
  3. 若 `buffer.活跃` → push 进环形过滤新条
  4. 若 `tx.活跃` → `tx.send(LogEntry)`（lagged receiver 容忍丢老数据）
  5. 若 `writer.活跃` → writeln + flush（给 tail -f 的人，与现状一致）

- 新增 `read_recent(&self, limit, offset, filter) -> Vec<LogEntry>`：读环形缓冲，latest-first，零文件 I/O。替换 flags.rs 当前 `read_all`+`trace_path` 流程。
- `new_trace_id`：`enabled` 时返回，而非 `writer.is_some()`。
- 删除 `trace_path()` 对外的 flush 语义——内存源不需要。但保留返回 `Option<PathBuf>` 给"文件是否在写"状态查询（如有 UI 提示）。

### 2.5 logs.rs 改写

**`get_logs`**
```rust
let inner = state.snapshot();
let entries = inner.message_tracer.read_recent(limit, q.offset, q.dir, q.id)?;
Ok(Json(LogsResponse { entries, total: ..., tracing_enabled: inner.message_tracer.is_enabled() }))
```
去掉 `read_all`/`trace_path`/`read_to_string`。

**`stream_logs`**
```rust
let inner = state.snapshot();
let mut rx = inner.message_tracer.subscribe()?;  // broadcast::Receiver
let stream = async_stream::stream! { ... }  // 或 unfold
// 加初始 backlog：先 read_recent 最近 N 条发一遍，再进 channel
```
去掉 500ms mtime 轮询循环。`broadcast` channel 天然支持"新 subscriber" 补发语义。

> **不引入 async_stream crate**（p5 §3.3 约束）：用 `tokio_stream::wrappers::BroadcastStream`（tokio-stream 已含 broadcast wrapper）或手写 unfold。

### 2.6 懒收敛：PDF 超大文件不再全量读

旧 `read_all` 无论分页都要 `read_to_string` 整文件。新内存源只拷 VM 里 VecDeque 的活跃区，offset+slice，O(limit)。

### 2.7 向后兼容

- 旧配置 `tracing.enabled=true` + `path`：行为=内存+文件双写，网页日志可见性反而**变好**（内存即时可见），无回退。
- 旧配置 `tracing.enabled=false`：无变化。
- `trace_path()` 主调点仅 flags.rs 两处，都改成 `read_recent()` / `subscribe()`。
- 不动 req-write 侧的 16 处 `trace_request/response/error` 调用语义（mod.rs server handler 无感）。

---

## 3. 改动清单

| 文件 | 改动 | 行数估 |
|------|------|--------|
| `src/cli/mod.rs` | `TracingConfig` 加 `max_entries` 字段 + default fn | +8 |
| `src/message_tracing/mod.rs` | 加 RingBuffer + broadcast；重写 new/trace_*/read_recent/subscribe | ~+120 / -50 |
| `src/message_tracing/mod.rs` | `trace_path()` 语义改为"返回文件路径（仅供状态）"，不再 flush | ~改 |
| `src/server/logs.rs` | `get_logs`→`read_recent`；`stream_logs`→`broadcast::subscribe` | ~+30 / -80 |
| `config.example.toml` | 注释加 `max_entries` 说明 | +3 |
| `README.md` | tracing 段落更新（内存为主，文件可选） | +改 |
| `src/server/i18n/{en,zh-CN}.json` | logs.description 文案微调（"来自 trace.jsonl"→"最近请求追踪"） | +2 |

合计约 +160 / -130，净增 ~30 行。

### 风险

| 风险 | 严重度 | 缓解 |
|------|--------|------|
| broadcast 滞后丢条 | 🟡 | capacity=64，rifle receiver 容清 backlog；SSE 偶丢可接受（非消息总线） |
| VecDeque Mutex 与 broadcast 双锁顺序 | 🟢 | 双写时"先 buffer→后 channel"，单方向锁序，无死锁 |
| 内存不降级（重启丢历史） | 🟢 | 预期行为（网页日志勿需持久）；需持久时配 path |
| reload 后旧缓冲丢历史 | 🟡 | 与现状文件 append 不同——改为 reload 时若无新缓冲则旧缓冲丢弃？**决策：reload 创建新 MessageTracer，旧缓冲 drop，历史不可见**；用户改配置需接受这点（与 Helm 重连类似的边界行为）。或：reload 时保留旧 buffer entries 到新 tracer？复杂度高，暂不做 |
| `tokio_stream BroadcastStream` 是否在本 dep 版本可用 | 🟡 | tokio-stream `0.1` 含 `wrappers::BroadcastStream`（0.1+ 有），确认 Cargo.toml 版本 |

---

## 4. 测试计划

| 测试 | 期望 |
|------|------|
| 单元 `read_recent` 分页+过滤 | offset/limit/dir/id 正确；超过缓冲存量返回空 |
| 单元 环形缓冲 max_entries 淘汰 | 写入 N+10 条仅保留最近 N 条 |
| 单元 broadcast subscribe 收新条目 | subscribe 后发布的条目能收到 |
| 集成 /api/logs（内存源） | 请求后无需 flush 即可读到 |
| 集成 /api/logs/stream | 新请求后 SSE 即时收到（无 500ms 延迟） |
| 集成 文件可选不冲突 | path 配置时文件仍正确写 |
| 245 既有测试保持 | 无退化 |

### 4.1 日志语义（已知行为说明，非 bug）

下列两条在 P6 实施后的真机审计中发现，经确认为**设计权衡**而非缺陷：

- **fallback 链每尝试一个 provider mapping 就记一条 `req` trace 条目**：
  `trace_request` 在 `src/server/mod.rs` 的 fallback for 循环内调用（L647
  OpenAI 端点、L923 Anthropic 端点），与 stdout `info!` 的 `[n/N]` 重试
  标记同步——记录每次 fallback 尝试的 actual_model，便于排查"哪个上游
  被尝试、哪个成功/失败"。`/api/logs` 的 `total` 是**trace 条目数**
  而非请求数；前端按 `trace_id` 去重才是请求数。
- **`err` LogEntry 的 model/provider/route_type 为 None**：`trace_error`
  只收 `id` + `error` 两参（`message_tracing/mod.rs` L337）。依赖 `id`
  软关联回同 id 的 `req` 条目可看到完整路由链——`read_recent` 支持
  `?id=<trace_id>` 过滤是此设计的配套。这是"日志条目轻量化"的权衡，
  与 p6 §1.3"`trace.jsonl` 存完整体、内存 LogEntry 只存精简字段"一致。

若未来要在 err 行直接显示 model/provider/route_type，需给 `trace_error`
扩展可选三参；属改进项，非 P6 范围。

---

## 5. 关联：Think 路由文档（用户要求"留文档"）

### 5.1 现状问题

CCM `route()` 硬编码 8 层 fallthrough（mod.rs:180），其中 Think 层（mod.rs:579）单一触发条件：

```rust
fn is_plan_mode(&self, request: &AnthropicRequest) -> bool {
    request.thinking.as_ref().map(|t| t.r#type == "enabled").unwrap_or(false)
}
```

只认 Anthropic Messages API 的 `thinking.type=="enabled"`。经 OpenAI 兼容端点（Hermes ccm 调用走此路径）的请求无 `thinking` 字段 → 永不命中 Think → 路由落到 Default。

### 5.2 CCR 对照

`musistudio/claude-code-router` 已删除 legacy-thinking/background/web-search 硬编码层（`config.ts:85` `REMOVED_LEGACY_ROUTER_RULE_IDS`），改为声明式 condition rules：用户配 `{left:"request.body.thinking.type", operator:"==", right:"enabled"}` 之类任意字段 → 任意模型。CCM 仍在使用已被 CCR 废弃的 legacy 硬编码方案。

### 5.3 建议方向（留档，暂不实施）

参考 p0-routing-strategy.md 的 `RouterRule` 体系（已实现 condition + model-prefix），将 Think/Background/WebSearch/LongContext 从硬编码层迁移为：
- 可由用户用 condition rule 覆盖的声明式规则
- 信号扩展：`request.body.thinking.type`(Anthropic) / `request.body.reasoning_effort`(OpenAI o-series) / 模型名启发式 / `thinking_budget` 等
- 保留硬编码作为默认 fallback，用户配的 rule 优先级更高

**本轮不写代码**，仅留文档。
