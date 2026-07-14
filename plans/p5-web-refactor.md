# P5: Web 前端重构 + 长上下文路由 + 日志查看 + i18n

> 状态：全部已实施 ✅。P5.1 长上下文路由 ✅，P5.2 SSE 日志流 ✅，P5.3 i18n（173 keys × 2 语言）✅，P5.4 日志查看器 ✅。性能/稳定性审计+修复 ✅（BufWriter, poison recovery, spawn_blocking, cached regex, autoSave 回退）。Docker 镜像 GHA 自动构建 ✅。
> 来源：用户增加 P1 级别需求（4 项）
> 创建：2026-07-10
> 最终更新：2026-07-14
> 依赖：P0.2 Token 阈值路由（已实施，`estimate_token_count` tiktoken BPE 基础设施可复用）

---

## 0. 需求分解与优先级

用户增加 4 个 P1 级别需求，本需求优先级高于同级别其他计划：

| 编号 | 需求 | 类型 | 实施顺序 |
|------|------|------|---------|
| P5.4 | 长上下文模型（自动路由） | 后端纯逻辑 + 前端配置 | **第 1 步**（功能优先） |
| P5.2 | 网页日志查看页面 | 后端 API + 前端页面 | **第 2 步** |
| P5.3 | i18n 默认简体中文 | 前端转换 | **第 3 步** |
| P5.1 | Tauri 2 前端重构 | 前端结构拆分 | **第 4 步**（功能全验证后最后做） |

**顺序理由**（用户第 4 点决策）：若风险大则分开干，先实现功能测试通过再拆 HTML 文件。
需求 1（前端重构）是结构改动有回归风险，放最后；其余 3 个功能先在现有单文件结构内实现并测试通过。

---

## 1. 现状基数

### 1.1 前端现状

| 项 | 现状 |
|---|------|
| 文件 | `src/server/admin.html`，**4120 行单体** |
| 托管 | `include_str!("admin.html")` 编译进二进制（`server/mod.rs:197`） |
| 技术栈 | HTMX 2.0.8（CDN）+ Franken UI 2.1.1（CDN）+ Tailwind CSS 4（CDN）+ 原生 JS |
| CDN 依赖 | 3 个：`unpkg.com/htmx`、`cdn.jsdelivr.net/franken-ui`、`cdn.jsdelivr.net/@tailwindcss/browser` |
| 功能 Tab | 6 个：overview / providers / models / router / settings / test |
| 路由模型配置 | `default_model` / `think_model` / `background_model` / `websearch_model`（router Tab，4 个 select） |
| 前端路由 | URL hash 参数 `?tab=xxx&view=yyy`，`showTab()` + `handleRoute()` |
| i18n | 无，全英文硬编码（约 ~200 处可翻译字符串） |

### 1.2 后端现状

| 项 | 现状 |
|---|------|
| API 端点 | `/api/config/json` (GET/POST) + `/api/reload` + oauth 系列端点 |
| 日志 | `tracing-subscriber` 输出到 stdout（无 API 暴露） |
| 消息追踪 | `message_tracing::MessageTracer` 写 JSONL 到 `~/.claude-code-mux/trace.jsonl`（`TracingConfig` 开关，默认关） |
| trace 结构 | `RequestTrace` / `ResponseTrace` / `trace_error`，含 ts/dir/id/model/provider/route_type/is_stream |
| 路由层 | **现状 6 个检查点**（WebSearch→Background→Subagent→RouterRule→PromptRule→Think→Default，其中代码注释 L175-180 头部只列 6 项且把 RouterRule 归入 "Prompt Rules"，实际 route() 代码流程有 7 个独立 return 点）；**P5.4 新增 LongContext 后变 7 层**——文档"第 7 层"是 P5 实施后目标，当前为 6 层。注释编号有重复偏差（L246 "5"/L254 "5"），是既有代码注释问题，不影响 P5 实施 |
| P0.2 基础设施 | `RouterConfig.rules[].threshold` + `AnthropicRequest.token_count` + `estimate_token_count()`（tiktoken cl100k_base，惰性计算） |

