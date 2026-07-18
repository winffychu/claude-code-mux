# P3: Provider Capability 路由 — 协议感知的模型/Fallback 自动重写

> 来源：CCR `service.ts:1851-1928`（rewriteProviderHeader L1851, rewriteFallbackForProtocol L1887, rewriteBodyModelForProtocol L1897, rewriteModelSelectorForProtocol L1910）vs CCM `server/mod.rs:610-879`（**注**：cost_first 测试加入后 server/mod.rs 已扩至 1234 行，`handle_messages` 现实际范围 `L795-1072`；本 plan 暂缓未实施，引用为历史审计快照）
> 状态：暂缓（2026-07-10 风险审计：硬依赖 P2 FallbackConfig + handler 已固定 protocol + 实际价值低，风险 > 收益）
> 审计修正：2026-07-03 — 补充 `rewriteBodyModelForProtocol()` 对齐（见底部修正日志）/ 2026-07-09 三轮修正（见底部修正日志 #7-#10）

---

## 对比分析

### CCR 实现（`service.ts:1851-1928`）

```
applyProviderCapabilityRouting(input):
  1. requestProtocolForPath(path) → 推断 client protocol
     - /v1/messages → anthropic_messages
     - /v1/chat/completions → openai_chat_completions
     - /v1/responses → openai_responses
     - /v1/gemini/* → gemini_generate_content

  2. rewriteProviderHeader(headers, "x-target-provider", config, protocol)
     → 根据 protocol 找到支持该协议的 provider，重写 target header

  3. rewriteModelSelectorForProtocol(routedModel, config, protocol)
     → 把客户端请求的 model 名映射为 provider 在该 protocol 下的等价模型

  4. rewriteFallbackForProtocol(fallback, config, protocol)
     → fallback 模型链也按 protocol 重写

  5. rewriteBodyModelForProtocol(body, config, protocol)
     → 修改请求 body 中 model 字段为 protocol-aware 的模型名
```

> **审计补充**：原文档对比表未列出 `rewriteBodyModelForProtocol()` — CCR 不仅重写 model selector 和 fallback，还在 L1825 **重写 request body 中的 model 字段**。CCM 需在 protocol 路由后同步重写 `anthropic_request.model`，否则 upstream provider 收到的 model 名可能与 protocol 不匹配。

**核心价值**：同一个逻辑模型名（如 `claude-sonnet-4`）在不同 provider 下有不同的实际模型名，CCR 自动做映射。

### CCM 现状

CCM 的路由完全基于 TOML 配置的 `models` 映射：
- `models[].name` → 逻辑模型名
- `models[].mappings[].provider` + `models[].mappings[].actual_model` → 实际模型

路由时 `decision.model_name` → 找 `model_config` → 遍历 `mappings` → provider。

**无 protocol 感知**：CCM 不检测请求的 protocol 类型，不根据 protocol 重写模型名或 fallback 链。如果客户端通过 `/v1/chat/completions` 入站，CCM 的 `handle_openai_chat_completions` 做转换但不路由。

### 差距

| 能力 | CCR | CCM |
|------|-----|-----|
| Protocol 自动检测 | ✅ 按 path 推断 4 种 protocol | ❌ 无（隐式 anthropic_messages） |
| 模型名 protocol 重写 | ✅ 同一逻辑模型在不同 protocol 下不同名 | ❌ 靠 TOML model mapping 手动配置 |
| **Body model 字段重写** | ✅ `rewriteBodyModelForProtocol()` L1825 | ❌ |
| Fallback 链 protocol 重写 | ✅ fallback 模型链也按 protocol 映射 | ❌ |
| Provider protocol 声明 | ✅ provider.capabilities 列出支持的 protocol | ❌ |

---

## 方案

### 目标

1. 新增 `ProviderProtocol` 枚举，声明 provider 支持的协议
2. 请求入站时检测 protocol（按 path 推断）
3. 模型名/fallback 链按 protocol 自动重写
4. 保留现有 TOML model mapping 作为底层机制，protocol 重写是上层优化

