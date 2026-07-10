# CCM Headers 透传实施计划

> 分期实施，P1 立即落地，P2/P3 逐步扩展
> 审计修正：2026-07-03 — 澄清 CCR 是 "header rewrite" 不是 "header passthrough" / 2026-07-09 三轮修正（见底部修正日志 #5-#7）

---

## ⚠️ CCR vs CCM 概念区分

| 概念 | CCR | CCM 本计划 |
|------|-----|-----------|
| **Header 操作类型** | **Rewrite（覆写）** — 声明式 `set`/`delete` | **Passthrough（透传）** — 白名单选择性传递 |
| **数据流向** | Router Rules 配置中指定 `set key=value` → 覆盖请求中的 header | 客户端 request headers → 经 `extract_client_forward_headers` → `merge_forward_headers` → 注入 upstream 请求 |
| **CCR `applyRouterRewrite()`** | `plugin:259-285`：支持 `request.header`/`request.headers` 路径的 `set`/`delete` 操作 | CCM 不做 rewrite，做 passthrough |
| **CCR `rewriteProviderHeader()`** | `service.ts:1851-1862`：在 capability routing 中重写 `x-target-provider` 等 header | CCM 无此能力 |
| **`forward_headers` 白名单** | **CCR 没有此能力** — CCR 不做 client→upstream headers 透传 | CCM P1 的核心新能力 |

> **修正说明**：原文档将 CCR 的 header rewrite 描述为"支持 headers 透传"，混淆了 rewrite（覆盖）和 passthrough（传递）两个概念。CCR 的 Router Rules 可以 `set`/`delete` headers（声明式），但不自动透传 client headers 到 upstream。CCM 的 `forward_headers` 白名单透传是 CCR 没有的**新能力**，不是对齐 CCR。

---

## P1: 全透传 + existing_keys 保护（立即实施）

### 目标
客户端 headers（User-Agent、anthropic-beta 等）全量透传到上游，通过 `existing_keys` 机制防止覆盖 provider 内部已设的 auth/UA 等关键 header。

### 新建模块 `src/headers/mod.rs`

```rust
// src/headers/mod.rs
// headers 透传与覆写模块
// 负责从入站请求提取客户端 headers，按规则处理后 merge 到出站请求

use axum::http::HeaderMap;

/// 安全过滤：永远不透传的 headers（协议级/安全级）
const BLOCK_LIST: &[&str] = &[
    "host",           // 反向代理不应透传
    "content-length", // reqwest 自动计算
    "transfer-encoding", // hop-by-hop
    "connection",     // hop-by-hop
    "upgrade",        // hop-by-hop
    "cookie",         // 安全风险
    "set-cookie",     // 安全风险
    "proxy-authorization", // 代理凭据
    "x-provider",     // CCM 内部路由，不应透传
    "x-forwarded-for", // P2 预留：保护真实 IP
    "x-real-ip",      // P2 预留：保护真实 IP
    "via",            // 代理泄露
    "forwarded",      // 代理泄露
];

/// 从入站 HeaderMap 提取可透传的 headers
/// 返回 Vec<(小写key, 值)>，已排除 BLOCK_LIST
/// 注意：与 anthropic_compatible.rs:27 已有的 extract_forward_headers 同名但功能不同
/// 现有函数提取 response headers（ANTHROPIC_FORWARD_HEADERS 白名单），本函数提取 request headers（BLOCK_LIST 黑名单）
/// 建议重命名为 extract_client_forward_headers 以避免歧义
    pub fn extract_client_forward_headers(headers: &HeaderMap) -> Vec<(String, String)> {
    headers.iter()
        .filter_map(|(name, value)| {
            let key = name.as_str().to_lowercase();
            // 跳过黑名单
            if BLOCK_LIST.iter().any(|b| *b == key) {
                return None;
            }
            // 注意：to_str() 对非 ASCII header 值会失败，非 ASCII 值静默丢弃
            // 如需透传非 ASCII header，改用 value.as_bytes() + unsafe 转换
            value.to_str().ok().map(|v| (key, v.to_string()))
        })
        .collect()
}

/// 将 forward_headers merge 到 reqwest RequestBuilder
/// existing_keys: provider 内部已设置的 header key 列表（小写）
/// 同名 header 不覆盖 provider 已设值，防止：
///   - 覆盖 auth (authorization, x-api-key)
///   - 覆盖 OAuth 浏览器 UA（绕 Cloudflare）
///   - 双重 content-type 等
pub fn merge_forward_headers(
    req_builder: reqwest::RequestBuilder,
    forward_headers: &[(String, String)],
    existing_keys: &[&str],
) -> reqwest::RequestBuilder {
    let existing_lower: Vec<String> = existing_keys.iter().map(|k| k.to_lowercase()).collect();
    forward_headers.iter().fold(req_builder, |rb, (key, value)| {
        if existing_lower.contains(&key.to_lowercase()) {
            rb // 跳过：provider 已设置此 header
        } else {
            rb.header(key.as_str(), value.as_str())
        }
    })
}
```

