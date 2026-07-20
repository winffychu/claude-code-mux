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

`router/mod.rs::route()` 优先级（L194-310，**`cost_first=true` 模式**；默认 `cost_first=false` 见本文件 §11.12）：

```
0. auto_map_regex   （模型名正则 → default 重写，最先）
1. webSearch        （tools 含 web_search，两种模式相同）
2. background       （模型名 regex 匹配 haiku 等，成本优化前置）
3. subagent         （system prompt 含 CCM-SUBAGENT-MODEL）
4. router rules     （RouterRule condition + model-prefix，regex+$ref）★
5. prompt rules     （user prompt 正则）
6. think           （is_plan_mode：顶层 thinking.type=="enabled"）
7. long context     （token_count >= threshold）
8. default          （auto-mapped 或原模型名）
```

默认 `cost_first=false`（think-first，匹配上游 9j）顺序为：

```
0. auto_map_regex → 1. webSearch → 2. subagent → 3. think → 4. background
  → 5. router rules → 6. prompt rules → 7. long context → 8. default
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

`is_plan_mode`（router/mod.rs L694）只认一个条件：

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

`resolve_path_value`（router/mod.rs L533）的路径解析逻辑：

1. **特判**：`model` / `body.model`、`messages` / `body.messages`、
   `messages.<n>.content`、`system` / `body.system`、`tools` / `body.tools`
   → 这些接受带或不带 `body.` 前缀。
2. **fallback**（L~570-590）：把整个 `AnthropicRequest` 序列化成 JSON Value，
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
  L703/OpenAI 端点、L979/Anthropic 端点的 `trace_request` 在 fallback for 循环
  内，每 mapping flag 一条。这与 stdout `info!`（L655 `retry_info=[n/N]`）
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

`route()` 优先级（router/mod.rs L194）：**auto_map 在最前（优先级0）**，
think 在第 6。

### 9.1 理论路径（若客户端发 `claude-*` 模型名）

1. **auto_map 可能吃掉 think 信号** —— 默认 `auto_map_regex = ^claude-`
   （None 时也回退此默认，L96）。`claude-3-5-sonnet-thinking` 这类
   Claude 模型名在到达 think 规则前会被 L200-206 重写为 default。
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

---

## 11. Fork vs 上游 route() 优先级链审计（2026-07-18）

用户要求"结合上游源码分析我们是否破坏了 think"。对比 9j 原始仓 +
`elidickinson` 中间 fork + 我们 fork 的 `route()` 优先级链：

| priority | 上游 `9j/claude-code-mux` | `elidickinson` fork | 我们 fork (`winffychu`) |
|:---:|---|---|---|
| 0 | auto_map | auto_map | auto_map |
| 1 | WebSearch | WebSearch | WebSearch |
| 2 | **Subagent** | **Background** ←前移 | **Background** ←前移 |
| 3 | **Think** | Subagent | Subagent |
| 4 | **Background** | Prompt Rules (新) | Router Rules (新) |
| 5 | Default | **Think** ←后挪 | Prompt Rules (新) |
| 6 | | Default | **Think** ←再后挪 |
| 7 | | | Long Context (新) |
| 8 | | | Default |

**溯源发现**：

- **background 前置**（位 2，think 之前）是 `elidickinson` fork 引入的（注释
  "checked early to save costs"），**不是我们 fork 引入的**。我们继承了它。
- 我们 fork 在 `elidickinson` 的基础上**插入了 Router Rules (位 4) and
  Long Context (位 7)**，把 think 从位 5 再推到位 6。

### 11.1 真Functional regression（已真机复现）

客户端发 `model=claude-haiku-4-5` + `thinking.type=enabled`：

- **上游 9j**：`is_background_task("claude-haiku-4-5")` 在位 4 检查但**think
  在位 3 已先命中** → 走 `think` model ✅
- **我们 fork**：`is_background_task("claude-haiku-4-5")` 在位 2 匹配 `(?i)claude.*haiku`
  就命中 → 走 `background` model，**think 永远不被检查** ❌

真机实证（`cfg-remote-mirror3.toml` 真配置镜像 + 本地 debug binary）：
发 payload `{"model":"claude-haiku-4-5","thinking":{"type":"enabled",...}}` 后
`/api/logs` 路由结果是 `route_type=background` 而非 `think`。

### 11.2 测试盲区根因

`test_routing_priority`（L1227）声称 "Think wins" 但用的是
`create_simple_request(...)`(model=`claude-opus-4`)，**不匹配** `(?i)claude.*haiku`
正则 → background 不会命中 → think "赢"。这条测试**没有暴露** claude-haiku+think
路径上的 regression。

### 11.3 已做的回归守护（不修代码,只 pin 当前行为）

新增 `test_think_vs_background_when_model_is_claude_haiku`（router/mod.rs
L1255），用 `model="claude-haiku-4-5"` + `thinking.type=enabled`，**pin**
当前 fork 的 `decision.route_type == Background` 行为。注释明确这是相对
上游 9j 的偏离。任何未来重排（无论方向）都会被这条测试挡下要求更新。

### 11.4 决策与实施（B 思路：修回上游 9j 顺序）

用户指示"先按 B 思路测试看看，若有改善则正式实施提交"。实施 B：
将 `route()` 里 Think 块从位 6 **前移到 Background 之前**（位 3），完整链
恢复到与上游 9j 一致：

| 位 | B 实施后(已提交) |
|:--:|---|
| 1 | WebSearch |
| 2 | Subagent |
| 3 | **Think** ← 从位 6 前移 |
| 4 | Background |
| 5 | Router Rules |
| 6 | Prompt Rules |
| 7 | Long Context |
| 8 | Default |

- 单元测试：当时 258 passed 0 failed，无回归（既有测试用 `claude-opus-4` 等非
  haiku 模型不受重排影响）。
- 守护测试 `test_think_vs_background_when_model_is_claude_haiku` 的断言
  从 `RouteType::Background` 改为 `RouteType::Think`，pin 修正后的行为。
- **真机复现改善**：用 `cfg-remote-mirror3.toml` + 新 debug binary 发
  `{"model":"claude-haiku-4-5","thinking":{"type":"enabled"}}`，原 fork
  返回 `route_type=background`，**B 实施后返回 `route_type=think`** ✅。
  fallback chain 仍走 think model 的 provider chain（agnes-ai / sub2api-cc
  fallback 与真配置 think = claude-haiku-4-5 的 mappings 一致）。

实施提交：见 git log "B-experiment: restore think before background (upstream 9j order)"。

### 11.5 设计权衡备注

- B 实施后，"claude-haiku 无 thinking 普通请求"会**多走一次 think 检查**
  （一次 `Option::map` 开销可忽略），再走 background 命中。成本差异微乎其微。
- 偏离 `elidickinson` fork 的"background 前移省成本"设计，但本 fork 本来
  就已与 elidickinson 分家（加了 Router Rules + Long Context）。从该 fork
  同步改动时需手动跳过这条 reorder。
- 远程真配置当前 `default=background=think=claude-haiku-4-5` 同 model，
  即便路由分支不同 fallback chain 仍相同——B 实施在远程**无破坏性影响**，
  仅修正 `/api/logs` 的 route_type 标签语义。

### 11.6 B 实施引入的回归面审计（Router Rules / Prompt Rules）

用户要求"审计是否 think 上位代替 background 造成错误感觉，必须测试"。
完整测试 + 真机实证后确认：B 实施把 Think 从位 6 前移到 Background 前的
位 3，但也让 Think 抢占了**原本在 Think 之前的 Router Rules / Prompt Rules**
（原 fork 顺序 Rules@4 < Think@6；B 实施后 Think@3 < Rules@5）。

**真实回归面**（已有单元 + 真机实证）：

| 场景 | 原 fork | B 实施后 |
|---|---|---|
| `claude-* + thinking=enabled` + 配有匹配 `claude` prefix 的 Router Rule | Router Rule 命中（Rules@4 先于 Think@6）→ 走 rule-target-model | **Think 命中**（Think@3 先于 Rules@5）→ 走 think model |
| `claude-* + thinking=None` + 同 Router Rule | Router Rule 命中 | Router Rule 仍命中（无 thinking 时 Think 不触发） |

单元测试验证（当时 258→267 passed，现 271 passed）：
- `test_think_now_beats_router_rule_when_both_match` — pin B 下 think 抢占
  Router Rule 的新行为（断言 Think, 注释明确"old fork behaviour would have
  returned PromptRule + rule-target.model"）。
- `test_router_rule_still_fires_without_thinking` — pin 无 thinking 时
  Router Rule 仍正常（无回归误伤）。
- `test_background_still_wins_when_claude_haiku_without_thinking` — pin
  claude-haiku 无 thinking 仍走 Background（对称面无破）。
- `test_subagent_wins_over_think_when_both_present` — pin subagent 在 think
  前（9j 顺序对的）。

真机实证（`/tmp/cfg-rule-test.toml` RouterRule ModelPrefix=claude →
rule-target-model, 启 `[server.tracing]`）：

| req | payload | route_type | model |
|---|---|---|---|
| [0] | claude-opus-4 无 thinking | prompt-rule ✅ | rule-glm (rule-target) |
| [1] | claude-opus-4 + thinking.type=enabled | **think** | glm-5 (think model = claude-haiku-4-5) |

**所以 B 实施准确无疑地改变了"thinking + Router Rule 重叠"场景的路由结果**：
原 fork 走 Router Rule 的 model，B 后走 think model。这不是 bug——是 B 修复
background-vs-think 顺带引入的取舍面，与 9j 原始设计语义一致（9j 无 Router
Rules 所以 9j 没这个问题；B 让 think 在 Rules 之前正好与 9j think@3 在
background@4 之前的语义保持一致）。

### 11.7 取舍决策（是否进一步让 Router Rules 覆盖 Think）

若希望用户**显式声明的 Router Rules 优先于 think 自动检测**（即 thinking+
Rule 重叠时走 Rule 而非 think），可选：

A. **保持 B 现状**（推荐）：语义清晰——内置检测（WebSearch/Subagent/Think/
  Background/LongContext）按固定优先级，用户 Rules 在其后是"无 thinking
  时的附加拦截"。文档需说明此语义。
B. **把 Router Rules 再前移到 Think 之前**（保留 think vs background 顺
  序但让 Rules 抢先 think）：完整链变 WebSearch(1) → Subagent(2) → Router
  Rules(3) → Think(4) → Background(5) → Prompt Rules(6) → Long Context(7)
  → Default(8)。这样 Rules 永远优先于 think，但与上游 9j 顺序不完全一致
  （9j 没 Rules，本就是扩展）。
C. **加 `request.thinking.type` 这种 condition 让用户用 Rule 显式判断
  thinking**，不依赖自动 think 检测。

倾向 A：当前 B 状态 + 守护测试已覆盖，文档 §11.6/§11.7 说清语义即可。
若有使用 Router Rules 想"覆盖 thinking"的场景再考虑 B 方案。

### 11.8 elidickinson 历史完整考古（commit 8c1b65a + c3a435c）

用户要求"elidickinson 基础上先测试（在没有 Router Rules 干扰）确认是否
正常"——通过 git worktree 在三个独立环境做纯净对照实测：

- **9j 上游纯净** (`/tmp/9j-test`): D 测试 `claude-haiku+thinking` 实测
  走 `🧠 Routing to think model (Plan Mode detected)` ✅ 与 README 一致
  (9j 设计 think 在 background 之前)
- **elidickinson 纯净** (`/tmp/eli-test`): D 测试实测走
  `🔄 Routing to background model` ❌ **回归是 elidickinson 原生引入**,
  与我们引入的 Router Rules/LongContext 完全无关
- **我们 B 实施后** (`/opt/data/home/ccm` HEAD=`2e8db1a` feat(router): add cost_first): D 测试走
  think ✅ — B 修了 elidickinson 原生回归

**elidickinson 历史考古**（commit message 原文）:

| Commit | 时间 | 设计意图 |
|---|---|---|
| `c3a435c feat(router): add prompt-based routing rules` | 2025-12-01 | elidickinson 首次引入 Prompt Rules, commit msg 原文: "Check prompt rules after subagent model but before think mode" — 主动设计 prompt rules > think |
| `8c1b65a fix: forward Anthropic rate limit headers and prioritize background routing` | 2025-12-20 | elidickinson 主动前移 background 到 priority 2, commit msg 原文: "2. Move background routing to 2nd priority (after websearch)" + "cost optimization" — 主动 cost-first 设计 |

**elidickinson 完整设计哲学**（commit 考古确认）:
- `c3a435c`: prompt rules 在 think 之前 — 主动指令 > 被动检测
- `8c1b65a`: background 在 think 之前 — cost optimization > user 显式 think
- 即: **background > prompt_rules > think** 是 elidickinson 主动设计

Regression 不是无意 bug: elidickinson 作者明确选择了 **cost-first** 而非
**think-first**。但作者没在 README 提示"claude-haiku + thinking 会走
background 而不是 think"这个 trade-off 副作用 — 间接导致用户 perspective
中的"functional regression"。

### 11.9 B 实施对 elidickinson 两个 commit 的影响

| elidickinson 主动设计 | elidickinson 原行为 | B 实施后 | 性质 |
|---|---|---|---|
| `8c1b65a` background cost-first (claude-haiku+thinking 走 background) | 走 background | **走 think** | **否定了 elidickinson cost-first 设计** (user perspective 修了 bug) |
| `c3a435c` prompt rules 优先 think (thinking+匹配规则走 rule) | 走 prompt rule model | **走 think** | **否定了 elidickinson 主动指令优先设计** |

所以 B 实施是**双向否定 elidickinson 两个主动设计 commit** — 不是简单
修复 bug, 是改变设计哲学:
- 从 elidickinson "cost-first + 主动指令-first" 回退到 9j "think-first"
  (user 显式 thinking 优先一切)

### 11.10 / 11.11 真机实证 — 拆出到独立文档

haiku vs opus 真机对照实证 + 真实模型 vs 客户端名澄清（用户要求验证）已拆分到
**`docs/routing-e2e-evidence.md` §11.10 / §11.11**，避免本设计文档压长度。
两节内容（含 4×3 environ 矩阵 + 真机 log）保持原 §11.X 编号，不动 cross-ref。

### 11.12 最终方案: `cost_first` 配置开关（用户拍板）

用户设计意图: **"默认 think-first + 设置里可选 cost-first 开关"**。

实现: `RouterConfig.cost_first: bool` (默认 `false` = think-first)。

配置语法（toml）:
```toml
[router]
default = "claude-haiku-4-5"
background = "..."
think = "..."
# 默认不写 cost_first = false (think-first, 匹配 9j 上游)

