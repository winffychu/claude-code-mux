# P0: 路由策略增强 — CCR 强项移植

> 来源：`musistudio/claude-code-router` (CCR) vs `elidickinson/claude-code-mux` (CCM) 代码级对比
> 状态：P0.1 Router Rules + P0.2 Token 阈值路由 已实施（2026-07-10，真机验证 route=default/long-context 触发正确）；P0.3 Subagent Tool Description 注入 未实施（基准审计 2026-07-11 发现：`src/router/agent.rs` 不存在 / git 0 commit / 0 测试；现有 `extract_subagent_model` mod.rs:902 仅做 `system prompt` tag 提取，非 `tool description` 注入增量）
> 预估：~260 行，6h
> 审计修正：2026-07-03 — 4 处断言修正（见底部"审计修正日志"）

---

## 对比分析

### ⚠️ 架构差异提醒

CCR 和 CCM 的路由架构有本质不同，对比时需注意：

| 维度 | CCR | CCM |
|------|-----|-----|
| 路由结构 | **声明式**：`router.rules` 数组按序遍历 + 顶层 `default`/`longContext`/`think`/`webSearch`/`background` 字段 | **程序化**：`route()` 函数固定 6 层 fallthrough 链 |
| 规则匹配 | 字符串比较运算符（`==`/`!=`/`contains`/`contains-deep`/`not-contains`/`starts-with`/`>`/`>=`/`<`/`<=`） | Rust `Regex` + `$1` 捕获组替换 |
| 关键差异 | **无 regex 运算符** — 条件路由仅支持字符串比较/包含/前缀匹配 | **有 regex + `$1`** — 这是 CCM 独有能力，CCR 没有 |
| `longContext` 字段 | `app.ts:528-529` 有类型定义，`default-config.ts:41` 有默认值(200000)，但 **server/plugin 路由逻辑中未使用** — 仅 config 加载/UI 渲染中出现 | 完全缺失 |

> **修正说明**：原文档将 CCR 描述为"支持 regex + `$1` capture"，实际 CCR `RouterRuleOperator`（`app.ts:466-476`）共 10 个运算符，无 regex。`routerRuleConditionMatches()` (`plugin:408-450`) 实现的是字符串比较/包含/前缀匹配。regex 是 CCM `prompt_rules` 的独有能力。

### CCR 路由策略比 CCM 强的 6 个点

| # | 能力 | CCR 实现 | CCM 现状 | 差距 |
|---|------|----------|----------|------|
| 1 | Router Rules 体系 | `condition`（10 个 operator，详见下方）+ `model-prefix` + 任意 `request.body.*` / `request.header.*` rewrite | `prompt_rules` — regex → model + `strip_match` + `$ref` | CCR 支持任意 body 路径 rewrite + header rewrite + condition 表达式 + model-prefix 匹配。**注意**：CCR 不支持 regex，这是 CCM 独有能力 |
| 2 | Token 阈值路由 | 路由决策前 `calculateTokenCount(messages, system, tools)`（plugin:51），可用 `request.tokenCount` 条件路由 | 无 token 计数在路由层 | CCR 通过 `request.tokenCount > N` 的 condition rule 达成阈值路由。**注意**：`longContext`/`longContextThreshold` 字段虽有定义但不被路由逻辑读取 |
| 3 | Subagent tool description 注入 | **当前 CCR 代码中不存在 `injectClaudeCodeAgentToolDescription` 函数** — 原文档称 `plugin:301-306` 有此函数，实际 grep 确认零匹配。仅 `request-log-store.ts` 有 `<CCR-SUBAGENT-MODEL>` 的日志正则 | `<CCM-SUBAGENT-MODEL>` tag（仅 system prompt 检测） | 原文档对 CCR 的引用有误，P0.3 需重新设计为 CCM 原创功能 |
| 4 | 多 Agent 自动识别 | User-Agent 检测 → `claude-code` / `codex` → 不同默认模型 | 无 agent 分类 | CCR 按 agent 类型自动选模型 |
| 5 | Fallback mode 选择 | `off` / `retry` / `model-chain` 三种 mode，per-rule 可独立配置 | 1:N model mapping 按 priority 遍历 | CCR 灵活的 fallback 策略（已拆分到 P2） |
| 6 | Custom Router 模块 | `CUSTOM_ROUTER_PATH` — 加载 `.js/.mjs` 文件，导出 `(request, config) → model` | 无 | CCR 支持外部可编程路由（Rust 不适用，不移植） |