### 1.3 路由配置结构（`cli/mod.rs:117`）

```rust
pub struct RouterConfig {
    pub default: String,
    pub background: Option<String>,
    pub think: Option<String>,
    pub websearch: Option<String>,
    pub auto_map_regex: Option<String>,
    pub background_regex: Option<String>,
    pub prompt_rules: Vec<PromptRule>,
    pub rules: Vec<RouterRule>,
}
```

### 1.4 RouteType 枚举（`models/mod.rs:277`）

```rust
pub enum RouteType { WebSearch, PromptRule, Think, Background, Default }
```

---

## 2. P5.4 长上下文模型（自动路由）— 第 1 步

### 2.1 设计

复用 P0.2 已实现的 `estimate_token_count()`（tiktoken BPE 惰性计算）。新增独立路由层 `LongContext`，在 Think 之后、Default 之前触发：当请求 token 数超过阈值且配置了长上下文模型时，路由到该模型。

### 2.2 改动清单

#### 2.2.1 `src/cli/mod.rs` — RouterConfig 加 2 字段

```rust
pub struct RouterConfig {
    // ... 现有字段 ...
    /// 长上下文专用模型（请求 token 超过阈值时使用）
    pub long_context: Option<String>,
    /// 长上下文触发阈值（token 数），默认 100000
    #[serde(default = "default_long_context_threshold")]
    pub long_context_threshold: Option<u32>,
}

fn default_long_context_threshold() -> Option<u32> { Some(100_000) }
```

- `Option<String>` + `Option<u32>` + `#[serde(default)]` 保持旧配置兼容
- `long_context` 未配置时该路由层跳过（同 `background`/`think` 模式）

#### 2.2.2 `src/models/mod.rs` — RouteType 加变体

```rust
pub enum RouteType {
    WebSearch,
    PromptRule,
    Think,
    LongContext,  // 新增
    Background,
    Default,
}

impl std::fmt::Display for RouteType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // ... 现有 ...
            RouteType::LongContext => write!(f, "long-context"),
        }
    }
}
```

- 注意：`Display` impl 是穷举 match，新增变体必须加 arm 否则编译错误（编译器强制覆盖）

#### 2.2.3 `src/router/mod.rs` — 新增第 7 层路由

执行位置：在 Think 之后（L265）、Default 之前（L267）。

```rust
// 6. Long Context (token count exceeds threshold)
if let Some(ref long_context_model) = self.config.router.long_context {
    let threshold = self.config.router.long_context_threshold.unwrap_or(100_000);
    // 惰性计算 token_count（复用 P0.2 estimate_token_count）
    if request.token_count.is_none() {
        request.token_count = Some(self.estimate_token_count(request));
    }
    let token_count = request.token_count.unwrap_or(0);
    if token_count >= threshold {
        debug!("📏 Routing to long-context model ({} tokens >= {})", token_count, threshold);
        return Ok(RouteDecision {
            model_name: long_context_model.clone(),
            route_type: RouteType::LongContext,
            matched_prompt: None,
        });
    }
}
```

优先级注释更新（L174-180）：
```
// 1. WebSearch
// 2. Background
// 3. Subagent
// 4. Router Rules
// 5. Prompt Rules
// 6. Think
// 7. LongContext - token count exceeds threshold  ← 新增
// 8. Default
```

#### 2.2.4 `src/server/admin.html` — router Tab 加配置 UI

在 `websearch_model` select 之后（L1243 之后）加：

```html
<div>
    <label class="block text-sm font-medium text-gray-700 mb-2">长上下文模型</label>
    <select name="long_context_model" class="input-field">
        <option value="">不使用</option>
        <!-- 动态填充模型选项 -->
    </select>
    <p class="text-xs text-gray-500 mt-1">当请求 token 数超过阈值时使用此模型</p>
</div>
<div>
    <label class="block text-sm font-medium text-gray-700 mb-2">长上下文阈值（token）</label>
    <input type="number" name="long_context_threshold" class="input-field" placeholder="100000" min="1000">
</div>
```

