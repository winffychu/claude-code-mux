# Routing E2E Evidence — cost_first 双模式真机实证闭环

> 本文档从 `think-routing.md` §11.10 / §11.11 / §11.14-11.16 实证链拆分而来（2026-07-18 D 拆分计划）。
> 保留原 `§11.X` 子号以与 `think-routing.md` §11 主体框架互引。
> 设计文档主干见 `think-routing.md`；本文只承载真机端到端实证数据与 server log 证据。

---

## §11 真机实证链（导出节）

### 11.10 haiku vs opus 真机对照实证（用户要求验证）

用户要求"用 haiku 和 opus 测试就知道了"。通过 git worktree 在 3 个独立
编译环境跑 4 个 payload × 3 binary 实测:

| 测试 | payload | **elidickinson** | **9j upstream** | **我们 B 后** |
|---|---|---|---|---|
| A | haiku + thinking | `🔄 background` ❌ | `🧠 think` ✅ | `🧠 think` ✅ |
| B | haiku 无 thinking | `🔄 background` ✅ | `🔄 background` ✅ | `🔄 background` ✅ |
| C | opus + thinking | `🧠 think` ✅ | `🧠 think` ✅ | `🧠 think` ✅ |
| D | opus 无 thinking | `✅ default(claude-haiku-4-5)` | 同 | 同 |

**核心差异暴露**:

- **A (haiku + thinking)**: 3 环境行为差异显著
  - elidickinson: background 命中（cost-first 副作用, 破坏 user 显式 thinking）
  - 9j / B 后: think 命中（think-first, 尊重 user 显式 thinking）
- **B (haiku 无 thinking)**: 3 环境一致走 background（无 thinking 优先级冲突）
- **C (opus + thinking)**: 3 环境一致走 think（opus 不命中 `(?i)claude.*haiku`,
  background 不触发）
- **D (opus 无 thinking)**: 3 环境一致走 default fallback

**haiku vs opus 差异本质**: `(?i)claude.*haiku` 默认背景正则**仅影响
haiku-family client 名**。haiku 通常是 cost-sensitive 客户端发 model,
elidickinson 作者意图"claude-haiku 通常用作 cheap background tasks 应走
background 而非 think" — `cost optimization` 设计本意。

但作者忽略:用户也可能用 claude-haiku 显式开 thinking 想 think — 这种场景
cost-first 与 user intent 冲突。

**当前真配置 (mirror3) 下影响分析**:

- `default=background=think = "claude-haiku-4-5"` (同 model name)
- A 测试无论走 background 还是 think, 最终都走 `claude-haiku-4-5` 这个 model
  的同一 fallback chain → 真实上游相同 (glm-5 / dsv4-pro / agnes-2.0-flash)
- **B 实施在当前配置下仅修正 `/api/logs` 的 route_type 标签语义, 无实际
  model 转向差异**
- 若配置改 `think="claude-opus-4-5"` 等**不同 model name**, A 测试走 think
  时真实上游 = claude-opus, 走 background 时仍 = glm-5 — 此时才有真实
  functional 差异

### 11.11 用户提的"真实模型 vs 客户端名"澄清

用户关心: "haiku-4-5 必定走 background 但真实模型是 dsv4flash, 那么这里
和你说的 glm 模型导致不走 think 冲突"。

源码审计确认:
- `is_background_task(&original_model)` 检查的是 **client 原始 model 名**
  (L196 保存, 在 auto_map L203 改 request.model 之前)
- **不是** auto-mapped 后的 default model
- **不是** `[[models]] name=...` fallback chain 选中的真实上游 actual_model
- README 明确写: "Background detection checks the ORIGINAL model name
  (before auto-mapping)"

我们引入 Router Rules 的目的（commit `a5b66ad`, 作者 Hermes Agent）:
- **声明式条件路由** (vs prompt regex 更结构化)
- 设计意图: 按 `request.body.model` 等 request 字段做条件分流 + `rewrite`
  改 model 字段
- commit 明确: "step 4 in route() fallthrough (after Subagent, before
  Prompt Rules)" — **模仿 elidickinson Prompt Rules "优先 think" 哲学**
  把 Router Rules 放在 think 之前的位 4

