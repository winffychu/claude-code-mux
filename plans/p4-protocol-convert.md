# P4: Protocol 协议转换 — Anthropic ↔ OpenAI ↔ Gemini

> 来源：CCR `service.ts` 中 `@the-next-ai/ai-gateway` 闭源转换层 vs CCM 已有的单向转换
> 状态：P4a 已实施（2026-07-10），含完整 tool 转换（5 项全实现，2026-07-12 真机验证）；唯一缺口 `tool_choice` 不透传（影响极小）；P4a-SSE / P4b 未实施
> 审计修正：2026-07-03 — 3 处修正 + 拆分为 P4a/P4b（见底部修正日志）

---

## ⚠️ 出站 vs 入站：两套独立转换路径

CCM 存在**两套独立转换路径**，必须区分：

### 路径 A — 出站 provider 转换（✅ 已完整实现）

```
Anthropic client → CCM server → AnthropicRequest →
  openai.rs / gemini.rs provider → 转换为 OpenAI/Gemini API 格式 →
  upstream API → OpenAI/Gemini response →
  转换回 Anthropic ProviderResponse/StreamResponse →
  返回 Anthropic client
```

- `openai.rs` L548-766: `transform_request()` 做 Anthropic→OpenAI 请求转换（含 tool_use→tool_calls、tool_result→tool role、thinking→dropped）
- `openai.rs` L767-860: `transform_response()` 做 OpenAI→Anthropic 响应转换
- `openai.rs` L1433-1679: `send_message_stream()` 做 **OpenAI SSE → Anthropic SSE** 转换（`StreamTransformState` 在 L275 定义，`transform_openai_chunk_to_anthropic_sse` 在 L917）
- `gemini.rs` 同理（995 行对应实现）

### 路径 B — 入站协议转换（✅ P4a 已实现，含完整 tool 转换）

```
OpenAI client → POST /v1/chat/completions → CCM server →
  openai_compat.rs: OpenAIRequest → AnthropicRequest (基本已实现) →
  route() → provider.send_message() → AnthropicResponse →
  openai_compat.rs: Anthropic → OpenAI 响应 (基本已实现) →
  返回 OpenAI client
```

- **✅ 已实现** (`openai_compat.rs` L93-214): system 提取、user/assistant message 转换、image data URL 转换
- **✅ 已实现** (`openai_compat.rs` L282-305): tool 定义转换（OpenAI function.format → Anthropic Tool），`tools` 字段已正确传递
- **✅ 已实现** (`openai_compat.rs` L211-237): assistant `tool_calls` → `ToolUse` content blocks 转换（含 input JSON 解析）
- **✅ 已实现** (`openai_compat.rs` L244-262): `role: "tool"` → `ToolResult` in user message 转换
- **✅ 已实现** (`openai_compat.rs` L317-353): 响应方向 — text content 提取 + `ToolUse` content blocks → `tool_calls` 转换 + `finish_reason` 完整映射（含 `tool_use → tool_calls`，L356-365）
- **⚠️ 唯一缺口**: `tool_choice` 字段反序列化（L24）但未透传到 `AnthropicRequest`（该结构无 `tool_choice` 字段）。影响：当客户端设 `tool_choice: "required"` 或指定 function 时，值被丢弃，模型按 `auto`（默认）行为处理。走 `/v1/messages`（Anthropic 协议）不受影响
- **❌ 缺失**: streaming SSE 转换 — handler L424-428 明确拒绝 `stream: true` 请求并返回错误提示
- **❌ 缺失**: Gemini 入站端点 (`server/mod.rs` 无 `/v1beta/models/:model:generateContent` 路由)

> **五轮审计修正（2026-07-12 真机验证）**：原文档（2026-07-10 四轮审计）列出 5 个 tool 相关 "❌ 缺失"，实际代码审查 + 真机测试确认 **5 项全已实现**（tool 定义转换 L282-305、assistant tool_calls L211-237、tool role L244-262、响应 tool_calls L328-353、finish_reason 映射 L356-365）。唯一真实缺口是 `tool_choice` 不透传——影响极小（仅 `tool_choice: "required"` 或指定 function 时生效，默认 `auto` 行为与 Anthropic 相同）。
>
> 真机测试：走 OpenAI 协议 `/v1/chat/completions` 发带 tool 请求，CCM 正确转换并传递 tool 定义到上游。模型选择不调用 tool 是上游模型行为，非 CCM 代码缺陷。