`loadRouterTab()` JS（L1865）加：
```js
const longContextSelect = document.querySelector('[name="long_context_model"]');
if (longContextSelect) longContextSelect.value = config.router.long_context || "";
const thresholdInput = document.querySelector('[name="long_context_threshold"]');
if (thresholdInput) thresholdInput.value = config.router.long_context_threshold || "100000";
```

`syncConfigToUI` / `collectConfig` 也要加对应字段收集。

#### 2.2.5 测试

`src/router/mod.rs` 加 3 单元测试：
- `test_long_context_triggers`: token_count >= threshold → LongContext
- `test_long_context_below_threshold`: token_count < threshold → Default
- `test_long_context_not_configured`: long_context=None → 跳过层

### 2.3 风险评估

| 风险 | 严重度 | 说明 |
|------|--------|------|
| 复用已验证的 estimate_token_count | 🟢 低 | P0.2 已测试通过 |
| 新增 RouteType 变体编译强制覆盖 | 🟢 低 | match 穷举会编译报错暴露遗漏 |
| 新增路由层优先级 | 🟡 中 | 放 Think 后 Default 前——长上下文优先级高于 Default |
| token 计算性能 | 🟢 低 | 惰性，仅在配置了 long_context 且 token_count 未计算时触发 |
| 与 P0.2 RouterRule.threshold 关系 | 🟢 无冲突 | RouterRule 是规则内阈值，LongContext 是全局路由层 |

**风险 < 收益**，可实施。

### 2.4 兼容性

- 旧配置无 `long_context` 字段 → `Option` + `#[serde(default)]` → 跳过该层，行为不变
- `RouteType` 新增变体 → 穷举 match 编译强制覆盖 → 无遗漏风险

---

## 3. P5.2 日志查看页面 — 第 2 步

### 3.1 设计

日志源复用已有 `message_tracing::MessageTracer` 写的 `trace.jsonl` 文件，不引入新依赖。后端新增 `/api/logs` 系列端点读取该文件并支持分页 + 过滤 + SSE 实时流。

### 3.2 日志源决策

| 候选 | 评估 | 选择 |
|------|------|------|
| stdout 环形内存缓冲 | 实时但 volatile，重启丢失，占内存 | ❌ |
| **trace.jsonl 文件** | 已有 MessageTracer 写入，持久，零新增依赖 | ✅ |
| tracing-subscriber file appender | 需引 rolling file 依赖 | ❌ |

### 3.3 改动清单

#### 3.3.1 后端 API — `src/server/mod.rs` 加 3 端点

```rust
.route("/api/logs", get(get_logs))           // 分页 + 过滤
.route("/api/logs/stream", get(stream_logs)) // SSE 实时流
.route("/api/logs/config", get(get_logs_config)) // tracing 配置查询
```

#### 3.3.2 `src/message_tracing/mod.rs` — 加查询方法

新增只读方法（不改动现有 trace_request/trace_response）：

```rust
impl MessageTracer {
    /// 读取 trace.jsonl 尾部 N 条记录（逆序）
    pub fn read_recent(&self, limit: usize, offset: usize, filter: LogFilter) -> Result<Vec<LogEntry>> { ... }
    
    /// 监听文件新增行（供 SSE 用，基于 notify/inotify 或轮询）
    pub fn watch_new(&self) -> impl Stream<Item = LogEntry> { ... }
}

#[derive(Deserialize)]
pub struct LogFilter {
    pub route_type: Option<String>,
    pub model: Option<String>,
    pub since: Option<DateTime<Utc>>,
    pub level: Option<String>,  // info/error 等
}

#[derive(Serialize)]
pub struct LogEntry {
    // 精简 API 视图——不含 RequestTrace 的完整 messages 体（隐私+体积）
    // 现有 RequestTrace 另有 tool_count/messages 2 个字段，LogEntry 不暴露
    pub ts: DateTime<Utc>,
    pub dir: String,
    pub id: String,
    pub model: String,
    pub provider: String,
    pub route_type: String,
    pub is_stream: bool,
    pub error: Option<String>,
}
```

#### 3.3.3 SSE 实时流端点