### 数据结构

```rust
// src/router/capability.rs（新建）

/// Provider 支持的上游协议
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub enum ProviderProtocol {
    /// Anthropic Messages API (/v1/messages)
    AnthropicMessages,
    /// OpenAI Chat Completions API (/v1/chat/completions)
    OpenAiChatCompletions,
    /// OpenAI Responses API (/v1/responses, Codex)
    OpenAiResponses,
    /// Google Gemini API (/v1/gemini/*)
    GeminiGenerateContent,
}

impl ProviderProtocol {
    /// 按请求 path 推断 client protocol
    /// 注意：当前 CCM handler 使用硬编码 protocol（每个 handler 已知自己的 protocol），
    /// 此方法保留供未来动态路由场景使用。
    pub fn from_path(path: &str) -> Option<Self> {
        if path.starts_with("/v1/messages") {
            Some(Self::AnthropicMessages)
        } else if path.starts_with("/v1/chat/completions") {
            Some(Self::OpenAiChatCompletions)
        } else if path.starts_with("/v1/responses") {
            Some(Self::OpenAiResponses)
        } else if path.starts_with("/v1/gemini") {
            Some(Self::GeminiGenerateContent)
        } else {
            None
        }
    }

    /// 检查 provider 是否支持此 protocol
    pub fn supports(&self, provider_protocols: &[ProviderProtocol]) -> bool {
        provider_protocols.contains(self)
    }
}

/// Protocol-aware 模型名重写
/// 同一逻辑模型在不同 protocol 下可能有不同实际名称
///
/// 示例：
///   逻辑模型 "claude-sonnet-4" → AnthropicMessages 下 = "claude-sonnet-4-20250514"
///   逻辑模型 "claude-sonnet-4" → OpenAiChatCompletions 下 = "anthropic/claude-sonnet-4-20250514"
pub fn rewrite_model_for_protocol(
    model: &str,
    protocol: &ProviderProtocol,
    config: &AppConfig,
) -> String {
    // 查找 protocol-specific 模型映射
    // 如果配置中有 [models.{model}.protocols.{protocol}] → model_alias
    // 否则返回原始 model
    config.models
        .iter()
        .find(|m| m.name == model)
        .and_then(|m| m.protocol_aliases.get(protocol))
        .cloned()
        .unwrap_or_else(|| model.to_string())
}

/// Protocol-aware fallback 链重写
pub fn rewrite_fallback_for_protocol(
    fallback: &FallbackConfig,
    protocol: &ProviderProtocol,
    config: &AppConfig,
) -> FallbackConfig {
    let mut rewritten = fallback.clone();
    rewritten.models = fallback.models
        .iter()
        .map(|m| rewrite_model_for_protocol(m, protocol, config))
        .collect();
    rewritten
}

/// Protocol-aware body model 字段重写
/// 修改 AnthropicRequest.model 为 protocol-aware 的模型名
/// 与 rewrite_model_for_protocol 相同逻辑，但直接修改 request body
pub fn rewrite_body_model(
    request: &mut AnthropicRequest,
    protocol: &ProviderProtocol,
    config: &AppConfig,
) {
    let rewritten = rewrite_model_for_protocol(&request.model, protocol, config);
    request.model = rewritten;
}
```

### 配置格式

```toml
# 声明 provider 支持的协议
[[providers]]
name = "anthropic-direct"
provider_type = "anthropic"
api_key = "sk-..."
protocols = ["anthropic_messages"]  # 新增字段

[[providers]]
name = "openrouter"
provider_type = "openrouter"
api_key = "sk-or-..."
protocols = ["openai_chat_completions", "anthropic_messages"]

# 模型 protocol 别名（可选）
[[models]]
name = "claude-sonnet-4"

[[models.mappings]]
provider = "anthropic-direct"
actual_model = "claude-sonnet-4-20250514"

[[models.mappings]]
provider = "openrouter"
actual_model = "anthropic/claude-sonnet-4-20250514"

# 新增：protocol-specific 别名（可选）
[models.protocols.anthropic_messages]
alias = "claude-sonnet-4-20250514"

[models.protocols.openai_chat_completions]
alias = "anthropic/claude-sonnet-4-20250514"
```