### 11.14 并发压力测试（真实 LLM 端到端，2026-07-18）

**测试目标**：在两种 `cost_first` 模式下，验证 CCM 路由层 + OpenAI provider 转换
层在并发压力下的稳定性、吞吐和路由分布正确性（之前 `§11.10` 矩阵只跑单 request
serial）。

#### 真实 LLM 上游
- Provider: `nvidia`（`provider_type = "openai"`）
- 上游 endpoint: `http://172.168.0.82:3001/proxy/nv/v1/chat/completions`
- 真实模型: `meta/llama-3.1-8b-instruct`
- 入口: CCM `/v1/messages`（Anthropic 格式）→ router → Anthropic→OpenAI transform → 上游
  LLM → OpenAI→Anthropic transform → 客户端
- `default-model` / `background-model` / `think-model` 全部映射到同一个 `llama-3.1-8b-instruct`
  真实模型，以排除上游模型差异，专注压测 ccm 自身路由 + 转换层。

#### 负载与 payload 组合
4 种 payload 轮询，严格按 §11.10 haiku/opus × thinking/no-thinking 矩阵：
- T1 `claude-haiku-4-5` + `thinking.type=enabled`
- T2 `claude-haiku-4-5`（无 think）
- T3 `claude-opus-4-1` + `thinking.type=enabled`
- T4 `claude-opus-4-1`（无 think）

每轮 4 种 payload 各占 1/4，并发度递增（cf=false: 10 → 30 → 50；cf=true: 10 → 20 → 40），每档跑 40~80 req。

#### `cost_first = false`（默认 think-first）

| 并发 | n_req | 吞吐 | 成功 | avg   | p50  | p95  | p99  | max  |
|------|-------|------|------|-------|------|------|------|------|
| 10   | 40    | 9.8 req/s  | 40/40 | 0.90s | 0.75s | 1.73s | 2.38s | 2.38s |
| 30   | 40    | 20.6 req/s | 40/40 | 0.75s | 0.72s | 1.17s | 1.45s | 1.45s |
| 50   | 80    | 38.5 req/s | 80/80 | 0.75s | 0.76s | 1.06s | 1.30s | 1.30s |

server `[:sync]` 路由分布按入站 model 反推（160 个决策行）：
- `claude-haiku-4-5` → `background` × 40（= T2，无 think）
- `claude-haiku-4-5` → `think`      × 40（= T1，haiku+thinking → 关键差异点）
- `claude-opus-4-1`  → `think`      × 40（= T3）
- `claude-opus-4-1`  → `default`    × 40（= T4）

#### `cost_first = true`（cost-first）

| 并发 | n_req | 吞吐 | 成功 | avg   | p50  | p95  | max  |
|------|-------|------|------|-------|------|------|------|
| 10   | 40    | 9.0 req/s  | 40/40 | 0.98s | 0.70s | 2.43s | 2.43s |
| 20   | 40    | 23.8 req/s | 40/40 | 0.65s | 0.66s | 0.92s | 0.93s |
| 40   | 40    | 23.1 req/s | 40/40 | 0.84s | 0.74s | 1.72s | 1.72s |

server `[:sync]` 路由分布（120 个决策行）：
- `claude-haiku-4-5` → `background` × 60（= T1 + T2 全走 background，**与 cf=false 关键差异**）
- `claude-opus-4-1`  → `think`       × 30（= T3）
- `claude-opus-4-1`  → `default`     × 30（= T4）

#### 结论
- 失败率: 0/280 (0%)，**无任何 HTTP 错误 / panic / deadlock warning**。
- 高压迫下吞吐峰值 ~38.5 req/s，p99 ≤ 1.30s — 路由 + Anthropic↔OpenAI 双向转换
  层无并发瓶颈 (延迟主导是上游 NVIDIA proxy)。
- **路由分布完全符合两种模式的理论预期**:
  - `cf=false`: T1 (haiku+thinking) → **think** (think-first 默认, 用户显式思考优先于成本优化)
  - `cf=true`: T1 (haiku+thinking) → **background** (cost-first, elidickinson 设计, 背景检测抢占 thinking 请求)
  - 两种模式下 T2/T3/T4 路由一致(见上两表)