> **实现说明**（2026-07-11 审计修正）：下方示例使用 `tracer.watch_new()` 概念 API，实际实现因 axum 0.7 SSE 对 `unfold` stream poll 时序问题（只发 keep-alive 不发 data），改为 `tokio::spawn` task + `mpsc::channel` + `ReceiverStream` 方案。详见 `src/server/logs.rs` `stream_logs()` 函数（commit 64643c7）。

用已有依赖构造流（不引 async_stream），示例用 `tokio::sync::watch` channel + `futures::stream`：

```rust
// 示例（概念）—— 实际实现见 src/server/logs.rs stream_logs()
use axum::response::sse::{Sse, Event, KeepAlive};

async fn stream_logs(State(state): State<AppState>) -> Sse<impl futures::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let tracer = state.message_tracer.clone();
    // watch_new 返回 impl Stream，内部用 tokio interval 轮询文件 mtime + 读 append 行
    let event_stream = tracer.watch_new().map(|entry| {
        let json = serde_json::to_string(&entry).unwrap();
        Ok(Event::default().data(json))
    });
    Sse::new(event_stream).keep_alive(KeepAlive::default())
}
```

- SSE 依赖：axum 已含 `Sse` 响应类型（`axum::response::sse::{Sse, Event, KeepAlive}`，默认 feature 就含，无需新增 crate 或 feature flag）
- `watch_new()` 实现：轮询文件 mtime + 读取 append 行（轻量，不引 notify 依赖），返回 `impl futures::Stream`（**注意**：此为计划阶段概念 API，实际实现用 mpsc channel 替代，见上方实现说明）
- 流构造用已有依赖：`futures = "0.3"` + `tokio-stream = "0.1"`（Cargo.toml L24/L23），用 `futures::stream::unfold` 或 `tokio::sync::watch` channel 构造，**不引 async_stream crate**

#### 3.3.4 前端 — 新增 logs Tab

在 `handleRoute()` 和 tab 导航加 logs Tab（临时在单文件结构内实现，P5.1 重构时归位到独立模块）。

前端特性：
- 实时流表格（SSE 连接 `/api/logs/stream`）
- 过滤栏（route_type / model / 时间范围）
- 分页（默认 50 条/页）
- 暂停/继续按钮
- tracing 开关提示（若 tracing 关闭则提示用户在 settings 开启）

#### 3.3.5 tracing 默认开启问题

**已拍板（§8 Q4）：方案 A——不改默认值。** `TracingConfig::default().enabled = false`（cli/mod.rs:49）保持不变。前端 logs Tab 检测 tracing 配置，未开启时显示提示"tracing 未开启，前往 settings 开启"。理由：trace.jsonl 含完整消息体，默认开启有隐私 + 磁盘增长风险，用户主动选择更安全。

### 3.4 风险评估

| 风险 | 严重度 | 说明 |
|------|--------|------|
| trace.jsonl 大文件分页 | 🟡 中 | tail + offset 读取，限制单次 max 500 条 |
| SSE 长连接资源 | 🟡 中 | keep-alive + 超时断开，限制并发连接数 |
| watch_new 轮询性能 | 🟡 中 | 1s 间隔 mtime 检查，轻量 |
| async_stream 依赖 | 🟢 低 | 检查是否已有，可用原生替代 |
| tracing 默认关闭 | 🟢 低 | 前端提示，不改默认值 |

**风险 < 收益**，可实施。

---

## 4. P5.3 i18n（默认简体中文）— 第 3 步

### 4.1 设计

轻量 JSON 词条 + `t(key)` 函数，无框架依赖（不引 i18next）。默认 `zh-CN`，`en` 可选。

> **已拍板（§8 Q1）：en.json 需要**——P5.3 同时提供 `zh-CN.json`（默认）和 `en.json`。
>
> **已拍板（2026-07-11）：完整覆盖 ~200 词条，但专有名词保留原文不翻译**：
> - 保留原文的：token（不翻"词元"）、router 路由（非"路由器"）、openai/openrouter/anthropic/deepseek 等品牌名、API/JSON/TOML/URL/SSE/OAuth 等技术缩写、provider type 名（openai/anthropic/openrouter 等）
> - 翻译的：UI 标签、按钮、提示、标题、说明性文字
> - 仅对"翻译不准确或会误导"的词保留原文