### 不移植的项

| # | 能力 | 原因 |
|---|------|------|
| 4 | 多 Agent 自动识别 | CCM 通过 Hermes 连接，只有一个 client，不需要 User-Agent 分流 |
| 6 | Custom Router 模块 | Rust 无法加载 JS/TS 模块；如需可编程路由，TOML condition rules 已覆盖大部分场景 |

---

## P0.1: Router Rules 体系升级

### 目标

将 CCM 的 `prompt_rules`（regex → model）升级为 CCR 级的 `RouterRule` 体系，支持：
- `condition` 类型：10 个 operator（`==` / `!=` / `contains` / `contains-deep` / `not-contains` / `starts-with` / `>` / `>=` / `<` / `<=`）
  - `contains-deep`：深度递归搜索 JSON 节点（如 `request.body.messages` → 遍历所有 message 的 content）
- `model-prefix` 类型：请求 model 名前缀匹配
- `rewrite` 操作：`set` / `delete` / `array-append` / `array-prepend` / `array-remove` / `array-replace`
- rewrite 路径支持 `request.body.model` / `request.body.*` / `request.header.*`
- per-rule 独立 `fallback` 配置

### 数据结构

```rust
// src/router/mod.rs 新增

#[derive(Debug, Clone, Deserialize)]
pub enum RouterRuleType {
    /// 条件匹配：left operator right
    Condition,
    /// 模型名前缀匹配
    ModelPrefix,
}

#[derive(Debug, Clone, Deserialize)]
pub enum RuleOperator {
    Eq,           // ==
    Ne,           // !=
    Contains,
    ContainsDeep, // 深度递归搜索 JSON 节点
    NotContains,
    StartsWith,
    Gt,           // >
    Ge,           // >=
    Lt,           // <
    Le,           // <=
}

#[derive(Debug, Clone, Deserialize)]
pub struct RuleCondition {
    /// 条件左值路径，如 "request.body.model"、"request.body.messages.0.content"、"prompt"
    pub left: String,
    pub operator: RuleOperator,
    /// 条件右值（字面量）
    pub right: String,
}

#[derive(Debug, Clone, Deserialize)]
pub enum RewriteOperation {
    Set,
    Delete,
    ArrayAppend,
    ArrayPrepend,
    ArrayRemove,
    ArrayReplace,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouterRuleRewrite {
    /// rewrite 路径，如 "request.body.model"、"request.header.x-custom"
    pub key: String,
    /// 默认 "set"
    #[serde(default)]
    pub operation: RewriteOperation,
    /// rewrite 值（字面量，支持模型别名解析）
    #[serde(default)]
    pub value: Option<String>,
    /// array-replace/remove 时的匹配条件
    #[serde(default)]
    pub r#match: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RouterRule {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub r#type: RouterRuleType,
    /// condition 类型时使用
    #[serde(default)]
    pub condition: Option<RuleCondition>,
    /// model-prefix 类型时使用
    #[serde(default)]
    pub pattern: Option<String>,
    /// 匹配后的模型（便捷语法，等价于 rewrite key=request.body.model operation=set）
    #[serde(default)]
    pub target: Option<String>,
    /// 精细 rewrite 列表
    #[serde(default)]
    pub rewrites: Vec<RouterRuleRewrite>,
    /// per-rule fallback（覆盖全局 fallback）
    #[serde(default)]
    pub fallback: Option<FallbackConfig>,
}

fn default_true() -> bool { true }
```

