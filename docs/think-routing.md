# Think 路由配方与"为何大部分走 default"根因

> 状态：调研完成（2026-07-16），待用户拍板处理方向
> 关联：p0 RouterRule（regex + $ref），p6-memory-logs.md（日志让路由决策可观测）

---

## 1. CCR vs CCM 对比

### Claude Code Router (CCR) 现状

CCR **已删除 `legacy-thinking` 硬编码层**（`src/main/config.ts` / `options.ts`
里 `legacy-thinking` 仅作 config key 残留）。它的 think 路由完全依赖两种
**通用**机制，无 think 专用 condition：

| 机制 | 实现 | think 能用吗 |
|------|------|:---:|
| inline route | `isKnownInlineRoute`，请求里直写 `provider/model` 命中 | ✅ |
| condition rule | `routerRuleConditionFromLegacy` 只处理 `model-prefix` | ✅（手配） |

CCR 另有 `supportsReasoning` capability + `reasoning_effort`，但那是**响应
侧**处理（控制模型推理预算/输出），**不做请求路由**。即 CCR 不自动路由
think，用户必须 inline route 或手配 `model-prefix` 规则。

### CCM (我们) 现状

`router/mod.rs::route()` 优先级（L186-300）：

```
0. auto_map_regex   （模型名正则 → default 重写，最先）
1. webSearch        （tools 含 web_search）
2. background       （模型名 regex 匹配 haiku 等，成本优化前置）
3. subagent         （system prompt 含 CCM-SUBAGENT-MODEL）
4. router rules     （RouterRule condition + model-prefix，regex+$ref）★
5. prompt rules     （user prompt 正则）
6. think           （is_plan_mode：顶层 thinking.type=="enabled"）
7. long context     （token_count >= threshold）
8. default          （auto-mapped 或原模型名）
```

**我们的 RouterRule（优先级 4）比 CCR 强**：支持 **regex 匹配**（CCR inline
route 只字符串前缀）+ **`$ref` 复用**规则块。

**但 think 用的是独立的优先级 6 硬编码 `is_plan_mode`**，不是 RouterRule。

---

## 2. 为什么大部分请求走 default（根因实证）

### 实证方法

启动 CCM（tracing 开），发 4 种 think 信号格式，看 `route_type`：

| # | 请求形式 | route_type | model |
|---|----------|:---:|:---|
| 1 | 顶层 `thinking:{"type":"enabled"}` | **think** ✅ | m |
| 2 | 顶层 `thinking:{"type":"disabled"}` | default | m |
| 3 | **无 thinking 字段** | default | m |
| 4 | **模型名带 `thinking`** | default | m |

### 根因

`is_plan_mode`（router/mod.rs L579-585）只认一个条件：

```rust
fn is_plan_mode(&self, request: &AnthropicRequest) -> bool {
    request.thinking.as_ref()
        .map(|t| t.r#type == "enabled")
        .unwrap_or(false)
}
```

它的检测范围是 **唯一的**：请求必须带顶层 `thinking` 字段且
`type == "enabled"`。而：

- **Claude Code 客户端大部分请求不发顶层 `thinking` 字段** —— 只有用户
  显式开启 extended thinking 时才发。日常对话、工具调用、后台任务都不发。
- 模型名带 think（`claude-3-5-sonnet-thinking`）不会被 think 路由捕获，
  因为 auto_map_regex / background_regex / think 三个都不匹配模型名里的
  "thinking"；如果不在 `default` 映射表里则另一层落空。

两条合起来：think 路由几乎从不触发 → 绝大多数请求落 default。

---

## 3. 这与 RouterRule "更强" 不矛盾

我们的**通用** RouterRule 确实比 CCR 强（regex + $ref）。问题是 think 路由
**没有用** RouterRule，而是用了一个独立的、检测面极窄的 `is_plan_mode`
硬编码层。换言之：我们有一个强大的通用路由引擎，却给 think 单独配了一
个孱弱的专用检测。

CCR 的演进方向（删 legacy-thinking）说明：**专用 think 检测层是技术债**，
应该让通用规则覆盖 think。

---

## 4. 处理方向与实证结论

### 推荐：方向 A —— 用通用 RouterRule 覆盖 think（实证可行）

**已实证**：用一条 RouterRule condition 即可捕获 think，无需 `[router].think`
硬编码层，与 CCR 演进方向（删专用层）一致。