### 改动 `src/models/mod.rs`

AnthropicRequest 增加字段（`#[serde(skip)]` 不影响反序列化/序列化）：

```rust
pub struct AnthropicRequest {
    // ... 现有字段 ...
    #[serde(skip)]
    pub forward_headers: Vec<(String, String)>,  // 客户端透传 headers
}
```

### 改动 `src/lib.rs`

```rust
pub mod headers;  // 新增
```

### 改动 `src/server/mod.rs`

3 个入口 handler 注入 forward_headers：

| handler | 行号 | 改动 |
|---------|------|------|
| `handle_messages` | L610 | 已有 `headers: HeaderMap`，添加 extract 调用 |
| `handle_openai_chat_completions` | L389 | 已有 `headers: HeaderMap`，添加 extract 调用 |
| `handle_count_tokens` | L882 | **补加** `headers: HeaderMap` 参数 + extract 调用。Axum extractor 自动提取，非 API 破坏性变更 |

代码：
```rust
use crate::headers;

// handle_messages / handle_openai_chat_completions 中：
anthropic_request.forward_headers = headers::extract_client_forward_headers(&headers);

// handle_count_tokens 中：
// 1. 函数签名加 headers: HeaderMap
// 2. routing_request.forward_headers = headers::extract_client_forward_headers(&headers);
```

### 改动 `src/providers/anthropic_compatible.rs` — 3 个构建点

| # | 方法 | 行号 | existing_keys |
|---|------|------|---------------|
| 1 | `try_send_message` | ~L358 之后 | `["anthropic-version", "content-type", "x-api-key", "authorization", "anthropic-beta"]` |
| 2 | `try_send_stream_request` | ~L406 之后 | 同上 |
| 3 | `count_tokens` | ~L480 之后 | 同上 |

```rust
// 在 custom_headers 循环之后、.json(body).send() 之前：
let req_builder = crate::headers::merge_forward_headers(
    req_builder,
    &request.forward_headers,
    &["anthropic-version", "content-type", "x-api-key", "authorization", "anthropic-beta"],
);
```

### 改动 `src/providers/openai.rs` — 3 个 `.send()` 调用点（4 个分支）

> **注意**：openai.rs 有 3 个 `.send()` 调用点，但 `send_message` 和 `send_message_stream` 内部各有 Codex/Chat 分支。`merge_forward_headers` 只需在每个 `.send()` 之前调用一次（共 3 处），但 `existing_keys` 需按分支区分。

| # | 方法 | 分支 | `.send()` 行号 | existing_keys |
|---|------|------|------|---------------|
| 4 | `send_message` | Codex (Responses API) | L1284 | `[...same...]` |
| 5 | `send_message` | Chat (Completions API) | L1357 | `[...same...]` |
| 6 | `send_message_stream` | Codex + Chat 共用 | L1498 | `[...same...]` |

```rust
// 在 custom_headers 循环之后、.json(body).send() 之前：
let req_builder = crate::headers::merge_forward_headers(
    req_builder,
    &request.forward_headers,
    &[...existing_keys...],
);
```