### 4.2 改动清单

#### 4.2.1 VG 语言文件

```
src/server/i18n/             # P5.3 阶段位置（P5.1 重构后迁移到 frontend/i18n/）
├── zh-CN.json   # 默认，~200 词条
└── en.json      # 英文（已拍板需要）
```

词条结构：
```json
{
  "tab.overview": "概览",
  "tab.providers": "提供商",
  "tab.models": "模型",
  "tab.router": "路由",
  "tab.settings": "设置",
  "tab.logs": "日志",
  "button.save": "保存",
  "button.cancel": "取消",
  "router.default_model": "默认模型",
  "router.think_model": "思考模型",
  "router.long_context_model": "长上下文模型",
  "router.long_context_threshold": "长上下文阈值（token）"
}
```

#### 4.2.2 i18n 模块

```js
// js/i18n.js (P5.1 重构后) 或 admin.html 内 (P5.3 临时)
const locale = localStorage.getItem('ccm_locale') || 'zh-CN';
const messages = {}; // 启动时 fetch 加载

async function loadLocale(loc) {
  const res = await fetch(`/api/i18n/${loc}`);
  messages[loc] = await res.json();
}

function t(key, ...args) {
  let s = messages[locale]?.[key] ?? messages['zh-CN']?.[key] ?? key;
  // 简单插值 {0} {1}
  args.forEach((a, i) => s = s.replace(`{${i}}`, a));
  return s;
}
```

#### 4.2.3 后端 — `/api/i18n/:locale` 端点

```rust
.route("/api/i18n/:locale", get(get_i18n_dict))
```

`include_str!` 编译进词条，或 `include_dir!` 在 P5.1 后自动包含。

#### 4.2.4 DOM 替换策略

所有可翻译文本加 `data-i18n="key"` 属性，启动时遍历替换：
```html
<h2 data-i18n="tab.router">Router</h2>
```
```js
document.querySelectorAll('[data-i18n]').forEach(el => {
  el.textContent = t(el.getAttribute('data-i18n'));
});
```

#### 4.2.5 settings Tab 加语言选择

同 `long_context_model` 模式，加 `locale` select，存 localStorage。

### 4.3 风险评估

| 风险 | 严重度 | 说明 |
|------|--------|------|
| ~200 处字符串逐个加 data-i18n | 🟡 中 | 工作量大但机械式 |
| 默认 zh-CN | 🟢 低 | localStorage 切换 |
| 动态文本插值 | 🟢 低 | 简单 {0}{1} 替换 |
| HTMX 动态内容 i18n | 🟡 中 | htmx:afterRequest 钩子需重新渲染翻译 |

**风险 < 收益**，可实施。建议 P5.1 拆分后做更顺（模块化后 data-i18n 标注更清晰），但当前顺序是 3 在 1 之前——作为验证方案，先在单文件内做 i18n 骨架 + 关键词条，P5.1 重构时补全全部词条。

---

## 5. P5.1 前端重构（Tauri 2）— 第 4 步（最后）

### 5.1 设计澄清

用户指示"Tauri 2 重构 web 为主，不是 GUI 客户端"+ "轻量不要 Node 重依赖"。

**结论**：Tauri 本身是桌面 GUI 框架（WebView + Rust 后端），与"web 为主"目标矛盾。用户的实际诉求是**轻量地拆分 4120 行单文件为模块化多文件，仍由 Rust 服务器托管**。不引入 Tauri，不引入 Node/Vite/esbuild。

### 5.2 技术决策（已与用户确认）

| 项 | 决策 |
|---|------|
| 构建工具 | **无**——不引入 Vite/esbuild/Node，纯 Rust 项目保持纯净 |
| 技术栈 | **沿用** HTMX 2.0.8 + Franken UI + Tailwind 4 + 原生 JS |
| 拆分方式 | ES Modules（浏览器原生 `<script type="module">` + import/export） |
| 嵌入方式 | `include_dir = "0.7"` crate + build.rs 自动包含 `frontend/` 目录 |
| CDN | **本地化**——htmx/franken-ui/tailwind 下载为本地静态文件放 `assets/vendor/` |