---

## 对比分析

### CCR 协议转换矩阵

CCR 的核心 gateway（`@the-next-ai/ai-gateway` v1.0.2，闭源 npm 包）支持 4 种 protocol 的双向转换。

> **注意**：CCR gateway 以**独立子进程**运行（`service.ts:2636-2642`：`spawn(node, [gatewayEntry])`），通过 HTTP 与 service.ts 交互。实际转换逻辑在闭源包内部，**无法从 CCR 源码验证**转换矩阵完整性。`app.ts:111-115` 定义了 4 种 `GatewayProviderProtocol`，`service.ts:218-223` 定义了 protocol fallback 顺序，但从源码只能确认 protocol 检测和路由逻辑，不能确认具体转换函数。

```
                 anthropic_messages (原生)
                    ↕
    openai_chat_completions ←→ openai_responses
                    ↕
              gemini_generate_content
```

| 入站 protocol | 出站 protocol | 转换方向 | 状态 |
|---------------|---------------|----------|------|
| anthropic_messages | anthropic_messages | 透传 | ✅ |
| anthropic_messages | openai_chat_completions | 正向 | ✅ |
| anthropic_messages | openai_responses | 正向 | ✅ |
| anthropic_messages | gemini_generate_content | 正向 | ✅ |
| openai_chat_completions | anthropic_messages | 逆向 | ✅ |
| openai_chat_completions | openai_responses | 横向 | ✅ |
| openai_chat_completions | gemini_generate_content | 横向 | ✅ |
| openai_responses | anthropic_messages | 逆向 | ✅ |
| gemini_generate_content | anthropic_messages | 逆向 | ✅ |

### CCM 协议转换矩阵

| 入站 protocol | 出站 protocol | 转换方向 | 状态 | 文件 | 备注 |
|---------------|---------------|----------|------|------|------|
| anthropic_messages | anthropic_messages | 透传 | ✅ | `anthropic_compatible.rs` | |
| anthropic_messages | openai_chat_completions | 正向（出站） | ✅ | `openai.rs` (1825 行) | 含 tools+SSE |
| anthropic_messages | gemini_generate_content | 正向（出站） | ✅ | `gemini.rs` (995 行) | 含 SSE |
| openai_chat_completions | anthropic_messages | 逆向（入站） | ✅ P4a 已实现 | `openai_compat.rs` L93-389 | 请求/响应 + 5 项 tool 转换全已实现（2026-07-12 真机验证）；唯一缺口 `tool_choice` 不透传；缺 SSE streaming |
| openai_chat_completions | openai_responses | 横向 | ❌ | — | |
| openai_chat_completions | gemini_generate_content | 横向 | ❌ | — | |
| openai_responses | anthropic_messages | 逆向（入站） | ❌ | — | |
| gemini_generate_content | anthropic_messages | 逆向（入站） | ❌ | — | server 无 Gemini 入站端点 |

> **审计修正历史**：原文档将 `openai_chat_completions → anthropic_messages` 标为 ❌，2026-07-03 改为 ⚠️ 部分（确认已有基本请求/响应转换），2026-07-12 改为 ✅（五轮审计 + 真机测试确认 5 项 tool 转换全已实现）。

### 差距

CCM 已覆盖：
- **出站转换（路径 A）**：Anthropic client → 任意 provider，含 tools+SSE，✅ 完整
- **入站转换（路径 B）**：OpenAI client → Anthropic 路由，基本转换 ✅，但缺 tools+SSE