### 改动 `src/providers/gemini.rs` — 4 个构建点

| # | 方法 | 分支 | 行号 | existing_keys |
|---|------|------|------|---------------|
| 8 | `send_message` | Code Assist (retry closure) | ~L492 后 | `["content-type", "authorization"]` |
| 9 | `send_message` | API Key / Vertex (retry closure) | ~L577 后 | `["content-type"]` |
| 10 | `send_message_stream` | Code Assist | ~L667 后 | `["content-type", "authorization"]` |
| 11 | `send_message_stream` | Vertex / API Key | ~L728 后 | `["content-type"]` |

**注意：** Gemini 有 2 处 retry closure（`handle_rate_limit_retry`），forward_headers 需要提前 clone 进 closure 捕获列表。

### 测试

```bash
cargo check
cargo test
cargo clippy --no-deps
```

新增单元测试（`src/headers/mod.rs` 底部）：
- `test_extract_client_forward_headers_block_list`：验证 BLOCK_LIST 中 header 被过滤
- `test_merge_forward_headers_skip_existing`：验证 existing_keys 中 header 不被覆写
- `test_merge_forward_headers_pass_new`：验证不在 existing_keys 中的 header 正常透传

### 提交

```
feat: 客户端 headers 全透传到上游 provider

- 新建 src/headers/mod.rs 模块：extract + merge + existing_keys 保护
- AnthropicRequest 增加 forward_headers 字段 (#[serde(skip)])
- 3 个入口 handler 提取客户端 headers 并注入
- 10 个出站构建点 merge 透传 headers，同名被 existing_keys 跳过
- User-Agent 正常透传（非 OAuth 路径）；OAuth 路径已有硬编码 UA 被 existing_keys 保护
- BLOCK_LIST 过滤协议级/安全级 header（host, cookie, x-real-ip 等）
```

---

## P2: Header 覆写规则（drop / set）

### 目标
支持按 header 名称配置 drop（丢弃）或 set（覆写为固定值），替代简单黑名单。

### 数据结构

```rust
// src/headers/mod.rs 扩展
pub enum HeaderAction {
    /// 透传原始值（默认行为）
    Pass,
    /// 丢弃此 header
    Drop,
    /// 覆写为固定值
    Set(String),
}
```

### 配置格式

```toml
# config.toml
[server.header_rewrite]
# 丢弃：保护真实 IP（CF CDN 场景）
"x-real-ip" = "drop"
"x-forwarded-for" = "drop"
"cf-connecting-ip" = "drop"

# 覆写：强制设固定值
"x-custom-trace" = "set:ccm-prod"
```

### 逻辑

- `extract_client_forward_headers` 升级：读取 config 中的 rewrite 规则，对每个 header 执行对应 action
- BLOCK_LIST 变为默认 Drop 规则（硬编码，不可覆盖）
- config 中的 rewrite 规则优先于默认 Pass

---

## P3: 条件覆写（Map）

### 目标
支持按 provider 或其他条件动态覆写 header。

### 数据结构

```rust
// src/headers/mod.rs 扩展
pub enum HeaderAction {
    Pass,
    Drop,
    Set(String),
    /// 条件覆写：按 provider 名称匹配不同规则
    Map {
        default: Box<HeaderAction>,
        rules: Vec<(String, HeaderAction)>,  // (provider_name, action)
    },
}
```

### 配置格式

```toml
# config.toml
[server.header_rewrite]
# User-Agent 按 provider 条件覆写
"user-agent" = """map:default=pass;chatgpt-codex=set:Mozilla/5.0..."""
```

### 与 OAuth UA 冲突的安全性

条件覆写优先级：
```
1. Provider 内部硬编码（OAuth UA 绕 CF）   ← 不可覆盖
2. P3 条件覆写                              ← 可覆盖客户端值
3. P2 固定覆写                              ← 可覆盖客户端值
4. 客户端原始 header                        ← 最低
```