### 配置格式 (TOML)

```toml
# 保留现有 prompt_rules 向后兼容（独立执行，不做展开为 condition rule——regex 语义与 condition contains 不等价）
[[router.prompt_rules]]
pattern = "(?i)\\bOPUS\\b"
model = "claude-sonnet-4-20250514"
strip_match = true

# 新增：condition rules
[[router.rules]]
id = "long-context"
name = "Long context → big model"
type = "condition"
enabled = true

[router.rules.condition]
left = "request.body.messages"
operator = "contains"
right = "large_file"

[[router.rules.rewrites]]
key = "request.body.model"
operation = "set"
value = "claude-sonnet-4-20250514"

# 新增：model-prefix rules
[[router.rules]]
id = "prefix-opus"
name = "opus* → big model"
type = "model-prefix"
pattern = "opus"
target = "claude-sonnet-4-20250514"
```

### 路由执行顺序

```
0. Auto-mapping（现有：model name regex → default model）
1. WebSearch（现有：has_web_search_tool → websearch model）
2. Background（现有：is_background_task → background model）
3. Subagent（现有+P0.3：CCM-SUBAGENT-MODEL tag + tool description 注入）
4. Router Rules（新增：condition + model-prefix → rewrite）
5. Prompt Rules（现有：regex pattern → model，独立执行，不做展开为 condition rule）
6. Think/Plan（现有：is_plan_mode → think model）
7. Default fallback（现有）
```

### 实现步骤

#### 1. `src/router/mod.rs` — 新增 RouterRule 数据结构 + matching 逻辑

新增 ~80 行：
- `RouterRule` / `RuleCondition` / `RuleOperator` / `RouterRuleRewrite` / `RewriteOperation` 结构体
- `match_rule(rule, request, body) -> bool`：按 rule.type 分发到 `match_condition` / `match_model_prefix`
- `match_condition(condition, request) -> bool`：解析 left 路径 → 读值 → 按 operator 比较
- `match_model_prefix(pattern, model) -> bool`：`model.starts_with(pattern)`
- `apply_rewrite(rewrite, request, body)`：解析 key 路径 → 按 operation 修改
- `resolve_path_value(path, body) -> Option<&Value>`：支持 `request.body.model` / `request.body.messages.0.content` / `request.header.x-custom`
- 现有 `prompt_rules` 保留，独立执行不改（regex 语义与 condition `contains` 不等价，不做展开）

### ⚠️ 实施风险

| # | 风险 | 说明 |
|---|------|------|
| P0-R1 | `RouterRule.fallback` 引用 `FallbackConfig`（P2 定义） | P0 先实施时 `fallback` 字段用 `Option<FallbackConfig>` 预留占位，P2 实施时填充。或 P0 先定义 `FallbackConfig` 基本结构 |
| P0-R2 | `resolve_path_value` 需区分 JSON 数组索引 vs object key | `messages.0.content` 中 `0` 需解析为 `Value::Array` 的 index，而非 `Value::Object` 的 key。实现时需 `parse::<usize>()` 尝试 |
| P0-R3 | Router Rules 插入位置 | 在 `route()` 方法中 `match_prompt_rule`（步骤 5）之前插入 `match_router_rule` 调用（作为新步骤 4）。现有链是 fallthrough return，插入新步骤只需在 prompt rules 检查前加一个 if-return |

#### 2. `src/cli/mod.rs` — 配置解析

新增 ~30 行：
- `AppConfig.router.rules: Vec<RouterRule>` 字段 + TOML derive
- 保留 `AppConfig.router.prompt_rules` 向后兼容

#### 3. `src/router/mod.rs` — `route()` 方法集成

新增 ~25 行：
- `route()` 方法的步骤 4（现有 prompt rules）之前插入 rules 匹配
- 遍历 `self.config.router.rules`，对 enabled 规则按顺序匹配
- 第一个匹配的规则触发 rewrite 并返回 `RouteDecision`
- 未匹配则继续到现有 prompt rules → think → default