### handler 集成

> **注意**：CCM 的 `handle_messages` 签名为 `(State, HeaderMap, Json<serde_json::Value>)`，没有 `Request` 参数。要获取请求路径，有两种方案：
> 1. 新增 `axum::extract::Request` 作为最后一个 extractor（Axum 允许多个 extractor，`Request` 必须放最后）
> 2. 在路由层通过独立的 handler 函数区分 protocol（每个 handler 已知自己的 protocol）
>
> 推荐方案 2：`handle_messages` 固定为 `AnthropicMessages`，`handle_openai_chat_completions` 固定为 `OpenAiChatCompletions`，无需运行时 path 检测。

```rust
// src/server/mod.rs — handle_messages 入口

// 1. 检测 protocol（推荐：按 handler 硬编码，而非运行时 path 检测）
// handle_messages → AnthropicMessages
// handle_openai_chat_completions → OpenAiChatCompletions
let protocol = ProviderProtocol::AnthropicMessages; // handle_messages 中固定

// 2. 如果检测到 protocol，重写模型名和 fallback
let routed_model = rewrite_model_for_protocol(&decision.model_name, &protocol, &inner.config);

let fallback = rewrite_fallback_for_protocol(&fallback_config, &protocol, &inner.config);
```

### Provider 字段

```rust
// src/providers/mod.rs — ProviderConfig 新增

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    // ... 现有字段 ...

    /// 此 provider 支持的上游协议列表
    /// 未配置时按 provider 类型推断默认值：
    /// - anthropic → [AnthropicMessages]
    /// - openai/openrouter → [OpenAiChatCompletions]
    /// - gemini → [GeminiGenerateContent]
    ///
    /// 注意：serde default 无法感知 provider_type，这里使用 anthropic_messages 作为
    /// 安全默认值（最通用）。非 anthropic provider 必须显式配置 protocols 字段，
    /// 否则 protocol 路由将无法正确匹配。实施时建议在 config 加载后做 post-deserialization
    /// 校验：对未配置 protocols 的 provider 按 provider_type 补全默认值。
    #[serde(default = "default_anthropic_protocol")]
    pub protocols: Vec<ProviderProtocol>,
}

fn default_anthropic_protocol() -> Vec<ProviderProtocol> {
    vec![ProviderProtocol::AnthropicMessages]
}
```

---

## 文件改动

| 文件 | 改动类型 | 代码量 |
|------|----------|--------|
| `src/router/capability.rs`（新建） | ProviderProtocol enum + from_path + supports + rewrite_model + rewrite_fallback + rewrite_body_model | ~70 行 |
| `src/providers/mod.rs` | ProviderConfig.protocols 字段 + default | ~10 行 |
| `src/server/mod.rs` | handle_messages 入口 protocol detection + rewrite | ~10 行 |
| **合计** | | **~90 行** |

---

## 与 P2 的关系

P2（故障重试）的 `build_fallback_chain` 产出 fallback 模型链后，P3 的 `rewrite_fallback_for_protocol` 可以对这个链做 protocol-aware 重写。两个模块是正交的：

```
P2: build_fallback_chain(model, fallback) → [(model, idx), ...]
P3: rewrite_model_for_protocol(model, protocol, config) → model'
```

P3 在 P2 之前执行（protocol 重写 → 然后构建 fallback chain）。

## 与 P0 的关系

P0.1 Router Rules 的 `condition.left` 可以引用 `request.protocol`，使条件路由可以按 protocol 分流：

```toml
[[router.rules]]
id = "openai-client-big-model"
name = "OpenAI client → big model"
type = "condition"

[router.rules.condition]
left = "request.protocol"
operator = "=="
right = "openai_chat_completions"
```