- 并发场景下 `RwLock` (provider registry / state) 与 `tokio::fs::write`
  (admin UI 同步路径)未观察到锁竞争异常(`update_config_json` 仍走非阻塞 async IO)。

### 11.15 全 9 routing 分支真机端到端覆盖（2026-07-18）

**测试目标**：`§11.14` 压测只在 payload 短/无 tool/无 tag 下覆盖了
`background` / `think` / `default` 三条线；`long_context` 之外的 web-search /
subagent / router-rule / prompt-rule / auto_map 分支从未真机触发。本节真机覆盖
**全部 9 个 routing 分支** × `cost_first` 双模式，逐一对照 `route() [tag :sync]`
日志与理论上路由顺序。

#### 真实 LLM 上游
- Provider: `nvidia` (`provider_type = "openai"`)
- 上游: `http://172.168.0.82:3001/proxy/nv/v1/chat/completions`
- 真实模型: `meta/llama-3.1-8b-instruct`
- 入口: CCM `/v1/messages` (Anthropic 格式) → router → Anthropic→OpenAI transform → 上游 → OpenAI→Anthropic

#### 触发配置（`all-branches-{false,true}.toml`）
- `long_context = "longctx-model"` + `long_context_threshold = 50`
- `websearch = "websearch-model"`
- `background_regex` = 默认 `^claude-haiku` (匹配 haiku 入站)
- `prompt_rules`: `^translate:` → `prompt-rule-model`  (`strip_match = false`)
- `router.rules`: `{ type = "model-prefix", prefix = "rollout-", model = "router-rule-model" }`
- `auto_map_regex` = 默认 `^claude-` (把入站 `claude-X` 改为 default 后再走规则)
- 7 个独立 `[[models]]` 全部映射到同一上游真实模型
  (`meta/llama-3.1-8b-instruct`)，以排除上游差异，专注 ccm 路由本身

#### `cost_first = false`（默认 think-first）

| case | 入站 payload | server `[:sync]` tag | 命中分支 |
|---|---|---|---|
| B1 | `claude-opus-4-1` + `tools=[web_search]` | `[web-search]` | web-search (优先级1) |
| B2 | `claude-opus-4-1` + `<CCM-SUBAGENT-MODEL>worker-agent</>` in system[1] | `[default]` (RouteType::Default) | subagent (tag) |
| B3 | `claude-opus-4-1` + `thinking.type=enabled` | `[think]` | think (plan-mode) |
| B4 | `claude-haiku-4-5` 无 think | `[background]` | background (haiku regex) |
| B5 | `rollout-some-model` (model-prefix 命中) | `[prompt-rule]` (复用 tag) | router-rule |
| B6 | `claude-opus-4-1` + user `translate: hi` | `[prompt-rule:translate:]` | prompt-rule |
| B7 | `claude-opus-4-1` + 长 system (~350 tokens) | `[long-context]` | long-context |
| B8 | `claude-foo-x` (auto_map 改成 `default-model`) | `[default]` | auto_map → default |
| B9 | `claude-opus-4-1` 短 prompt 无 tool 无 tag | `[default]` | default fallback |

时序日志 (cf=false):
```
14:07:22  [web-search]    claude-opus-4-1   → nvidia/llama-3.1-8b-instruct
14:07:23  [default]       claude-opus-4-1   → nvidia/llama-3.1-8b-instruct   (B2 subagent)
14:07:26  [think]         claude-opus-4-1   → nvidia/llama-3.1-8b-instruct
14:07:28  [background]    claude-haiku-4-5  → nvidia/llama-3.1-8b-instruct
14:07:28  [prompt-rule]   rollout-some-model→ nvidia/llama-3.1-8b-instruct   (B5 router_rule)
14:07:29  [prompt-rule:translate:] opus     → nvidia/llama-3.1-8b-instruct
14:07:30  [long-context]  claude-opus-4-1   → nvidia/llama-3.1-8b-instruct
14:07:32  [default]       claude-foo-x      → nvidia/llama-3.1-8b-instruct   (B8 auto_map)
14:07:33  [default]       claude-opus-4-1   → nvidia/llama-3.1-8b-instruct
```