P4 真正缺口：
1. **入站 tool 定义转换** — `openai_compat.rs` L212 `tools: None` 需改为实际转换
2. **入站 tool_calls 历史转换** — `OpenAIMessage` 缺 `tool_calls` 字段，assistant 历史中的工具调用丢失
3. **入站 tool role 消息转换** — `OpenAIMessage` 缺 `tool` role 处理，tool_result 消息丢失
4. **入站响应 tool_calls 转换** — `OpenAIResponseMessage` 缺 `tool_calls` 字段，响应中 tool_use 无法返回
5. **入站响应 finish_reason 映射** — 缺 `tool_use → tool_calls` 映射
6. **入站 SSE streaming 转换** — handler 当前不做 SSE 事件转换，需新建 `anthropic_sse_to_openai_sse()`
7. **入站 Gemini 端点** — server 无 Gemini 路由（需求极低，暂不实施）

---

## 需求评估

### 何时需要逆向/横向转换？

| 场景 | 需要 | 优先级 |
|------|------|--------|
| Hermes（Anthropic client）→ 各 provider | 正向（已有） | — |
| Codex CLI（OpenAI Responses）→ 各 provider | openai_responses → anthropic 逆向 | 中 |
| 其他 OpenAI 兼容 client → 各 provider | openai_chat_completions → anthropic 逆向 | 中 |
| Gemini client → 各 provider | gemini → anthropic 逆向 | 低 |

### CCR 多了什么

CCR 的核心价值不在于转换逻辑本身（CCM 的 `openai.rs` 1825 行已实现 Anthropic→OpenAI 正向），而在于：

1. **入站协议无关性**：任何 protocol 的 client 请求都可以路由到任何 protocol 的 provider
2. **SSE 流式双向转换**：不只是 JSON 格式转换，还包括 SSE 事件的格式适配
3. **Tool call 格式互转**：Anthropic tool_use ↔ OpenAI function_call ↔ Gemini functionCall

---

## 方案

### 目标

分两步实施：
- **P4a（降级版）**：增强 `openai_compat.rs` — 补全 5 项入站工具相关转换（tool 定义 + tool_calls 历史 + tool role + 响应 tool_calls + finish_reason 映射），**不做 SSE streaming**
- **P4a-SSE（后续单独评估）**：入站 SSE streaming 转换 — 状态机复杂度高，单独审计后决定
- **P4b**：OpenAI Responses → Anthropic 逆向转换（Codex 场景，新建端点）

Gemini 逆向暂不实施（需求极低）。

> **审计修正**：原文档 P4.1 方案为 "新建 `openai_to_anthropic.rs`"，实际 `openai_compat.rs` 已实现基本请求/响应转换。P4a 改为**增强现有模块**，仅补全 tools 转换和 SSE streaming，而非从零新建。
>
> **四轮审计修正（2026-07-10）**：P4a 拆分为 P4a（降级版，工具转换）和 P4a-SSE（streaming）。降级版只做同步转换，不涉及 SSE 状态机，风险可控。

### P4a: 增强 `openai_compat.rs`（工具转换，不含 SSE）

#### 核心流程

```
Client (POST /v1/chat/completions)
  ↓ OpenAIRequest (role/content/tool_calls/functions)
  ↓
转换层: OpenAI → Anthropic
  - messages: [{role, content}] → [{role, content}] (role 映射)
  - tool_calls: [{function: {name, arguments}}] → [{type: tool_use, name, input}]
  - system message: 顶层 system → Anthropic system field
  - stream: bool → bool (SSE 事件格式转换)
  ↓
AnthropicRequest → route() → provider.send_message()
  ↓
ProviderResponse (Anthropic format)
  ↓
转换层: Anthropic → OpenAI
  - content blocks → choices[].message.content
  - tool_use blocks → tool_calls[]
  - usage → usage (token 计数格式适配)
  - stop_reason → finish_reason 映射
  ↓
Client receives OpenAIResponse
```

#### 数据结构

> **审计修正**：以下代码原标注 `src/providers/openai_to_anthropic.rs（新建）`，实际 `openai_compat.rs` L93-266 已实现 `transform_openai_to_anthropic()` (L94) 和 `transform_anthropic_to_openai()` (L217)。P4a 是**在现有函数基础上扩展**，不新建同名函数，仅补全 tools 转换和新增 SSE 函数，而非新建文件。