#### 4. `src/server/mod.rs` — handler 集成

新增 ~10 行：
- `handle_messages` 中 `route()` 调用不变（route 内部处理 rules）
- 确保 `request` 中的 body 变更回写到 `anthropic_request`（route 已做 clone + modify）

### 文件改动

| 文件 | 改动类型 | 代码量 |
|------|----------|--------|
| `src/router/mod.rs` | 新增结构体 + matching + rewrite 逻辑 | ~80 行 |
| `src/cli/mod.rs` | `RouterRule` 相关字段 + TOML derive | ~30 行 |
| `src/router/mod.rs`（route 方法） | rules 集成到路由链 | ~25 行 |
| `src/server/mod.rs` | 确保 body 变更回写 | ~10 行 |
| **合计** | | **~145 行** |

---

## P0.2: Token 阈值路由

### 目标

在路由决策前计算请求的 token 数，支持基于 token 阈值触发路由规则。

> **审计修正**：原文档声称 CCR `longContext`/`longContextThreshold` 用于路由，实际验证发现这些字段仅在 `app.ts:528-529` 类型定义和 `default-config.ts:41` 默认值中存在，**server 端和 plugin 路由逻辑中完全未使用**（grep 确认 0 匹配）。CCR 的替代方案是通过 `request.tokenCount > N` 的 condition rule 达成等效效果。CCM 应直接实现 `tokenCount > threshold` 条件路由，不引入 `longContext` 字段。

### CCR 实现

```typescript
// claude-code-router-plugin.ts:51
const tokenCount = calculateTokenCount(body.messages, body.system, body.tools);
// L657: function calculateTokenCount() 定义
// L58: request.tokenCount = tokenCount — 注入到 request 对象
```

### CCM 方案

CCM 已有 `tiktoken-rs` 依赖（`Cargo.toml: tiktoken-rs = "0.5"`），可以直接复用。

#### 1. `src/router/mod.rs` — 阈值检测

新增 ~15 行：
- `RouterRule` 增加 `threshold: Option<u32>` 字段（token 数 ≥ threshold 时规则触发）
- `match_rule` 中在 condition/model-prefix 匹配通过后，额外检查 threshold
- threshold = None → 不检查；threshold = Some(n) → `token_count >= n` 才触发

#### 2. `src/server/mod.rs` — token 计算注入

新增 ~20 行：
- `handle_messages` 在 `route()` 调用前，使用 `tiktoken-rs` 计算 request token count
- 将 token count 注入 `AnthropicRequest`（新增 `#[serde(skip)] pub token_count: Option<u32>` 字段）
- `route()` 方法从 request 中读取 token_count 传入 `match_rule`

#### 3. `src/models/mod.rs`

新增 ~5 行：
- `AnthropicRequest` 增加 `#[serde(skip)] pub token_count: Option<u32>` 字段

### 配置示例

```toml
[[router.rules]]
id = "long-context-threshold"
name = "Long context (>100k tokens) → big model"
type = "condition"
enabled = true
threshold = 100000

[router.rules.condition]
left = "request.body.model"
operator = "=="
right = "claude-sonnet-4-20250514"

[[router.rules.rewrites]]
key = "request.body.model"
operation = "set"
value = "claude-opus-4-20250514"
```

### 文件改动

| 文件 | 改动类型 | 代码量 |
|------|----------|--------|
| `src/router/mod.rs` | threshold 字段 + 检测逻辑 | ~15 行 |
| `src/server/mod.rs` | token 计算注入 | ~20 行 |
| `src/models/mod.rs` | `AnthropicRequest.token_count` 字段 | ~5 行 |
| **合计** | | **~40 行** |

---

## P0.3: Subagent Tool Description 注入

### 目标

向 subagent tool 的 description 中注入 CCM 路由指令，确保 agent 知道如何正确使用 `<CCM-SUBAGENT-MODEL>` tag。