### 5.3 目录结构

```
src/server/frontend/
├── index.html              # 骨架（~100 行）：meta + script module 入口 + div#app
├── css/
│   ├── main.css            # 自定义样式（从 admin.html <style> 提取）
│   └── vendor/
│       ├── franken-ui.min.css    # 本地化的 Franken UI
│       ├── tailwind-browser.js   # 本地化的 Tailwind 4 浏览器版
│       └── htmx.min.js           # 本地化的 HTMX 2.0.8
├── js/
│   ├── main.js             # 入口模块（初始化 + 路由）
│   ├── i18n.js             # i18n 模块（P5.3 产物归位）
│   ├── log-viewer.js       # 日志查看模块（P5.2 产物归位）
│   └── modules/
│       ├── overview.js     # overview Tab 逻辑
│       ├── providers.js    # providers Tab 逻辑
│       ├── models.js       # models Tab 逻辑
│       ├── router.js       # router Tab 逻辑（含 P5.4 long_context 配置）
│       ├── settings.js     # settings Tab 逻辑
│       └── test.js         # test Tab 逻辑
├── i18n/                   # P5.3 产物归位位置
│   ├── zh-CN.json
│   └── en.json
└── assets/
    └── vendor/             # 同 cdn/vendor 或统一放此
```

### 5.4 嵌入方式 — include_dir + build.rs

`Cargo.toml`：
```toml
[dependencies]
include_dir = "0.7"
```

`src/server/mod.rs`：
```rust
use include_dir::{include_dir, Dir};
use axum::response::Response;

static FRONTEND_DIR: Dir<'_> = include_dir!("src/server/frontend");

async fn serve_frontend(Path(path): Path<String>) -> Response {
    // 按 path 查找 FRONTEND_DIR.get_file(path)
    // content-type 根据 extension 设置
    // index.html 回退
}
```

路由注册：
```rust
.route("/", get(serve_admin))                    // → serve_frontend("index.html")
.nest_service("/static", get(serve_frontend))    // → /static/js/main.js 等
```

### 5.5 CDN 本地化

下载 3 个资源到 `frontend/assets/vendor/`（通过 SOCKS5 代理）：
- `htmx.min.js`（~14KB）from `unpkg.com/htmx.org@2.0.8`
- `franken-ui.min.css`（~?KB）from `cdn.jsdelivr.net/franken-ui@2.1.1/dist/css/core.min.css`
- `tailwind-browser.js`（~?KB）from `cdn.jsdelivr.net/@tailwindcss/browser@4`

`index.html` 改为本地路径：
```html
<script src="/static/assets/vendor/htmx.min.js"></script>
```

体积影响：二进制增~300KB（可接受，换取离线可用 + 无追踪）。

### 5.6 改动清单

1. `Cargo.toml` 加 `include_dir = "0.7"`
2. 创建 `src/server/frontend/` 目录结构
3. 下载 3 个 CDN 资源到 `assets/vendor/`
4. 拆 `admin.html` 4120 行 → index.html 骨架 + 各模块 JS
5. 拆 `<style>` → `css/main.css`
6. 拆 JS 函数（`loadConfig`/`showTab`/`loadRouterTab` 等约 ~50 个函数）→ 各 Tab 模块
7. P5.2/P5.3 产物归位到 `js/log-viewer.js` + `js/i18n.js` + `i18n/*.json`
8. `server/mod.rs` 加 `serve_frontend` + `include_dir!`
9. 移除旧 `include_str!("admin.html")`

### 5.7 风险评估

