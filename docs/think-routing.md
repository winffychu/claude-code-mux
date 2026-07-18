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

**修正（2026-07-18）**：本节原始内容基于"远程无 `/api/config` 端点、SSH 不通
则逆向推断"，**不准确**。从 `src/server/mod.rs` 路由表（L159）确认远程实际
暴露了 `GET /api/config/json` 端点，可直接读真实配置。以下按真配置回写。

### 8.1 真实配置（`GET /api/config/json` 直读）

```
[router] default=claude-haiku-4-5, think=claude-haiku-4-5, websearch=deepseek-v4-flash,
         long_context=deepseek-v4-pro (threshold=202000),
         auto_map_regex=null → 回退默认 ^claude-,
         background_regex=null → 回退默认 (?i)claude.*haiku,
         rules=[], prompt_rules=[]
[providers] sub2api-cc(anthropic,172.168.0.83:58083), nvidia(openai,172.168.0.82:3001),
            agnes-ai(openai,apihub.agnes-ai.com)
[models] 5 个; claude-haiku-4-5 有 5 条 mappings fallback:
          1: sub2api-cc→glm-5, 2: nvidia→z-ai/glm-5.2, 3: nvidia→deepseek-v4-pro,
          4: sub2api-cc→deepseek-v4-pro, 5: agnes-ai→agnes-2.0-flash
```

### 8.2 真实流量分布（`GET /api/logs`）

> ⚠️ **count 口径警告**: `/api/logs` 的一个请求经 fallback 链会触发**多条**
> `req` 条目（每个 provider mapping attempt 一条 `trace_request`，见 §8.4），
> 所以 `total` 是"trace 条目数"而非"请求数"。统计占比应按 `id` 去重。

抽样 50 条（remote ring buffer 最新）：`route_type=default` 占绝大多数，`model`
为 `glm-5`（actual_model，经 sub2api-cc 转发）。

### 8.3 为何大部分走 default —— 真实根因（修正 §9）

远程客户端发往 ccm 的 `request.body.model` 是 **`glm-5`**（已是 actual_model，
不是 `claude-haiku-4-5` 这种路由名）。又因远程 `[router] default=think=claude-haiku-4-5`
但 `claude-haiku-4-5` 的 mappings fallback 第一名是 `sub2api-cc→glm-5`——
即一个发 `glm-5` 的请求也被（auto_map 不触发，因为 `glm-5` 不匹配 `^claude-`）
直接落 default 路由，再经 mapping 转发到 sub2api-cc→glm-5 actual。

换言之，远程"92% default"的真根因是 **客户端直接发 actual_model 名（`glm-5`）**，
既不触发 auto_map，也不触发 think（无顶层 `thinking`）、long_context、websearch。
**§9 的"auto_map 吃掉 think 信号"在此真实配置下不适用**——因为客户端压根
不发 `claude-*` 模型名。

### 8.4 相关行为（非缺陷，已确认是设计）

- **fallback 每次 attempt 在 `/api/logs` 各记一条 `req`**：`src/server/mod.rs`
  L647/OpenAI 端点、L923/Anthropic 端点的 `trace_request` 在 fallback for 循环
  内，每 mapping flag 一条。这与 stdout `info!`（L599 `retry_info=[n/N]`）
  一致——日志和终端**都正确反映每次 fallback 链路**，可观测，非 bug。
- **err 条目在 `/api/logs` 丢了 model/provider/route_type**（`trace_error`
  `src/message_tracing/mod.rs` L337-361 把这些字段置 None）：影响为网页
  err 行无内联路由详情；但 `read_recent` 支持 `?id=<trace_id>` 过滤，前端按
  trace id 查询能看到同 id 的 req entry 完整链路，所以影响有限。
  非本轮处理范围。
- **`claude-haiku-4-5` 会被 `background_regex` 默认 `(?i)claude.*haiku` 命中**→
  route_type=background。但远程客户端发 `glm-5` 不命中此正则，故远程日志
  从未出现 background 分类。

### 8.5 真实上游 502 来源

远程历史里 `z-ai/glm-5.2` 相关 502 是 `claude-haiku-4-5` fallback chain
第 2 位（nvidia→`z-ai/glm-5.2`）尝试失败而后续 mapping 成功的真实上游故障。
fallback chain 本身工作正常（会继续尝试第 3-5 位并成功），**非配置错误**。

---

## 9. 根因链（本地实证 + 真实配置修正）

`route()` 优先级（router/mod.rs L186）：**auto_map 在最前（优先级0）**，
think 在第 6。

### 9.1 理论路径（若客户端发 `claude-*` 模型名）

1. **auto_map 可能吃掉 think 信号** —— 默认 `auto_map_regex = ^claude-`
   （None 时也回退此默认，L96）。`claude-3-5-sonnet-thinking` 这类
   Claude 模型名在到达 think 规则前会被 L191-198 重写为 default。
   本地实测：`auto_map="^$"`（关）→ `model contains "thinking"` 规则命中
   （route_type=prompt-rule）；`auto_map=""`（默认 ^claude-）→ 不命中。
2. **is_plan_mode 只认顶层 thinking 字段** —— 客户端不发顶层 `thinking`
   对象 → think 路由不触发。

### 9.2 真实配置下的实际路径（远程 172.168.0.71）

远程客户端发 `request.body.model = glm-5`**（actual_model 名）**：

1. `glm-5` 不匹配 `^claude-` → **auto_map 不触发**（9.1 此处不适用）
2. 不匹配 `(?i)claude.*haiku` → background 不触发
3. 无顶层 thinking → think 不触发
4. 直接落 **default** 路由（路由名 `claude-haiku-4-5`），再经 mappings
   fallback 转到 actual `glm-5` (priority 1 sub2api-cc)

**故§9.1 的"auto_map 吃信号"在远程真配置下不是主因**——主因是客户端
直接发 actual_model 名，跳过了所有路由分类层。think 路由 74 条命中是
少部分客户端显式发顶层 `thinking` 的情况。

### 9.3 结论

"大部分走 default"有两个独立成因：
- **9.1 路径**（claude-* 客户端）：auto_map 提前吃掉信号 + is_plan_mode 过窄
- **9.2 路径**（actual-model 客户端，远程真实情况）：客户端根本不走路由分类

要减少 default，**必须知道客户端发什么 model**——若发 actual_model，路由层
怎么扩 think 检测都没用（§4 方向 A 也救不了，因为信号在 model 名而非 routing field）。

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