> **审计修正**：原文档引用 `claude-code-router-plugin.ts:301-306` 中的 `ccrSubagentToolModelInstruction` 和 `injectClaudeCodeAgentToolDescription(body, config)` 函数。实际 grep 确认：**当前 CCR 代码中不存在这两个函数**。`<CCR-SUBAGENT-MODEL>` 仅出现在 `request-log-store.ts` 的日志搜索正则中。P0.3 需作为 CCM 原创功能设计，不是从 CCR 移植。

### 设计依据

CCR 虽然不在 tool description 中注入指令，但 CCR 的 subagent 路由仍然依赖 system prompt 中的 `<CCR-SUBAGENT-MODEL>` tag 检测。CCM 现有的 `extract_subagent_model` 已做了同样的 tag 提取。P0.3 的增强是在 **tool description 中追加指令**，让 agent 在调用 subagent tool 时主动添加 tag，而非仅靠 system prompt 约定。

### ~~CCR 实现~~（原文档引用有误，保留以记录修正过程）

```typescript
// 原文档声称 claude-code-router-plugin.ts:301-306 存在以下代码
// 实际 grep 确认：这行在当前 CCR 代码中不存在
// const ccrSubagentToolModelInstruction = ...
// injectClaudeCodeAgentToolDescription(body, config);
// 
// 修正：P0.3 是 CCM 原创功能，以下方案不依赖 CCR 实现
```

### CCM 方案

#### 1. `src/router/agent.rs`（新建）

新增 ~80 行：

```rust
/// Subagent tool description 注入指令
const SUBAGENT_TOOL_INSTRUCTION: &str =
    "CCM subagent routing is enabled. When calling this tool, the prompt parameter MUST start with \
     <CCM-SUBAGENT-MODEL>Provider/model</CCM-SUBAGENT-MODEL> on its own first line, \
     replacing Provider/model with the best configured CCM model. \
     CCM consumes the tag, removes it from the prompt, and routes the spawned agent request to that model.";

/// 检测请求中是否有 subagent 相关 tool
fn has_subagent_tool(request: &AnthropicRequest) -> bool {
    if let Some(ref tools) = request.tools {
        tools.iter().any(|tool| {
            tool.r#type.as_ref().map(|t| t == "subagent").unwrap_or(false)
                || tool.name.as_ref().map(|n| {
                    n.contains("agent") || n.contains("task") || n == "delegate"
                }).unwrap_or(false)
        })
    } else {
        false
    }
}

/// 向 subagent tool 的 description 末尾追加指令
pub fn inject_subagent_tool_instruction(request: &mut AnthropicRequest) {
    if let Some(ref mut tools) = request.tools {
        for tool in tools.iter_mut() {
            if is_subagent_tool(tool) {
                if let Some(ref mut desc) = tool.description {
                    if !desc.contains("CCM-SUBAGENT-MODEL") {
                        desc.push_str("\n\n");
                        desc.push_str(SUBAGENT_TOOL_INSTRUCTION);
                    }
                }
            }
        }
    }
}
```

#### 2. `src/server/mod.rs`

新增 ~10 行：
- `handle_messages` 入口处（route 调用前）：
  ```rust
  crate::router::agent::inject_subagent_tool_instruction(&mut anthropic_request);
  ```

#### 3. 现有 `src/router/mod.rs` 中 `extract_subagent_model`

已有 `<CCM-SUBAGENT-MODEL>` tag 提取逻辑保持不变。

### 文件改动

| 文件 | 改动类型 | 代码量 |
|------|----------|--------|
| `src/router/agent.rs`（新建） | 指令常量 + tool 检测 + description 注入 | ~80 行 |
| `src/server/mod.rs` | handler 中调用注入 | ~10 行 |
| `src/lib.rs` | `pub mod router` 子模块声明 | ~1 行 |
| **合计** | | **~91 行** |

---

## P0 总计