`existing_keys` 仍保护第 1 层不被第 2-4 层覆盖。
P3 只影响第 2-4 层之间的优先级，不会和 OAuth UA 打架。

---

## 安全 P1（独立于透传）：入站认证中间件

### 当前问题

- `handle_messages` / `handle_openai_chat_completions` / `handle_count_tokens`：无认证
- `GET /api/config/json`：**泄露所有 provider api_key 明文**
- `POST /api/config/json`：**可篡改配置/注入 key**
- `POST /api/reload`：可触发重载

`server.api_key` 配置项存在于 `cli/mod.rs` L25 但**没有 handler 检查它**。

### 实施（独立 PR）

1. Axum middleware layer 检查 `x-api-key` / `Authorization: Bearer xxx`
2. 保护 `/api/*` 端点（管理接口必须认证）
3. `/v1/*` 端点可选认证（按 config 开关）
4. `get_config_json` 返回时脱敏 api_key（显示 `sk-***xxx`）

---

## 文件改动汇总

| 文件 | P1 | P2 | P3 |
|------|----|----|-----|
| `src/headers/mod.rs` | 新建 ~50行 | +30行 | +50行 |
| `src/models/mod.rs` | +2行 | — | — |
| `src/lib.rs` | +1行 | — | — |
| `src/server/mod.rs` | +11行 | — | — |
| `src/providers/anthropic_compatible.rs` | +6行（3×2） | — | — |
| `src/providers/openai.rs` | +8行（4×2） | — | — |
| `src/providers/gemini.rs` | +10行（4×2.5，含 clone） | — | — |
| **P1 合计** | **~88行** | | |

### 构建点守卫（防遗漏）

CI / pre-commit 中加 grep 检查：
```bash
test $(grep -c "merge_forward_headers" src/providers/*.rs) -eq 10
```

---

## 审计修正日志（2026-07-03）

| # | 原始内容 | 修正为 | 依据 |
|---|---------|--------|------|
| 1 | 文档标题/描述暗示对齐 CCR "headers 透传" | CCR 是 "header rewrite"（`set`/`delete` 声明式覆写），不是 passthrough。CCM `forward_headers` 白名单透传是 CCR 没有的新能力 | CCR `applyRouterRewrite()` (`plugin:259-285`) 支持 `request.header` 路径 `set`/`delete`。但 CCR 不做 client→upstream headers 自动透传 |
| 2 | `forward_headers` 是与 CCR 对齐 | `forward_headers` 是 CCM 独创能力 | grep `forward_headers\|merge_forward\|extract_forward` 跨 CCR `src/` = 0 匹配 |
| 3 | openai.rs 构建点 ~L1280/L1353/L1494 | 修正为精确 L1284/L1357/L1498（3 个 `.send()` 调用点，非 4 个） | grep `\.send()` 确认；send_message_stream 的 Codex/非Codex 共用同一 `.send()` |
| 4 | `extract_client_forward_headers` 静默丢弃非 ASCII | 补充注释说明 | `to_str().ok()` 对非 ASCII 返回 Err |
| 5 | 三轮自审：openai.rs 构建"4 个构建点" | 实际 3 个 `.send()` 调用点（4 个分支）。send_message_stream 的 Codex/非Codex 共用同一 `.send()` | 代码验证：`grep -c '.send()' openai.rs` = 3 |
| 6 | 三轮自审：总构建点 11 个 | 修正为 10 个（3+4+3），守卫同步修正 | 代码验证：anthropic_compat=3, openai=3, gemini=4 |
| 7 | 三轮自审：`extract_forward_headers` 与 `anthropic_compatible.rs:27` 已有同名函数冲突 | 重命名为 `extract_client_forward_headers`。现有函数提取 response headers（白名单），新函数提取 request headers（黑名单） | 代码验证：`grep -n "fn extract_forward_headers" anthropic_compatible.rs` = L27 |

### 拆分说明

本文档已采用拆分模式（P1/P2/P3 分期 + 安全 P1 独立 PR），结构良好，不再进一步拆分。