理论 `cf=false` 顺序对照（router L222-232 doc）：
Subagent → Think → Background → Router Rules → Prompt Rules → Long Context → Default
（WebSearch 优先级 1 在最前，AutoMap 是 step 0 的模型名变换）— **9 个 case 全一致**。

#### `cost_first = true`（cost-first）

cf=true 顺序：WebSearch → Background → Subagent → Router Rules → Prompt Rules →
Think → Long Context → Default（关键差异：Background 在 Subagent / Think 之前）。

| case | 入站 payload | server `[:sync]` tag | 命中分支 | cf=false 对照 |
|---|---|---|---|---|
| C1 | opus + web_search tool | `[web-search]` | web-search | 同 |
| C2 | opus + subagent tag | `[default]` | subagent (tag) | 同 (background 不匹配 opus) |
| C3 | **haiku + subagent tag** | `[background]` | background | **差异** — cf=false 走 subagent, cf=true background 抢先 ✓ |
| C4 | **haiku + thinking** | `[background]` | background | **差异** — cf=false 走 think, cf=true background 抢先 ✓ |
| C5 | opus + thinking | `[think]` | think | 同 (background 不匹配 opus) |
| C6 | haiku 无 think | `[background]` | background | 同 |
| C7 | `rollout-some-model` | `[prompt-rule]` | router-rule | 同 |
| C8 | opus + `translate: hi` | `[prompt-rule:translate:]` | prompt-rule | 同 |
| C9 | 长 system opus | `[long-context]` | long-context | 同 |
| C10 | `claude-foo-x` (auto_map) | `[default]` | auto_map → default | 同 |
| C11 | opus 短 prompt | `[default]` | default | 同 |

时序日志 (cf=true):
```
14:10:48  [web-search]            opus              → nvidia/llama-3.1-8b-instruct
14:10:50  [default]               opus              → nvidia/llama-3.1-8b-instruct   (C2 subagent)
14:10:52  [background]            haiku              → nvidia/llama-3.1-8b-instruct   (C3 关键差异)
14:10:58  [background]            haiku              → nvidia/llama-3.1-8b-instruct   (C4 关键差异)
14:10:58  [think]                 opus              → nvidia/llama-3.1-8b-instruct
14:10:59  [background]            haiku              → nvidia/llama-3.1-8b-instruct
14:10:59  [prompt-rule]           rollout-some-model→ nvidia/llama-3.1-8b-instruct   (router_rule)
14:11:00  [prompt-rule:translate:]opus              → nvidia/llama-3.1-8b-instruct
14:11:01  [long-context]          opus              → nvidia/llama-3.1-8b-instruct
14:11:02  [default]               claude-foo-x      → nvidia/llama-3.1-8b-instruct   (auto_map)
14:11:05  [default]               opus              → nvidia/llama-3.1-8b-instruct
```

#### 双模式差异点真机实证表

| payload 形态 | cf=false 命中 | cf=true 命中 | 差异根因 |
|---|---|---|---|
| haiku + thinking (B3/C4) | `think` | `background` | cf=true: background 在 think 之前 |
| haiku + subagent tag (B2/C3 不含 opus 主 case 对照, 这里是 C3 haiku) | subagent→`default` tag | `background` | cf=true: background 在 subagent 之前 |
| opus + thinking (B3/C5) | `think` | `think` | background_regex 不匹配 opus, 两条路都走 think |

> **Route layer / AutoMap 关键语义**:
> - `try_background` 用 **`original_model`** (router L196 `let original_model = request.model.clone();`
>   在 step-0 auto_map 之前拷贝)，**不被 auto_map 影响**。所以 `claude-haiku-4-5`
>   即使被 `^claude-` auto_map 匹配改成 `default-model`, background 仍基于原
>   `claude-haiku-4-5` 命中 `^claude-haiku` regex —— 这是设计 (避免 auto_map
>   屏蔽 background detection 的 background-tasks 当作默认路由)。
> - `try_subagent` / `try_router_rule` / `try_long_context` / `try_think` /
>   fallback default **都用** `request.model` (auto_map 转换后)。
> - server log `[:sync]` 行第三列 (入站 model) 显示的是 `original_model`
>   (字符串)，而实际发到 provider 的 model 字段是 `route_decision.model_name`
>   (=命中的 target model, 或 fallback 时 `request.model` 转换后值)。
> - `auto_map` 命中不直接产生 routing tag —— 它仅 step 0 变换 model name，后续
>   由 default fallback 或 background/subagent/router_rule 等按需要消费。