```rust
// src/server/openai_compat.rs — 现有函数扩展

/// OpenAI Chat Completions 请求 → Anthropic Messages 请求
/// 现有: transform_openai_to_anthropic() L94-214 (已实现基本转换)
/// 扩展: 补全 tools 转换 (当前 L212 为 tools: None)
pub fn transform_openai_to_anthropic(
    openai: &OpenAIRequest,
) -> Result<AnthropicRequest, ProviderError> {
    let model = openai.model.clone();
    let max_tokens = openai.max_tokens.unwrap_or(8192);

    // System message 提取
    let system = openai.messages.iter()
        .find(|m| m.role == "system")
        .map(|m| SystemPrompt::Text(m.content.clone()));

    // 非系统消息转换
    let messages = openai.messages.iter()
        .filter(|m| m.role != "system")
        .map(|m| convert_openai_message(m))
        .collect::<Result<Vec<_>, _>>()?;

    // Tools 转换
    let tools = if let Some(ref tools) = openai.tools {
        Some(tools.iter()
            .map(|t| convert_openai_tool(t))
            .collect::<Result<Vec<_>, _>>()?)
    } else {
        None
    };

    Ok(AnthropicRequest {
        model,
        messages,
        max_tokens,
        system,
        tools,
        temperature: openai.temperature,
        top_p: openai.top_p,
        stop_sequences: openai.stop.clone(),
        stream: Some(openai.stream.unwrap_or(false)),
        thinking: None, // OpenAI 无 thinking
        ..Default::default()
    })
}

/// Anthropic ProviderResponse → OpenAI Chat Completions 响应
pub fn transform_anthropic_to_openai(
    anthropic: ProviderResponse,
    original_model: &str,
) -> OpenAIResponse {
    let content = anthropic.content.iter()
        .filter_map(|block| match block {
            ContentBlock::Known(KnownContentBlock::Text { text, .. }) => Some(text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");

    let tool_calls = anthropic.content.iter()
        .filter_map(|block| match block {
            ContentBlock::Known(KnownContentBlock::ToolUse { id, name, input, .. }) => {
                Some(OpenAIToolCall {
                    id: id.clone(),
                    r#type: "function".to_string(),
                    function: OpenAIToolCallFunction {
                        name: name.clone(),
                        arguments: serde_json::to_string(input).unwrap_or_default(),
                    },
                })
            }
            _ => None,
        })
        .collect::<Vec<_>>();

    let finish_reason = match anthropic.stop_reason.as_deref() {
        Some("end_turn") | Some("stop_sequence") => "stop",
        Some("tool_use") => "tool_calls",
        Some("max_tokens") => "length",
        _ => "stop",
    };

    OpenAIResponse {
        id: anthropic.id,
        object: "chat.completion".to_string(),
        model: original_model.to_string(),
        choices: vec![OpenAIChoice {
            index: 0,
            message: OpenAIMessage {
                role: "assistant".to_string(),
                content,
                tool_calls: if tool_calls.is_empty() { None } else { Some(tool_calls) },
            },
            finish_reason: finish_reason.to_string(),
        }],
        usage: OpenAIUsage {
            prompt_tokens: anthropic.usage.input_tokens,
            completion_tokens: anthropic.usage.output_tokens,
            total_tokens: anthropic.usage.input_tokens + anthropic.usage.output_tokens,
        },
    }
}
```

#### SSE 流式转换（入站方向）

> **方向澄清**：此函数做的是 **入站响应方向**（Anthropic SSE → OpenAI SSE），返回给 OpenAI client。
> CCM **已有**出站方向的 `openai.rs` L1433-1679 `send_message_stream()` 做 OpenAI SSE → Anthropic SSE（provider 返回值→CCM 内部）。
> 两个方向是不同路径，P4a 仅补入站方向。