配方（实测命中 `route_type = "prompt-rule"`）：

```toml
[[router.rules]]
id = "route-thinking"
name = "Route extended-thinking requests to think model"
type = "condition"
left = "thinking.type"        # ⚠️ 必须，不能带 body. 前缀（见 §6 陷阱）
operator = "=="
right = "enabled"
model = "think-target-model"
```

实测矩阵（`[router].think` 未设，纯靠 condition rule）：

| 请求 | route_type | model | 期望 |
|------|:---:|:---:|:---:|
| thinking.type=enabled | prompt-rule | thinking-m | ✅ 命中 |
| thinking.type=disabled | default | m | ✅ 不命中 |
| 无 thinking 字段 | default | m | ✅ 不命中 |

对比 `is_plan_mode` 硬编码层：两者**恰好**捕获同一种信号（顶层
thinking.type=="enabled"），RouterRule 方案做同样的事但不需专用代码，可
配置、可组合（加 threshold、加更多 condition）。

### ⚠️ 不解决的根本问题

方向 A 和现有 `is_plan_mode` **一样只认顶层 thinking 字段**。若用户的客户端
根本不发顶层 thinking（即 §2 根因），**两种方案都不会命中**，仍走 default。
要覆盖"无顶层 thinking 字段"的请求（如模型名带 think、system prompt 关键词），
需额外规则（见 §7 扩展配方），那才是真正减少"走 default"的关键。

---

## 5. 方向 B/C 摘要（不推荐）

- **方向 B（放宽 is_plan_mode）**：加模型名/system prompt 关键词匹配。与
  CCR 去专用化方向相反，新增硬编码。
- **方向 C（删 is_plan_mode，纯走 rules）**：架构最干净但破坏现有 `[router].think`
  用户预期，需迁移。

方向 A 已能达到 C 的"通用规则覆盖 think"目标，且不破坏 `[router].think`
向后兼容，故优先于 C。

---

## 6. 路径陷阱 —— request.body.* vs 顶层字段

`resolve_path_value`（router/mod.rs L444）的路径解析逻辑：

1. **特判**：`model` / `body.model`、`messages` / `body.messages`、
   `messages.<n>.content`、`system` / `body.system`、`tools` / `body.tools`
   → 这些接受带或不带 `body.` 前缀。
2. **fallback**（L490-505）：把整个 `AnthropicRequest` 序列化成 JSON Value，
   再按 `path.split('.')` 逐段 `get()` 遍历。

**陷阱**：fallback 序列化后的 JSON 顶层**没有 `body` 包裹**
（AnthropicRequest 字段直接在顶层：model/messages/thinking/…）。
所以 `request.body.thinking.type` 在 fallback 里第一段 `body` 取不到 →
condition 永不命中。正确路径是 `thinking.type`（去掉 `body.`）。

实证：`left = "request.body.thinking.type"` → 不命中（route_type=default）；
`left = "thinking.type"`（或 `request.thinking.type`，strip 前缀后同）→ 命中。

**已修复（②）**：`resolve_path_value` fallback 现在会跳过开头的 `body` 段
（router/mod.rsFallback 注释），让 `request.body.thinking.type` 等价于
`request.thinking.type`。新增单元测试
`test_resolve_path_supports_body_prefix_for_arbitrary_fields` 覆盖三种写法
（裸、`body.`、`request.body.`）均命中 + disabled 负例不命中。

**文档已澄清（①）**：config.example.toml 加了"Path convention"说明块与
think 配方 Example 3，明确 `body.` 前缀对顶层任意字段被容忍并剥离。

---

## 7. 扩展配方 —— 真正减少"走 default"

方向 A 的基础配方只覆盖"客户端发顶层 thinking"的情况。要覆盖更多 think 信号
（减少落 default），可叠加规则：

```toml
# 规则 1：顶层 thinking.type == enabled（基础）
[[router.rules]]
type = "condition"
left = "thinking.type"
operator = "=="
right = "enabled"
model = "think-target-model"

# 规则 2：模型名含 "thinking"（覆盖模型变体式 think）
[[router.rules]]
type = "condition"
left = "model"
operator = "contains"
right = "thinking"
model = "think-target-model"
```

## 8. 真实流量数据佐证（远程实例 172.168.0.71:13456）

读取用户实际在用的 ccm 实例 `/api/logs`（分页拉取 2015 条历史，tracing 开启）：