# 想恢复 elidickinson cost-first 行为:
cost_first = true
```

两种模式下 route() 优先级链:

| 模式 | 优先级链 |
|---|---|
| `cost_first=false` (默认, 9j think-first) | Auto-map → WebSearch → **Subagent → Think → Background → Router Rules → Prompt Rules → Long Context** → Default |
| `cost_first=true` (elidickinson cost-first) | Auto-map → WebSearch → **Background → Subagent → Router Rules → Prompt Rules → Think → Long Context** → Default |

架构实现: route() 提取 6 个 helper (`try_subagent`/`try_think`/`try_background`/
`try_router_rule`/`try_prompt_rule`/`try_long_context`), 按 `cost_first` 选择
顺序调度 — 无代码重复, 易审计, 易扩展。

真机实证（基于真配置 mirror3 + 同一 payload `claude-haiku-4-5 + thinking.type=enabled`）:

| 配置 | 实测路由日志 | route_type |
|---|---|---|
| `cost_first=false`（默认）| `🧠 Routing to think model (Plan Mode detected)` | think ✅ |
| `cost_first=true` | `🔄 Routing to background model` → fallback chain (glm-5/glm-5.2/dsv4-pro/...) | background |

守护测试:
- `test_think_vs_background_when_model_is_claude_haiku` — pin think-first 默认
- `test_cost_first_haiku_thinking_routes_to_background` — pin cost_first=true
  恢复 background 抢占
- `test_cost_first_prompt_rule_beats_think_when_both_present` — pin cost_first=true
  也恢复 elidickinson `c3a435c` 的 prompt-rule > think 优先
- 271 tests passed, 0 failed

### 11.13 设计哲学归属与尊重

B 实施本身就是设计观点改变, 但放弃对 elidickinson 设计的兜底 (强制 think-first)
是不尊重 fork 作者选择 — `cost_first=true` 配置恢复原名 `elidickinson` 设计
意图(`8c1b65a` cost-first + `c3a435c` prompt-rule-first), 保留向后兼容的逃生口
, 同时以 think-first 为默认推荐。

### 11.14 / 11.15 / 11.16 真机实证 — 拆出到独立文档

并发压测（280 req / 0 fail）+ 全 9 routing 分支覆盖（20 req）+ 异常/空规则/1:N fallback
（10 req，含 G1/G2 'All N mappings failed' 真机复现）三段实证已拆分到
**`docs/routing-e2e-evidence.md` §11.14 / §11.15 / §11.16**。
本文 §11.12 (cost_first 设计) + §11.13 (设计哲学) 保留，作为路由设计主干。

---

## 12. 决策记录

- ✅ ① **think-first 为默认**（§11.12）— `cost_first=false`（默认）使 think 在 background 之前，
  符合上游 9j 设计。新增 `cost_first` config 字段 + admin UI toggle + en/zh i18n + API GET/POST
  + 全量守护测试（cf=false 单元 + cf=true 单元 + cf=false 真机 + cf=true 真机 + 并发压测）。
- ✅ ② **cost-first 为可选开关**（§11.12 / §11.4）— 用户拍板：默认 keep think-first，
  想用 elidickinson 的 background-first 就开 `cost_first=true`。开关语义已二次审计 (R1+R2) 校验：UI
  roundtrip + syncToServer + 后端 GET/POST + i18n + UI 路由切换真机 3/3 全过。
- ❄️ ③ **不动 auto_map 顺序**（维持默认 `^claude-`，文档说明如何用
  `auto_map_regex = "^$"` 关掉）。`original_model` 在 auto_map 之前捕获用于 `try_background`,
  所以 auto_map 不会屏蔽 background 检测（§11.15 caveats 已记）。