| 风险 | 严重度 | 说明 |
|------|--------|------|
| 4120 行拆分回归 | 🟡 中 | 逐 Tab 拆分 + 全量功能测试对比 |
| ES Modules 浏览器兼容 | 🟢 低 | 现代浏览器原生支持 |
| include_dir build.rs 编译时间 | 🟢 低 | 仅编译时包含静态文件 |
| CDN 本地化路径 | 🟢 低 | 静态 route nest_service |
| HTMX 动态绑定时序 | 🟡 中 | htmx 需在模块加载前就绪 |

**风险中等但可控**——因放在最后且 P5.2/P5.3/P5.4 已验证功能，拆分后可逐 Tab 回归测试。

---

## 6. 整体实施顺序与验证

### 6.1 实施顺序

```
P5.4 长上下文路由（后端+前端配置+测试）
  ↓ cargo test 通过
P5.2 日志查看页（API+SSE+前端临时实现）
  ↓ cargo test 通过 + 手测 SSE
P5.3 i18n 骨架（zh-CN 词条+data-i18n 标注关键路径）
  ↓ 验证 t() 函数 + 语言切换
P5.1 前端重构（拆分+include_dir+CDN本地化+产物归位）
  ↓ 全量回归测试
```

### 6.2 验证标准（每步）

1. 本地 `cargo check --tests` + `cargo test` 全通过
2. `cargo clippy --no-deps` 0 errors
3. 无敏感数据提交 git / 硬编码
4. 文档同步更新本计划文档状态
5. 二进制远程构建（若需要）git push 后取回验证

### 6.3 测试矩阵

| 子项 | 新增测试数 | 验证方式 |
|------|-----------|---------|
| P5.4 | 3 单元测试（新增） | cargo test long_context（现有 4 个 threshold 测试可复用模式） |
| P5.2 | 后端 API 测试 + 手测 SSE | curl /api/logs + 浏览器 EventSource |
| P5.3 | t() 函数测试 | 前端 console 验证 |
| P5.1 | 回归测试 | 6 Tab 全功能对比重构前 |

---

## 7. 风险/收益总结

| 子项 | 风险 | 收益 | 判定 |
|------|------|------|------|
| P5.4 长上下文路由 | 🟢 低（复用 P0.2） | 高（长上下文请求自动用大窗口模型） | 收益 > 风险 ✅ |
| P5.2 日志查看+SSE | 🟡 中（文件分页+SSE） | 高（运维可见性） | 收益 > 风险 ✅ |
| P5.3 i18n | 🟡 中（工作量） | 中（中文用户友好） | 收益 > 风险 ✅ |
| P5.1 前端重构 | 🟡 中（回归） | 中（可维护性） | 收益 > 风险 ✅（放最后控回归） |

**全部可实施**，按 4→2→3→1 顺序推进，每步独立验证测试通过后再进下一步。

---

## 8. 开放问题（已全部拍板 2026-07-10）

| # | 问题 | 拍板结果 |
|---|------|---------|
| 1 | en.json 是否需要 | ✅ **需要**——P5.3 提供 en 词条文件 |
| 2 | long_context_threshold 默认值 | ✅ **100000**——合理 |
| 3 | P5.2 async_stream / KeepAlive 依赖 | ✅ **零新增依赖**——axum SSE (`Sse`/`Event`/`KeepAlive`) 在 axum 核心模块默认 feature；`futures = "0.3"` (L24) + `tokio-stream = "0.1"` (L23) 已有；不引 async_stream，用 `futures::stream` 原生构造（`stream::unfold` 或 channel）替代 `stream!{}` 宏 |
| 4 | tracing 默认开关 | ✅ **方案 A：不改默认值 (`enabled: false`)**，前端 logs Tab 检测 tracing 配置，未开启时提示"tracing 未开启，前往 settings 开启"。理由：trace.jsonl 含完整消息体，默认开启有隐私 + 磁盘增长风险 |
| 5 | CDN 资源本地化下载方式 | ✅ **SOCKS5 代理 192.168.8.3:7890 下载** + **`include_dir!` 编译进二进制**（保持单二进制部署特性，与当前 `include_str!("admin.html")` 一致，体积 +~300KB 可接受） |

---

> 本计划文档遵循 CCM plans/ 目录 markdown 格式（与 P0-P4 一致），不走 rapidspec 流程。