| route_type | 数量 | 占比 |
|:---:|---:|---:|
| default | 1890 | **93.8%** |
| think | 74 | 3.7% |
| error(502) | 51 | 2.5% |

**远程配置轮廓（从行为逆向推断，因远程无 /api/config 端点且 SSH 不通）**：

- **provider × model**：
  - `sub2api-cc` —— actual_model `glm-5`（default & think 同一 target）
  - `nvidia` —— actual_model `z-ai/glm-5.2`
- **router**：`default = glm-5`，`think = glm-5`（think 同 default，
  命中 74 条但不换模型 → 思考路由在此实例无实际意义）。
- **无 `[router.rules]` condition / 无 prompt_rules** —— route_type
  字段从未出现 `prompt-rule` / `long-context` / `web-search`，think 全靠
  `[router] think` 硬编码（即 `is_plan_mode`）命中。
- **auto_map 默认 `^claude-`** —— `z-ai/glm-5.2` 不以 `claude-` 开头故不被吃，
  保留原名走 default（52 条 nvidia + 1840 条 sub2api-cc glm-5 = default 1890）。

**疑似配置问题**：51 条 502 错误全是 `z-ai/glm-5.2` via `sub2api-cc`（上游
报 `Upstream request failed`），但同一 actual_model `z-ai/glm-5.2` 经由
`nvidia` 的 52 条无误 → `glm-5.2` 的 mappings fallback chain 把 sub2api-cc
排在 nvidia 之前，而 sub2api-cc 在该模型上不可用。

---

## 9. 根因链（本地实证完整还原）

`route()` 优先级（router/mod.rs L186）：**auto_map 在最前（优先级0）**，
think 在第 6。两步顺序决定了根因：

1. **auto_map 吃掉 think 信号** — 默认 `auto_map_regex = ^claude-`
   （L96，None 时也回退此默认）。任何 `claude-3-5-sonnet-thinking` 这类
   Claude 模型名在到达 think 规则前，已被 L191-198 重写为 default。
   实测：`auto_map="^$"`（关）→ `model contains "thinking"` 规则命中
   （route_type=prompt-rule）；`auto_map=""`（默认 ^claude-）→ 不命中。
2. **is_plan_mode 只认顶层 thinking 字段** — 大部分 Claude Code 请求
   不发顶层 `thinking` 对象（只有显式开 extended thinking 才发）→ think 路由
   不触发。
3. 两步叠加 → default 吃掉 92%。

即：我们有一个比 CCR 强的通用 RouterRule 引擎，但 think 路由**没用它**（用了
窄的 `is_plan_mode` 硬编码层），而本可补救的模型名规则又被 auto_map 在
更早一步吃掉了信号。

---

## 10. 是否改代码 —— 倾向与建议

### 不必须改代码的情况

如果用户 think 用例就是"客户端显式发顶层 thinking"（远程 74 条命中即此），
则现状 `is_plan_mode` 已够用。方向 A 的 condition 配方可让 think 走通用规则，
但功能等价，只是架构更干净（可选）。

### 值得改代码的情况

**§6 路径陷阱**（已修复 ②）—— fallback 对 `request.body.<非特判字段>` 不工作。
选择修代码（通用增强）：`resolve_path_value` fallback 跳过开头 `body` 段，
让 `request.body.thinking.type` ≡ `request.thinking.type`。新增单元测试覆盖。
同时修文档（①）：config.example.toml 澄清路径约定 + 加 think 配方 Example 3。

**auto_map 吃 think 信号**是设计权衡，保持不改（③）：
- 现状（auto_map 最前，默认 `^claude-`）：把未知 claude-* 模型名归入 default，
  安全但吃 think 信号。用户可显式设 `auto_map_regex = "^$"` 关掉。
- 后移到 rules 之后能保住信号但破坏所有现有用户的默认行为，故不动。
  文档已说明如何关（本节 + config.example.toml 注释）。

### 已做 / 不做的总结

- ✅ ① 改文档澄清路径陷阱（config.example.toml "Path convention" + Example 3）。
- ✅ ② 修代码让 fallback 支持 `request.body.*` 通用（+ 单元测试）。
- ✅ ④ 不删 `is_plan_mode`（保持 `[router].think` 向后兼容），文档推荐方向 A
  配方替代。
- ❄️ ③ 不动 auto_map 顺序（保持默认行为，文档说明如何关）。