```rust
/// Anthropic SSE 事件 → OpenAI SSE 事件
///
/// Anthropic 事件类型:
///   message_start, content_block_start, content_block_delta,
///   content_block_stop, message_delta, message_stop
///
/// OpenAI 事件类型:
///   chat.completion.chunk (role + delta content + finish_reason)
///
/// 注意：openai_compat.rs 当前不处理 SSE streaming——stream 字段被透传至
/// AnthropicRequest 但 handler 不做 SSE 事件转换，streaming 请求会因 Anthropic
/// 端返回 SSE 而 OpenAI client 无法解析。此函数为 P4a 新增。
///
/// 类型来源说明：
/// - `SseEvent`：`src/providers/streaming.rs:10`（pub struct，可直接引用）
/// - `OpenAIStreamChunk` / `OpenAIStreamChoice` / `OpenAIStreamDelta`：`src/providers/openai.rs:209` 中的私有结构体。
///   方案 A：将这些结构体移至 `src/providers/streaming.rs` 或新建 `src/providers/openai_types.rs` 并 pub
///   方案 B：在 `openai_compat.rs` 中重新定义同名 pub 结构体（重复但解耦）
///   推荐：方案 A（移至公共模块），避免重复定义
/// - `StreamTransformState`：`src/providers/openai.rs:275` 中的私有结构体。
///   P4a 入站转换需要独立的状态结构体（与出站状态不同），建议新建 `InboundStreamState`
pub fn anthropic_sse_to_openai_sse(
    event: SseEvent,
    state: &mut InboundStreamState,
) -> Vec<SseEvent> {
    match event.event.as_deref() {
        Some("message_start") => {
            // → 发送 role: assistant 的首个 chunk
            vec![SseEvent {
                event: None,
                data: serde_json::to_string(&OpenAIStreamChunk {
                    object: "chat.completion.chunk",
                    choices: vec![OpenAIStreamChoice {
                        index: 0,
                        delta: OpenAIStreamDelta {
                            role: Some("assistant"),
                            content: None,
                        },
                        finish_reason: None,
                    }],
                }).unwrap(),
            }]
        }
        Some("content_block_delta") => {
            // 解析 delta JSON → 提取 text → OpenAI content delta
            if let Ok(json) = serde_json::from_str::<Value>(&event.data) {
                if let Some(delta) = json.get("delta") {
                    if let Some(text) = delta.get("text").and_then(|t| t.as_str()) {
                        return vec![SseEvent {
                            event: None,
                            data: serde_json::to_string(&OpenAIStreamChunk {
                                object: "chat.completion.chunk",
                                choices: vec![OpenAIStreamChoice {
                                    index: 0,
                                    delta: OpenAIStreamDelta {
                                        role: None,
                                        content: Some(text),
                                    },
                                    finish_reason: None,
                                }],
                            }).unwrap(),
                        }];
                    }
                    // tool_use input delta → OpenAI tool_call arguments delta
                    if let Some(partial_json) = delta.get("partial_json") {
                        // 转为 tool_calls delta
                    }
                }
            }
            vec![]
        }
        Some("message_delta") => {
            // 提取 stop_reason → finish_reason
            if let Ok(json) = serde_json::from_str::<Value>(&event.data) {
                if let Some(stop_reason) = json.get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(|s| s.as_str())
                {
                    let finish_reason = match stop_reason {
                        "end_turn" | "stop_sequence" => "stop",
                        "tool_use" => "tool_calls",
                        "max_tokens" => "length",
                        _ => "stop",
                    };
                    return vec![SseEvent {
                        event: None,
                        data: serde_json::to_string(&OpenAIStreamChunk {
                            object: "chat.completion.chunk",
                            choices: vec![OpenAIStreamChoice {
                                index: 0,
                                delta: OpenAIStreamDelta {
                                    role: None,
                                    content: None,
                                },
                                finish_reason: Some(finish_reason),
                            }],
                        }).unwrap(),
                    }];
                }
            }
            vec![]
        }
        Some("message_stop") => {
            // → 发送 [DONE]
            vec![SseEvent {
                event: None,
                data: "[DONE]".to_string(),
            }]
        }
        _ => vec![],
    }
}
```

#### handler 集成