> **注意**：`request.protocol` 不是 body 字段，P0 的 `resolve_path_value` 需特殊处理此路径——从 `AnthropicRequest` 的独立元数据字段读取（P3 注入），而非从 `request.body.*` JSON 路径查找。

---

## 验证

```bash
cargo check
cargo test
cargo clippy --no-deps
```

新增单元测试：
- `test_protocol_from_path_messages`：`/v1/messages` → AnthropicMessages
- `test_protocol_from_path_chat_completions`：`/v1/chat/completions` → OpenAiChatCompletions
- `test_protocol_from_path_unknown`：`/unknown` → None
- `test_rewrite_model_with_protocol_alias`：有 alias 时返回 alias
- `test_rewrite_model_without_alias`：无 alias 时返回原始模型名
- `test_rewrite_fallback_chain`：fallback 列表中每个模型都做 protocol 重写
- `test_rewrite_body_model`：验证 request body 中 model 字段被 protocol-aware 重写
- `test_provider_protocol_default`：未配置 protocols 时默认 AnthropicMessages

---

## 审计修正日志（2026-07-03）

| # | 原始内容 | 修正为 | 依据 |
|---|---------|--------|------|
| 1 | 对比表未列出 body model 字段重写 | 补充 `rewriteBodyModelForProtocol()` — CCR L1825（调用点）/ L1897（函数定义）不仅重写 model selector 和 fallback，还重写 request body 中的 model 字段 | CCR `service.ts`：L1825 调用 `rewriteBodyModelForProtocol(body, config, protocol)`，函数定义在 L1897 |
| 2 | 文件改动表 ~80 行 | 更新为 ~90 行（新增 `rewrite_body_model` 函数 ~10 行） | — |
| 3 | 正文无 `rewrite_body_model` 函数代码段 | 在 `rewrite_fallback_for_protocol` 后补充 `rewrite_body_model` 函数代码 | 文件改动表已列出该函数 ~10 行 |
| 4 | "与 P0 的关系"章节未说明 `request.protocol` 路径特殊性 | 补充说明：`request.protocol` 非 body 字段，P0 `resolve_path_value` 需从 `AnthropicRequest` 元数据字段读取（P3 注入） | — |
| 5 | ProviderConfig.protocols 默认值注释不准确 | 修正为按 provider 类型推断默认值：anthropic → [anthropic_messages]，openai → [openai_chat_completions]，gemini → [gemini_generate_content] | — |
| 6 | `rewriteBodyModelForProtocol` 行号标注为 L1825 | 修正为 L1825（调用点）/ L1897（函数定义） | CCR `service.ts` L1825 = 调用点，L1897 = 函数定义 |
| 7 | 三轮自审：CCR 行号引用 `service.ts:1856-1886` | 修正为 `1851-1928`（rewriteProviderHeader L1851 → rewriteModelSelectorForProtocol L1910） | 代码验证：grep 4 个函数确认 |
| 8 | 三轮自审：CCM 行号引用 `server/mod.rs:608-807` | 修正为 `610-879` | 代码验证：handle_messages L610，结束 L879 |
| 9 | 三轮自审：`request.uri().path()` 不可用 | handle_messages 参数是 `Json<serde_json::Value>`，无 Request。推荐按 handler 硬编码 protocol | 代码验证：`grep -n "async fn handle_messages" mod.rs` = L610 |
| 10 | 三轮自审：`#[serde(default = "default_anthropic_protocol")]` 矛盾 | 注释说"按 provider 类型推断"但 default 函数只返回 anthropic_messages。补充说明 serde 无法感知 provider_type，推荐用 `#[serde(skip_serializing_if)]` + builder 模式 | — |

### 拆分说明

P3 文档规模较小（~90 行），逻辑清晰（单一 protocol 检测 + 重写），无高风险操作，不需要进一步拆分。
