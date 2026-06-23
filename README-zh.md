# Claude Code Mux

[![Build](https://github.com/winffychu/claude-code-mux/workflows/Build/badge.svg)](https://github.com/winffychu/claude-code-mux/actions)
[![Latest Release](https://img.shields.io/github/v/release/winffychu/claude-code-mux)](https://github.com/winffychu/claude-code-mux/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)

OpenRouter 与 Claude Code Router 的结合产物。

---

现在你的编码助手可以在同一个会话中用 GLM 4.6 做一件事，用 Kimi K2 Thinking 做另一件事，用 Minimax M2 做第三件事。当主提供商宕机时，自动回退到备用提供商。

⚡️ **多模型智能 + 提供商弹性**

一个轻量级、Rust 驱动的代理，为 Claude Code 提供智能模型路由、提供商故障转移、流式支持和完整的 Anthropic API 兼容性。

```
Claude Code → Claude Code Mux → 多个 AI 提供商
              (Anthropic API)    (OpenAI/Anthropic APIs + 流式)
```

## Fork 增强功能

此 Fork 在原始项目基础上增加了以下重要改进：

- **提示短语路由 (Prompt Phrase Routing)** — 基于用户消息中的正则表达式模式路由请求（例如 "Think hard" → Opus, "[fast]" → Haiku）
- **提示缓存 (Prompt Caching)** — Anthropic 提示缓存透传，含缓存命中/未命中统计和 Token 速度指标
- **延续提示 (Continuation Prompts)** — 实验性功能（默认关闭）：自动注入延续提示，减少模型中途放弃多步骤任务的情况
- **增强日志** — 缓存统计、Token 吞吐量（tokens/sec）、匹配的路由短语
- **更好的模型切换** — 跨提供商保留思考/推理状态，处理切换时的内容块不兼容问题
- **大小写不敏感匹配** — 模型名称大小写不敏感匹配
- **改进的错误处理** — 防止崩溃，处理未知内容块

## 目录

- [Fork 增强功能](#fork-增强功能)
- [主要特性](#主要特性)
- [安装](#安装)
- [快速开始](#快速开始)
- [截图](#截图)
- [使用指南](#使用指南)
- [路由逻辑](#路由逻辑)
- [配置示例](#配置示例)
- [支持的提供商](#支持的提供商)
- [高级功能](#高级功能)
- [CLI 用法](#cli-用法)
- [故障排除](#故障排除)
- [FAQ](#faq)
- [性能](#性能)
- [为什么选择 Claude Code Mux？](#为什么选择-claude-code-mux)
- [文档](#文档)
- [更新日志](#更新日志)
- [贡献指南](#贡献指南)
- [许可证](#许可证)

## 主要特性

### 🎯 核心功能
- ✨ **现代化管理界面** — 美观的 Web 界面，支持自动保存和 URL 导航
- 🔐 **OAuth 2.0 支持** — Claude Pro/Max、ChatGPT Plus/Pro 和 Google AI Pro/Ultra 用户**免费使用**
- 🧠 **智能路由** — 按任务类型自动路由（websearch、reasoning、background、default）
- 🔄 **提供商故障转移** — 基于优先级的自动回退到备用提供商
- 🌊 **流式支持** — 完整的 Server-Sent Events (SSE) 流式响应
- 🌐 **多提供商支持** — 18+ 提供商，包括 OpenAI、Anthropic、Google Gemini/Vertex AI、Groq、ZenMux 等
- ⚡️ **高性能** — ~5MB 内存占用，<1ms 路由开销（Rust 驱动）
- 🎯 **统一 API** — 完整的 Anthropic Messages API 兼容性

### 🚀 高级功能
- 🔀 **自动映射** — 基于正则的模型名称转换（例如将所有 `claude-*` 转换为默认模型）
- 🎯 **后台任务检测** — 可配置的正则模式用于检测后台任务
- 🤖 **多代理支持** — 通过 `CCM-SUBAGENT-MODEL` 标签动态切换模型
- 📊 **实时测试** — 内置测试界面，验证路由和响应
- ⚙️ **集中设置** — 专用设置标签管理正则模式
- 🔄 **热重载** — 配置变更即时生效，无需重启进程

## 截图

<details>
<summary>📸 点击查看截图（5 张）</summary>

### 概览仪表盘
![仪表盘显示路由器配置、提供商和模型摘要](docs/images/dashboard.png)
*包含路由器配置和提供商管理的主仪表盘*

### 提供商管理
![提供商管理界面，支持添加/编辑](docs/images/providers.png)
*添加和管理多个 AI 提供商，自动格式转换*

### 模型映射与回退
![含优先级的回退路由模型配置](docs/images/models.png)
*配置模型及基于优先级的回退路由*

### 路由器配置
![智能路由规则配置界面](docs/images/routing.png)
*为不同任务类型设置智能路由规则*

### 实时测试界面
![使用真实 API 调用验证配置的测试界面](docs/images/testing.png)
*通过实时 API 请求和响应测试配置*

</details>

## 支持的提供商

**18+ AI 提供商，支持自动格式转换、流式和故障转移：**

- **Anthropic 兼容**: Anthropic（API Key/OAuth）、ZenMux、z.ai、Minimax、Kimi
- **OpenAI 兼容**: OpenAI、OpenRouter、Groq、Together、Fireworks、Deepinfra、Cerebras、Moonshot、Nebius、NovitaAI、Baseten
- **Google AI**: Gemini（OAuth/API Key）、Vertex AI（GCP ADC）

<details>
<summary>📋 查看完整提供商详情</summary>

### Anthropic 兼容（原生格式）
- **Anthropic** — 官方 Claude API 提供商（支持 API Key 和 OAuth）
- **Anthropic (OAuth)** — 🆓 **Claude Pro/Max 订阅者免费**
- **ZenMux** — 统一 API 网关（美国森尼维尔）
- **z.ai** — 中国，GLM 系列模型
- **Minimax** — 中国，MiniMax-M2 模型
- **Kimi For Coding** — 需要 Kimi 高级会员

### OpenAI 兼容
- **OpenAI** — 官方 OpenAI API（支持 API Key 和 OAuth）
- **OpenAI (OAuth)** — 🆓 **ChatGPT Plus/Pro 订阅者免费**
- **OpenRouter** — 统一 API 网关（500+ 模型）
- **Groq** — LPU 推理（极速）
- **Together AI** — 开源模型推理
- **Fireworks AI** — 快速推理平台
- **Deepinfra** — GPU 推理
- **Cerebras** — 晶圆级引擎推理
- **Moonshot AI** — 中国，Kimi 模型（OpenAI 兼容）
- **Nebius** — AI 推理平台
- **NovitaAI** — GPU 云平台
- **Baseten** — ML 部署平台

### Google AI
- **Gemini** — Google AI Studio/Code Assist API（支持 OAuth 和 API Key）
- **Gemini (OAuth)** — 🆓 **Google AI Pro/Ultra 订阅者免费**
- **Vertex AI** — GCP 平台，ADC 认证（支持 Gemini、Claude、Llama）

</details>

## 安装

### 方式 1：下载预编译二进制（推荐）

从 [GitHub Releases](https://github.com/winffychu/claude-code-mux/releases/latest) 下载最新版本，或从 [Actions](https://github.com/winffychu/claude-code-mux/actions/workflows/build.yml) 获取开发构建。

**Linux (x86_64)**
```bash
# 下载并解压（glibc）
curl -L https://github.com/winffychu/claude-code-mux/releases/latest/download/ccm-linux-x86_64.tar.gz | tar xz

# 或下载 musl 版本（静态链接，更便携）
curl -L https://github.com/winffychu/claude-code-mux/releases/latest/download/ccm-linux-x86_64-musl.tar.gz | tar xz

# 移动到 PATH
sudo mv ccm /usr/local/bin/
```

**macOS（Apple Silicon）**
```bash
curl -L https://github.com/winffychu/claude-code-mux/releases/latest/download/ccm-macos-aarch64.tar.gz | tar xz
sudo mv ccm /usr/local/bin/
```

#### 验证安装
```bash
ccm --version
```

### 方式 2：通过 Cargo 安装

```bash
cargo install --git https://github.com/winffychu/claude-code-mux.git
```

### 方式 3：从源码构建

```bash
git clone https://github.com/winffychu/claude-code-mux
cd claude-code-mux
cargo build --release
```

## 快速开始

### 1. 启动 Claude Code Mux

```bash
ccm start
```

服务器将在 `http://127.0.0.1:13456` 启动，带有 Web 管理界面。

> **💡 首次使用**：默认配置文件将自动创建在：
> - **Unix/Linux/macOS**: `~/.claude-code-mux/config.toml`
> - **Windows**: `%USERPROFILE%\\.claude-code-mux\\config.toml`

### 2. 打开管理界面

访问：
```
http://127.0.0.1:13456
```

你会看到包含以下标签的现代管理界面：
- **Overview（概览）** — 系统状态和配置摘要
- **Providers（提供商）** — 管理 API 提供商
- **Models（模型）** — 配置模型映射和回退
- **Router（路由器）** — 设置路由规则（自动保存！）
- **Test（测试）** — 使用实时请求测试配置

### 3. 配置 Claude Code

设置 Claude Code 使用代理：

```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:13456"
export ANTHROPIC_API_KEY="任意字符串"
claude
```

### 4. 可选：安装状态行（推荐）

查看 CCM 实际使用的模型：

```bash
ccm install-statusline
```

然后在 `~/.claude/settings.json` 中配置。

大功告成！

## 路由逻辑

**流程**: 自动映射（转换）→ WebSearch > Background > Subagent > Think > Default

### 0. 自动映射（模型名称转换）
- **触发条件**: 模型名称匹配 `auto_map_regex` 模式
- **示例**: 请求 `model="claude-4-5-sonnet"`，正则 `^claude-`
- **操作**: 将 `claude-4-5-sonnet` → `minimax-m2`（默认模型）
- **然后**: 继续执行下面的路由逻辑

### 1. WebSearch（最高优先级）
- **触发条件**: 请求的 tools 数组中包含 `web_search` 工具
- **路由到**: `websearch` 模型（如 GLM-4.6）

### 2. 后台任务（成本优化）
- **触发条件**: 原始模型名称匹配 `background_regex` 模式
- **默认模式**: `(?i)claude.*haiku`
- **路由到**: `background` 模型（如 GLM-4.5-air）

### 3. 子代理模型
- **触发条件**: 系统提示中包含 `<CCM-SUBAGENT-MODEL>model-name</CCM-SUBAGENT-MODEL>` 标签
- **路由到**: 指定的模型

### 4. 提示规则
- **触发条件**: 最后用户消息匹配配置的提示规则正则
- **路由到**: 匹配规则中指定的模型

### 5. 思考模式
- **触发条件**: 请求包含 `thinking` 字段且 `type: "enabled"`
- **路由到**: `think` 模型（如 Kimi K2 Thinking）

### 6. 默认（回退）
- **触发条件**: 无任何路由条件匹配
- **路由到**: 转换后的模型名称（如已自动映射）或原始模型名称

## 配置示例

### 成本优化方案（约 $0.35/百万 tokens）

**提供商**: Minimax（极速低价）、z.ai（GLM 模型）、Kimi（思考任务）、OpenRouter（回退）

**模型**:
- `minimax-m2` → Minimax（`MiniMax M2`）— $0.30/$1.20 每百万 tokens
- `glm-4.6` → z.ai（`glm-4.6`）— $0.60/$2.20 每百万 tokens
- `kimi-k2-thinking` → Kimi（`kimi-k2-thinking`）— 256K 上下文

**路由**:
- 默认: `minimax-m2`（Claude 成本的 8%，100 TPS）
- 思考: `kimi-k2-thinking`
- 后台: `glm-4.5-air`
- WebSearch: `glm-4.6`
- 自动映射: `^claude-`
- 后台正则: `(?i)claude.*haiku`

### 质量优先方案

**提供商**: Anthropic、OpenRouter

**路由**:
- 默认: `claude-sonnet-4-5`
- 思考: `claude-opus-4-1`
- 后台: `claude-haiku-4-5`

## CLI 用法

```bash
# 启动服务器
ccm start
ccm start --config path/to/config.toml
ccm start --port 8080

# 后台运行（nohup）
nohup ccm start > ccm.log 2>&1 &

# 其他命令
ccm --version
ccm --help
ccm install-statusline
```

## 性能

- **内存**: ~6MB RAM（Node.js 路由器的 25 倍效率）
- **启动**: <100ms 冷启动
- **路由**: <1ms 每个请求的开销
- **吞吐量**: 现代硬件上可处理 1000+ req/s
- **流式**: 零拷贝 SSE 流式，极低延迟

## FAQ

<details>
<summary><b>与现有 Claude Code 设置兼容吗？</b></summary>

是的！只需设置两个环境变量：
```bash
export ANTHROPIC_BASE_URL="http://127.0.0.1:13456"
export ANTHROPIC_API_KEY="任意字符串"
claude
```
</details>

<details>
<summary><b>所有提供商都失效了怎么办？</b></summary>

代理返回包含故障转移链详情的错误响应。
</details>

<details>
<summary><b>可以在 Claude Pro/Max 订阅中使用吗？</b></summary>

可以！Claude Code Mux 支持 OAuth 2.0 认证。通过管理界面 → Providers → Anthropic → 选择 OAuth。
</details>

## 为什么选择 Claude Code Mux？

### 🎯 两大核心优势

#### 1. 自动故障转移 🔄
基于优先级的提供商回退 - 主提供商失效时自动路由到备用。

#### 2. 更简单高效 ⚡️

| 特性 | Claude Code Router | Claude Code Mux |
|------|-------------------|----------------|
| **UI 访问** | `ccr ui`（单独启动） | 内置于 `http://localhost:13456` |
| **配置格式** | JSON + Transformers | TOML（更简单） |
| **内存占用** | ~156MB（Node.js） | ~6MB（Rust）- **25 倍更轻** |
| **故障转移** | ❌ 不支持 | ✅ 基于优先级的自动回退 |
| **Claude Pro/Max** | 仅 API Key | ✅ 支持 OAuth 2.0 |

## 许可证

MIT License - 见 [LICENSE](LICENSE)

**用 ⚡️ Rust 打造**