```rust
// src/server/openai_compat.rs — 现有函数扩展

// 修改 src/server/mod.rs 中的 handle_openai_chat_completions（L389），
// 调用 src/server/openai_compat.rs 中的转换函数

// 现有：handle_openai_chat_completions (mod.rs:400-405) 显式拒绝 streaming 请求
// 改为：支持 streaming + 路由

async fn handle_openai_chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(openai_request): Json<OpenAIRequest>,
) -> Result<Response, AppError> {
    let original_model = openai_request.model.clone();
    let is_streaming = openai_request.stream.unwrap_or(false);

    // 1. OpenAI → Anthropic 转换
    let mut anthropic_request = transform_openai_to_anthropic(&openai_request)?;

    // 2. 路由（复用现有 route()）
    let inner = state.snapshot();
    let decision = inner.router.route(&mut anthropic_request)?;

    // 3. 找 model config + provider（复用现有逻辑）
    let model_config = inner.config.models.iter()
        .find(|m| m.name.eq_ignore_ascii_case(&decision.model_name));

    // provider 获取逻辑与 handle_messages 中相同（从 sorted_mappings 找 mapping → provider_registry.get_provider）

    // 4. 发送请求
    if is_streaming {
        // 流式：provider 返回 Anthropic SSE → 转换为 OpenAI SSE
        let stream_response = provider.send_message_stream(anthropic_request).await?;
        let openai_stream = stream_response.stream
            .map_ok(|sse_event| {
                let mut state = InboundStreamState::default();
                anthropic_sse_to_openai_sse(sse_event, &mut state)
            })
            .flatten(); // 一个 Anthropic 事件可能产生 0-N 个 OpenAI 事件
        // 返回 SSE response
    } else {
        // 非流式：provider 返回 Anthropic response → 转换为 OpenAI response
        let anthropic_response = provider.send_message(anthropic_request).await?;
        let openai_response = transform_anthropic_to_openai(anthropic_response, &original_model);
        Ok(Json(openai_response).into_response())
    }
}
```

### P4b: OpenAI Responses → Anthropic 逆向转换

Codex CLI 使用 `/v1/responses` 端点，格式与 Chat Completions 略有不同（更接近 function calling 的原生格式）。

**与 P4a 的区别**：
- 请求格式：`input` 数组而非 `messages` 数组
- Tool 格式：`custom` type + `name` 而非 `function` + `parameters`
- SSE 事件格式：`response.created` / `response.output_text.delta` 等

代码量额外 ~100 行（复用 P4a 的 Anthropic→X 转换逻辑，仅入站解析不同）。

---

## 文件改动

### P4a（降级版，不含 SSE）

| 文件 | 改动类型 | 代码量 |
|------|----------|--------|
| `src/server/openai_compat.rs` | ① `OpenAIMessage` 加 `tool_calls` 字段 + `tool` role 处理 (~35 行)<br>② `OpenAIResponseMessage` 加 `tool_calls` 字段 (~5 行)<br>③ `transform_openai_to_anthropic()` 补全 tool 定义转换 (L212 `None`→实现) + tool_calls 历史 → tool_use + tool role → tool_result (~30 行)<br>④ `transform_anthropic_to_openai()` 补全 tool_use → tool_calls 转换 + finish_reason 补 `tool_use→tool_calls` (~22 行) | ~92 行逻辑 |
| 测试 | 7 个单元测试 | ~80 行 |
| **合计** | | **~172 行** |

> **四轮审计修正**：原 P4a 含 SSE 估 ~210 行。降级版去掉 SSE（`anthropic_sse_to_openai_sse` + `InboundStreamState` + `OpenAIStreamChunk` pub 化），补全 4 个计划遗漏的缺口，净增 ~172 行。

### P4a-SSE（后续单独评估）

| 文件 | 改动类型 | 代码量 |
|------|----------|--------|
| `src/server/openai_compat.rs` | 新增 `anthropic_sse_to_openai_sse()` + `InboundStreamState` | ~120 行 |
| `src/providers/openai.rs` | `OpenAIStreamChunk`/`OpenAIStreamChoice`/`OpenAIStreamDelta` pub 化 | ~10 行 |
| `src/server/mod.rs` | `handle_openai_chat_completions` 支持 streaming + 路由 | ~30 行 |
| **合计** | | **~160 行** |

### P4b

