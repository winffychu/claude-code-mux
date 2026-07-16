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

**config.example.toml 示例全用 `request.body.model`，但那是靠特判（L448），
不是 fallback。对 model/messages/system/tools 之外的任何字段（thinking、
temperature、metadata…），必须用无 `body.` 前缀的顶层路径。这是文档应澄清的点。**

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

是否扩展取决于用户的 think 用例 —— 若客户端从不发顶层 thinking 且无模型名
线索，则**任何路由层都无法自动判定**，只能靠 prompt-rule（用户在 prompt 里
写触发词）或 subagent 标签，这已超出"think 路由"范畴。