| 子项 | 代码量 | 工时 |
|------|--------|------|
| P0.1 Router Rules 升级 | ~145 行 | 3h |
| P0.2 Token 阈值路由 | ~40 行 | 1h |
| P0.3 Subagent tool 注入 | ~91 行 | 2h |
| **合计** | **~276 行** | **6h** |

---

## 验证

```bash
cargo check
cargo test
cargo clippy --no-deps
```

新增单元测试（计划名 vs 实际代码名，2026-07-11 审计核实）：
- `test_condition_rule_match` → 实际：`test_router_rule_condition_eq_model` 等 condition operator 系列
- `test_model_prefix_match` → 实际：`test_router_rule_model_prefix`
- `test_rewrite_set_body_model` → 实际：`test_real_router_rule_rewrite_model`
- `test_rewrite_delete_header` → 实际：`test_rewrite_header_delete`
- `test_threshold_routing` → 实际：`test_threshold_blocks` / `test_threshold_passes` / `test_threshold_none` / `test_threshold_lazy_computation`（4 个）
- `test_subagent_tool_injection` → 不存在（P0.3 未实施，该测试随功能一并跳过）
- `test_prompt_rules_backward_compat` → 实际：`test_prompt_rule_*` 系列
- `test_contains_deep_operator` → 实际：`test_contains_capture_reference`

---

## 审计修正日志（2026-07-03）

| # | 原始内容 | 修正为 | 依据 |
|---|---------|--------|------|
| 1 | "CCR 支持 regex + `$1` capture references" | CCR 不支持 regex；这是 CCM 独有能力 | CCR `RouterRuleOperator` (`app.ts:466-476`) 共 10 个运算符，无 regex。`routerRuleConditionMatches()` (`plugin:408-450`) 实现的均为字符串比较 |
| 2 | "CCR `longContext`/`longContextThreshold` 用于超长上下文路由" | 仅类型定义存在，路由逻辑未使用 | grep `service.ts` + `plugin.ts` = 0 匹配。仅 `app.ts:528-529`、`config.ts`、`default-config.ts`、renderer UI 出现 |
| 3 | "`RouterRuleOperator` 9 个 operator" | 10 个 operator | 遗漏 `contains-deep`（`app.ts:474`）— 深度递归 JSON 搜索 |
| 4 | "CCR `injectClaudeCodeAgentToolDescription()` 函数 L301-306" | 该函数在当前 CCR 代码中不存在 | grep `injectClaudeCodeAgentToolDescription` 跨 `src/` = 0 匹配。`<CCR-SUBAGENT-MODEL>` 仅在 `request-log-store.ts` 日志正则中出现 |
| 4b | "P0.3 是从 CCR 移植" | P0.3 是 CCM 原创功能 | CCR 无对应实现可供移植 |
| 4c | "CCR 路由 6 层" | CCR 是声明式 rules 数组，非固定 6 层 | CCR `resolveConfiguredRouteDecision()` (`plugin:178-198`) 遍历 rules 数组；`webSearch`/`background`/`think` 是顶层配置字段 |
| 5 | `calculateTokenCount` 引用 `plugin:64` | 实际 L51 调用，L657 函数定义 | grep 确认 |
| 6 | `prompt_rules` "展开为 condition rule" | regex 语义与 `contains` 不等价，不做展开 | CCM `prompt_rules` 用 `Regex`，condition 用字符串匹配 |
| 7 | — | 补充 P0-R1/R2/R3 实施风险说明 | — |
| 8 | 三轮自审：对比表 L19 列出 9 个 operator | 实际 10 个，遗漏 `not-contains` | CCR `app.ts:466-476` RouterRuleOperator 共 10 个 |
| 9 | 三轮自审：L148 "prompt_rules 作为 syntactic sugar（展开为 condition rule）" 与 L204 矛盾 | 修正为"独立执行，不做展开" | regex 语义与 condition contains 不等价 |
| 10 | 三轮自审：L188 "prompt rules 作为 condition 子集的 syntactic sugar" 与 L204 矛盾 | 修正为"独立执行，不做展开" | 同 #9 |