#### 结论
- 9 + 11 = **20 个 payload，全 9 个 routing 分支 × 2 模式真机触发**，
  失败率 0/20, **route tag 与时序完全符合两模式的理论优先级链**。
- `prompt-rule` 出现在 router-rule 命中上：这是设计
  (router 规则命中走 `RouteType::PromptRule`，见 router/mod.rs `try_router_rule` L343-347)，
  非 bug。
- `subagent` 命中后 route tag 显示为 `default` (设计：`Subagent` 命中后 `route_type` 复用
  `Default`)，时序区分靠随后 `[tag]` 标签是否有的 case 区分。
- `auto_map` 优先于所有路由层 (`step 0`)，但**不改路由顺序**：变换后的 model 走默认路由。
- 与 §11.10 单 request e2e 和 §11.14 并发压测合并，三层证据链一致：
  - §11.10 — 单 request 真机 (4 payload × 2 mode = 8):
   routing 分布符合理论；
  - §11.14 — 并发压测真实 LLM (280 req):
   并发稳定性 0 失败；
  - §11.15 — 全 9 分支真机覆盖 (20 req):
   每个分支真机触发且 tag 符合理论。

### 11.16 异常 / 空规则 / 1:N mapping fallback 真机覆盖（2026-07-18）

**测试目标**：用户问题——`router.rules` / `prompt_rules` 都留空时 ccm 是否退默认？
某模型异常（路由命中 target=未配置的 model / 上游 provider 失败）是否退默认？
本节真机覆盖这四种异常 + 空 规则场景的 ccm 实际行为。

#### 源码事实（已 grep 确认）
- 路由链 **fall through** 是设计：空 `router.rules` 数组 → `match_router_rule` 循环 0
  次 → 自动跳到下个层（prompt_rules → long_context → default）。空 `prompt_rules`
  同理。**这不算异常，是路由链 fall through 的正常设计**。
- router 命中的 target model 未在 `[[models]]` 配置 + `provider_registry` 也找不到
  → server/mod.rs `L714-755`：**`❌ No model mapping or provider found`** HTTP 502
  返回，**不退 default**（fail-fast；路由决策已下，layer fallback 不会跳回路由层）。
- 命中 model 的 mapping 在 `[[models]]` 配置了但上游 provider 调用失败
  → server/mod.rs `L610-709`：按 priority 顺序 **1:N mapping retry** (`[idx+1/N]`
  indicator)，全失败后 `❌ All N provider mappings failed` HTTP 502，
  **不退 default**（fail-fast 在同 model 的 N 个 mapping 内）。

#### 触发配置（`edge-false.toml` / `edge-true.toml`）
- 注意：`router.rules` 含一条 `prefix = "broken-"` → `model = "nonexistent-model"`，
  `nonexistent-model` **不**在 `[[models]]` 配置（用来触发 L755 fail-fast）。
- `think-model` 有 1:N mapping:
  ```toml
  [[models]]
  name = "think-model"
  mappings = [
      { priority = 1, provider = "nvidia-bad", actual_model = "meta/llama-3.1-8b-instruct" },
      { priority = 1, provider = "nvidia",    actual_model = "meta/llama-3.1-8b-instruct" }
  ]
  ```
  `nvidia-bad` 用 invalid API key, 期望其 401 → fallback 到第二条 `nvidia`。

#### `cost_first = false`（4 case 全过 / 失败=预期）