| 文件 | 改动类型 | 代码量 |
|------|----------|--------|
| `src/server/openai_responses.rs`（新建） | Responses 格式解析 + 转换 + `handle_openai_responses` handler | ~100 行 |
| `src/server/mod.rs` | 注册 `/v1/responses` 路由 | ~3 行 |
| **合计** | | **~110 行** |

### 总计

| 子项 | 代码量 | 工时 |
|------|--------|------|
| P4a 降级版 (工具转换，不含 SSE) | ~172 行 | 1.5h |
| P4a-SSE (streaming，后续评估) | ~160 行 | 2h |
| P4b OpenAI Responses 逆向 | ~110 行 | 1.5h |
| **合计（降级版 + P4b）** | **~282 行** | **3h** |
| **合计（全部）** | **~442 行** | **5h** |

---

## 优化建议

先实施 P4a 降级版（增强 `openai_compat.rs`，补全 5 项工具转换），覆盖 OpenAI client 非流式工具调用场景。SSE streaming 单独评估。P4b 等 Codex 需求确认后再做。

---

## 与其他 P 的关系

- **P3（Capability 路由）**：P3 检测入站 protocol 后做模型名重写，P4 在 P3 之后执行（先确定 protocol → 转换请求格式 → 路由）
- **P2（故障重试）**：P4 转换后的 AnthropicRequest 走现有 retry/fallback 链，无需改动
- **P1（Headers 透传）**：P4 转换后的请求仍需透传 headers，与 P1 正交

---

## 验证

```bash
cargo check
cargo test
cargo clippy --no-deps
```

新增单元测试：
- `test_transform_openai_to_anthropic_simple`：单轮对话转换
- `test_transform_openai_to_anthropic_with_system`：system message 提取
- `test_transform_openai_to_anthropic_with_tools`：tool 定义转换
- `test_transform_openai_to_anthropic_with_tool_calls`：tool_calls → tool_use 转换
- `test_transform_anthropic_to_openai_response`：响应逆转换
- `test_transform_anthropic_to_openai_tool_use`：tool_use → tool_calls 转换
- `test_stop_reason_mapping`：stop_reason → finish_reason 映射
- `test_sse_message_start_to_openai`：SSE message_start → role chunk
- `test_sse_content_block_delta_to_openai`：SSE delta → content delta
- `test_sse_message_stop_to_done`：SSE message_stop → [DONE]

集成测试：
- `test_openai_streaming_end_to_end`：OpenAI streaming 请求 → Anthropic provider → OpenAI SSE 返回

---

## 审计修正日志（2026-07-03）