| case | 入站 | server `[:sync]` / error | 状态码 | 行为 |
|---|---|---|---|---|
| E1 | `claude-opus-4-1` 短 prompt 无 tool 无 tag | `[default]` | 200 OK | 空 rules → fall through → default ✓ |
| E2 | `broken-some-name` (prefix=`broken-` 命中) | `❌ No model mapping or provider found for model: nonexistent-model` | **HTTP 502** | router-rule 命中 target 未配置 → **fail-fast, 不退 default** |
| E3 | opus + thinking (think 命中) | `[think] opus → nvidia-bad` → `⚠️ nvidia-bad failed: 401 UNAUTHORIZED, trying next fallback` → `[think] opus → nvidia [2/2]` → `✅` | 200 OK | **1:N mapping fallback 真机触发** |
| E4 | opus + 长 system (~350 tokens) | `[long-context]` | 200 OK | 空 rules 不破坏 long_context fall through ✓ |

server log（cf=false）:
```
# E1/E2/E4 一组连跑（配置里 think-model 没有 bad mapping）
14:28:46  [default]      claude-opus-4-1   → nvidia/meta/llama-3.1-8b-instruct      # E1
14:28:51  ❌ No model mapping or provider found for model: nonexistent-model          # E2
14:28:55  [long-context] claude-opus-4-1   → nvidia/meta/llama-3.1-8b-instruct       # E4

# E3 是配 think-model mapping 增 `nvidia-bad` 后重启 server、单独跑:
14:31:00  [think]         claude-opus-4-1   → nvidia-bad/meta/llama-3.1-8b-instruct   # E3 (1/2)
14:31:00  ⚠️ Provider nvidia-bad failed: 401 - Authentication failed, trying next fallback
14:31:00  [think]         claude-opus-4-1   → nvidia/meta/llama-3.1-8b-instruct [2/2]  # E3 (2/2)
14:31:01  📊 1786ms 3tok
```

#### `cost_first = true`（4 case 全过 / 失败=预期）

| case | 入站 | server `[:sync]` / error | 状态码 | 行为 |
|---|---|---|---|---|
| F1 | `claude-opus-4-1` 短 prompt | `[default]` | 200 OK | 空 rules fall through → default ✓ |
| F2 | `broken-x` 命中 prefix | `❌ No model mapping` | **HTTP 502** | 同 E2 — fail-fast |
| F3 | opus + thinking → think 命中 | `[think] → nvidia-bad → ⚠️ 401 → nvidia [2/2] → ✅` | 200 OK | 同 E3 — 1:N fallback |
| F4 | opus + 长 system | `[long-context]` | 200 OK | 空 rules 不影响 long_context ✓ |

server log（cf=true）：
```
14:32:36  [default]       claude-opus-4-1   → nvidia/llama-3.1-8b-instruct          # F1
14:32:38  ❌ No model mapping or provider found for model: nonexistent-model          # F2
14:32:38  [think]         claude-opus-4-1   → nvidia-bad/llama-3.1-8b-instruct       # F3 (1/2)
14:32:38  ⚠️ Provider nvidia-bad failed: 401, trying next fallback
14:32:38  [think]         claude-opus-4-1   → nvidia/llama-3.1-8b-instruct    [2/2]  # F3 (2/2)
14:32:39  📊 1521ms 3tok
14:32:40  [long-context]  claude-opus-4-1   → nvidia/llama-3.1-8b-instruct          # F4
```

#### 结论
- **空 `router.rules` / `prompt_rules`**：ccm **正常 fall through** 到下个路由层，
  最终走 default (或 long_context, 取决于上下文长度)。不是异常，是设计。
- **路由命中 target=未配置 model**（含 router_rule 命中、prompt_rule model 字段、
  `subagent` tag → unregistered-model 等情况）：
  → HTTP 502 `No model mapping or provider found`，**不退 default**。
  这是 fail-fast — 路由决策已下，layer fallback 不会回退到路由层。
- **路由命中 + model 有 1:N provider mapping 但 primary 上游异常**：
  → 同 model 内 1:N retry 优先级，HTTP 200 + 上游映射第二次成功，**不退 default**。
- **若所有 N 个 mapping 全失败**:
  → HTTP 502 `All N provider mappings failed`, **不退 default**。（**已真机复现**
  — 见下方附加的 `cost_first = false/true` 各一组 `G1/G2` 测试。配置：把命中的
  model 全部 mapping 指向 invalid-key 上游 provider，触发 1:N 遍历完都 401 →
  `❌ All N mappings failed`。两模式行为一致。）
- **cf=false 与 cf=true 在所有 4 种异常场景下行为一致** (路由前两层的 fail-fast
  与 1:N fallback 都与 `cost_first` 路由顺序无关，因为 fallback 机制发生在**路由决策之后**)。
- **注意**：单 mapping model（如 `default-model` 本身配置只 1 个 nvidia provider）失败
  时 **不会退到其他 routing 分支**。例：若 `default-model` 的 single mapping provider error,
  整请求 502 — 见 server L714 `❌ All 1 provider mappings failed`. 同样 fail-fast 不跳回
  路由层的 next branch.
- 本节未覆盖的边缘：上游返回非 200 + 仍 parsed JSON 但内容异常（如部分截断
  /上游反 401 但附 HTML body）。ccm 对所有 transport-level 错都进 `trying next fallback`
  分支（见 L704-709），不依赖响应体内容语义。

#### 补充：所有 N mapping 全失败 真机复现（cf=false/true 各一组）

为补 §11.16 的"所有 N 全 fail → HTTP 502"事项的真机实证（之前文档写"留给单元测试
`test_all_mappings_failed` 覆盖"，但该单元测试**实际不存在**——所以真机补上）。

**配置**（`edge-all-fail.toml` / `edge-all-fail-true.toml`）:
```toml
[[providers]]
name = "broken1"
provider_type = "openai"
api_key = "invalidkey1"   # invalid key → 401
base_url = "${NVIDIA_BASE_URL}"
models = []

[[providers]]
name = "broken2"
provider_type = "openai"
api_key = "invalidkey2"   # invalid key → 401
base_url = "${NVIDIA_BASE_URL}"
models = []

[[models]]
name = "default-model"
mappings = [
    { priority = 1, provider = "broken1", actual_model = "meta/llama-3.1-8b-instruct" },
    { priority = 2, provider = "broken2", actual_model = "meta/llama-3.1-8b-instruct" }
]
```
入站：`claude-opus-4-1` 短 prompt → 路由命中 default（`default-model`），1:N fallback
遍历 broken1 → broken2，**全部 401 → HTTP 502 `All 2 provider mappings failed`**。

**`cost_first = false` G1 server log** (port 18841):
```
14:52:34  [default]     claude-opus-4-1   → broken1/meta/llama-3.1-8b-instruct          # (1/2)
14:52:34  ⚠️ Provider broken1 failed: 401 - UNAUTHORIZED, trying next fallback
14:52:34  [default]     claude-opus-4-1   → broken2/meta/llama-3.1-8b-instruct  [2/2]   # (2/2)
14:52:34  ⚠️ Provider broken2 failed: 401 - UNAUTHORIZED, trying next fallback
14:52:34  ❌ All provider mappings failed for model: default-model
→ HTTP 502: {"error":{"type":"error","message":"All 2 provider mappings failed for model: default-model"}}
```

**`cost_first = true` G2 server log** (port 18842, 复用同配置只改 `cost_first=true`):
```
14:54:21  [default]     claude-opus-4-1   → broken1/meta/llama-3.1-8b-instruct          # (1/2)
14:54:21  ⚠️ Provider broken1 failed: 401, trying next fallback
14:54:21  [default]     claude-opus-4-1   → broken2/meta/llama-3.1-8b-instruct  [2/2]   # (2/2)
14:54:21  ⚠️ Provider broken2 failed: 401, trying next fallback
14:54:21  ❌ All provider mappings failed for model: default-model
→ HTTP 502: {"error":{"type":"error","message":"All 2 provider mappings failed for model: default-model"}}
```

**结论**：所有 N 个 mapping 全失败 → HTTP 502 + `❌ All N provider mappings failed`
（与 server/mod.rs L714-719 完全一致），**不退 default**。cf=false 和 cf=true 行为
一致（fail-fast 仍在同 model 内，与路由顺序无关）。完全闭环 —— §11.16 现在所有
4 种异常场景全部真机复现 + log 在文档中可查。

---
---

> end of routing-e2e-evidence.md