| # | 原始内容 | 修正为 | 依据 |
|---|---------|--------|------|
| 1 | CCM 矩阵表 `openai_chat_completions → anthropic_messages` 标为 ❌ | 改为 ⚠️ 部分 — `openai_compat.rs` L93-214 已实现请求转换，L217-266 已实现响应转换 | 代码验证：`transform_openai_to_anthropic()` (L94) + `transform_anthropic_to_openai()` (L217) |
| 2 | P4.1 方案 "新建 `openai_to_anthropic.rs`" | 改为 P4a "增强现有 `openai_compat.rs`" — 仅补全 tools 转换和 SSE streaming | `openai_compat.rs` 已有基本转换；`openai.rs` 1825 行含完整出站转换（含 tools+SSE），证明 CCM 有转换能力 |
| 3 | 缺少出站/入站方向区分 | 新增"出站 vs 入站"章节，明确两套独立转换路径 | 出站 `openai.rs` (provider→client) vs 入站 `openai_compat.rs` (client→provider) |
| 4 | "CCM 缺少 SSE 流式转换" | 精确为：**出站** SSE ✅ (`openai.rs` L1433-1679)；**入站** SSE ❌（`openai_compat.rs` handler 不做 SSE 事件转换） | `send_message_stream()` 确认 OpenAI SSE→Anthropic SSE 已实现 |
| 5 | "CCM 缺少 tool_calls 转换" | 精确为：**出站** tool 转换 ✅ (`openai.rs` L540-541)；**入站** tool 转换 ❌ (`openai_compat.rs` L212 `tools: None`) | openai.rs 测试 L1799-1807 确认 outbound tool 转换已实现 |
| 6 | P4.1/P4.2 命名 | 改为 P4a/P4b（与其他文档拆分命名一致） | — |
| 7 | 原文档将 "Streaming is not supported" 文本归到 `openai_compat.rs` | 修正为：该文本在 `src/server/mod.rs:400-403`，不在 `openai_compat.rs`（仅 267 行）。`openai_compat.rs` 中 stream 字段仅出现在 L20（字段定义）和 L209（赋值）。handler (`mod.rs:L400-403`) 显式拒绝 streaming 请求 | 代码验证：`grep -n "Streaming is not supported" src/server/mod.rs` = L400, L403 |
| 8 | `openai.rs` 行号：transform_request L531-686、transform_response L306+、send_message_stream L1433-1573 | 修正为：transform_request L548-766、transform_response L767-860、send_message_stream L1433-1679（StreamTransformState 在 L275，transform_openai_chunk_to_anthropic_sse 在 L917） | 代码验证：`openai.rs` 实际行号 |
| 9 | P4a handler 代码段标注 "src/server/openai_compat.rs — 现有 handle_openai_chat_completions 修改" | 修正为：handle_openai_chat_completions 在 `src/server/mod.rs` L389，调用 `openai_compat.rs` 中的转换函数 | 代码验证：`handle_openai_chat_completions` 在 `server/mod.rs` L389 |
| 10 | P4a 新建 `openai_to_anthropic_request` 和 `anthropic_to_openai_response` 函数 | 修正为：扩展现有 `transform_openai_to_anthropic()` (L94) 和 `transform_anthropic_to_openai()` (L217)，不新建同名函数 | 代码验证：`openai_compat.rs` 已有这两个函数 |
| 11 | P4b 文件路径 `src/providers/openai_responses.rs`（新建） | 修正为 `src/server/openai_responses.rs`（新建）——入站转换属于 server 层 | 架构分层：入站转换归属 server 层 |
| 12 | handler 代码段使用 `provider` 变量但未定义 | 补充注释：provider 获取逻辑与 handle_messages 中相同（从 sorted_mappings 找 mapping → provider_registry.get_provider） | — |
| 13 | 三轮自审：`StreamState` 类型未定义 | 实际 CCM 中出站状态叫 `StreamTransformState` (openai.rs:275)，入站需要新建 `InboundStreamState`。修正所有引用为 `InboundStreamState` | 代码验证：grep `struct StreamState` = 0 匹配；`struct StreamTransformState` = openai.rs:275 |
| 14 | 三轮自审：`OpenAIStreamChunk` 等结构体在 openai.rs 中是私有的 | openai.rs:209 的 `OpenAIStreamChunk`/`OpenAIStreamChoice`/`OpenAIStreamDelta` 需移至 pub 模块或 pub 化，否则 openai_compat.rs 无法引用。新增文件改动表条目 | 代码验证：grep `struct OpenAIStreamChunk` = openai.rs:209（无 pub） |
| 15 | 三轮自审：P4 原始审计日志 #7 措辞歧义 | 修正：文本 "Streaming is not supported" 确实存在于 mod.rs:400-403，只是不在 openai_compat.rs 中 | 代码验证：`grep -n "Streaming is not supported" src/server/mod.rs` = L400, L403 |

### 拆分说明

P4 是 5 份文档中最复杂的（涉及双向转换、出站/入站路径、SSE 状态机、tools 格式互转），拆分为：

- **P4a（增强 `openai_compat.rs`）**：在现有代码上扩展，风险可控。改 1 个文件，补全 tools 转换 + 新增 SSE 函数。需理解现有 `transform_openai_to_anthropic()` (L94) 和 `transform_anthropic_to_openai()` (L217) 的完整逻辑，遗漏会导致 OpenAI client 收到不完整响应。
- **P4b（OpenAI Responses 端点）**：新建 handler + 转换逻辑，独立于 P4a。可等 Codex 需求确认后再做。

推荐 P4a 先实施（200 行，影响面在 1 个文件内），P4b 视需求再做。
