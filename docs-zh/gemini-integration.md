# Google AI 集成指南

## 概述

本文档涵盖 Google AI 模型的两个独立提供商：

1. **Google Gemini**（`provider_type: "gemini"`）
   - 用于 Google AI Studio（AI.google.dev）
   - 支持 OAuth 2.0（Code Assist API）和 API Key
   - **仅限 Gemini 模型**

2. **Vertex AI**（`provider_type: "vertex-ai"`）✅
   - 用于 Google Cloud Platform
   - 使用应用默认凭据（ADC）
   - **支持 Gemini、Claude、Llama 及其他模型**

## ⚠️ 重要提示：OAuth 使用 Code Assist API

来自 gemini-cli 的 OAuth client_id `681255809395-...` 仅注册于 Google 的 **Code Assist API**（`cloudcode-pa.googleapis.com`），**而非** 公共 Gemini API（`generativelanguage.googleapis.com`）。

因此：
- **OAuth** → Code Assist API（`cloudcode-pa.googleapis.com/v1internal:generateContent`）
- **API Key** → 公共 Gemini API（`generativelanguage.googleapis.com/v1beta/models/{model}:generateContent`）

---

## Google Gemini 提供商（`provider_type: "gemini"`）

### 认证方式

#### 1. OAuth 2.0（Google AI Pro/Ultra）- Code Assist API

**OAuth 配置：**
```
client_id: 681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com
client_secret: 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
auth_url: https://accounts.google.com/o/oauth2/v2/auth
token_url: https://oauth2.googleapis.com/token
redirect_uri: http://localhost:13456/api/oauth/callback
scopes:
  - https://www.googleapis.com/auth/cloud-platform
  - https://www.googleapis.com/auth/userinfo.email
  - https://www.googleapis.com/auth/userinfo.profile
```

**OAuth 流程：**
1. 使用 PKCE 生成授权 URL
2. 用户在浏览器中授权 → 重定向到回调 URL
3. 用授权码换取 access token + refresh token
4. **调用 loadCodeAssist API 获取/验证项目 ID**
5. 将令牌与 project_id 一起存储，以便自动刷新

**项目 ID 处理：**

| 账户类型 | 项目 ID 来源 | 需要执行的操作 |
|----------|-------------|---------------|
| **个人账户**（免费版） | API 自动生成 | ✅ 无需操作 - 自动工作 |
| **工作空间账户**（标准版） | 用户自定义 | ⚠️ 设置 `GOOGLE_CLOUD_PROJECT` 环境变量 |
| **授权用户** | 用户自定义 | ⚠️ 设置 `GOOGLE_CLOUD_PROJECT` 环境变量 |

**仅限工作空间/授权用户：**
1. 访问 [Google Cloud Console](https://console.cloud.google.com/)
2. 创建新项目或选择现有项目
3. 为该项目启用 "Cloud AI Companion API"
4. 复制 Project ID（不是项目名称）
5. 在启动服务器前设置环境变量：
   ```bash
   export GOOGLE_CLOUD_PROJECT=your-project-id
   # 或者
   export GOOGLE_CLOUD_PROJECT_ID=your-project-id
   ```

**LoadCodeAssist API:**
```
POST https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist
Authorization: Bearer {access_token}
Content-Type: application/json

{
  "cloudaicompanionProject": null,
  "metadata": {
    "ideType": "IDE_UNSPECIFIED",
    "platform": "PLATFORM_UNSPECIFIED",
    "pluginType": "GEMINI"
  }
}

响应：
{
  "cloudaicompanionProject": "projects/123456789",
  "currentTier": { ... }
}
```

**API 端点（OAuth - Code Assist API）：**
```
https://cloudcode-pa.googleapis.com/v1internal:generateContent
```

**请求格式（Code Assist API）：**
```json
{
  "model": "gemini-2.0-flash-exp",
  "user_prompt_id": "gemini-1234567890",
  "request": {
    "contents": [...],
    "systemInstruction": {...},
    "generationConfig": {...}
  }
}
```

**响应格式（Code Assist API）：**
```json
{
  "response": {
    "candidates": [...],
    "usageMetadata": {...}
  },
  "traceId": "..."
}
```

**请求头：**
```
Authorization: Bearer {access_token}
Content-Type: application/json
```

#### 2. API Key（Google AI Studio）

**API Key 配置：**
- 从 https://aistudio.google.com/app/apikey 获取 API key

**API 端点：**
```
https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent?key={api_key}
```

**请求头：**
```
Content-Type: application/json
```

### 配置示例

**基于 OAuth 的 Gemini 提供商：**
```toml
[[providers]]
name = "gemini-oauth"
provider_type = "gemini"
auth_type = "oauth"
oauth_provider = "google"
models = ["gemini-2.0-flash-exp", "gemini-2.5-pro"]
enabled = true
```

**基于 API Key 的 Gemini 提供商：**
```toml
[[providers]]
name = "gemini-studio"
provider_type = "gemini"
auth_type = "apikey"
api_key = "your-api-key-from-ai-studio"
models = ["gemini-1.5-pro", "gemini-1.5-flash"]
enabled = true
```

---

## Vertex AI 提供商（`provider_type: "vertex-ai"`）

Vertex AI 是 Google Cloud 的统一 AI 平台，可访问多个模型系列：
- **Gemini 模型**（2.5 Flash、2.0 Flash、1.5 Pro 等）
- **Claude 模型**（3.7 Sonnet、3.5 Haiku 等）
- **Llama 模型**（Llama 4、Llama 3.x）
- **其他模型**（通过 Model Garden）

### 认证

Vertex AI 使用**应用默认凭据（ADC）**进行认证。

**设置步骤：**
1. 安装 Google Cloud SDK：https://cloud.google.com/sdk/docs/install
2. 使用 ADC 认证：
   ```bash
   gcloud auth application-default login
   ```
3. 这将创建服务器自动使用的凭据

**其他认证方式：**
- 服务账号 JSON 密钥
- Google Cloud API key
- Workload Identity（用于 GKE 部署）

### 配置

**必填字段：**
- `project_id`：您的 Google Cloud Project ID（不是项目名称）
- `location`：GCP 区域（例如 us-central1、europe-west1）

**可用区域：**
```
us-central1, us-east1, us-west1, us-west4
europe-west1, europe-west2, europe-west4
asia-east1, asia-northeast1, asia-southeast1
australia-southeast1
```

**TOML 配置示例：**
```toml
[[providers]]
name = "vertex-gemini"
provider_type = "vertex-ai"
project_id = "my-gcp-project-id"
location = "us-central1"
models = [
    "publishers/google/models/gemini-2.0-flash-exp",
    "publishers/google/models/gemini-1.5-pro",
]
enabled = true
```

### 管理界面设置

1. 在管理界面中，选择 **"Vertex AI"** 作为提供商类型
2. 输入您的 **Project ID**（来自 Google Cloud Console）
3. 从下拉菜单中选择 **Location**（11 个区域）
4. 使用 `publishers/google/models/` 前缀添加模型名称
5. 保存配置
6. **启动服务器前**，使用 ADC 认证（见上文）

**管理界面功能：**
- ✅ 从 11 个预配置区域中选择
- ✅ 保存前验证 Project ID 和 Location
- ✅ 在提供商卡片上显示 Vertex AI 徽章
- ✅ 随时编辑/更新 Vertex AI 凭据

### API 端点

```
https://{location}-aiplatform.googleapis.com/v1/projects/{project}/locations/{location}/publishers/google/models/{model}:generateContent
```

**示例：**
```
https://us-central1-aiplatform.googleapis.com/v1/projects/my-project/locations/us-central1/publishers/google/models/gemini-2.0-flash-exp:generateContent
```

**请求头：**
```
Authorization: Bearer {access_token}  // From ADC
Content-Type: application/json
```

### 支持的模型

Vertex AI 支持来自多个提供商的模型。模型名称必须包含 `publishers/{publisher}/models/` 前缀。

**Gemini 模型：**
```
publishers/google/models/gemini-2.0-flash-exp
publishers/google/models/gemini-1.5-pro
publishers/google/models/gemini-1.5-flash
```

**Claude 模型：**
```
publishers/anthropic/models/claude-3-7-sonnet
publishers/anthropic/models/claude-3-5-haiku
```

**Llama 模型：**
```
publishers/meta/models/llama-4-maverick
publishers/meta/models/llama-3-2-90b
```

---

## API 格式

### 请求格式（Gemini generateContent API）

```json
{
  "contents": [
    {
      "role": "user",
      "parts": [
        {
          "text": "你好，你怎么样？"
        }
      ]
    }
  ],
  "generationConfig": {
    "temperature": 1.0,
    "topP": 0.95,
    "topK": 40,
    "maxOutputTokens": 8192,
    "stopSequences": []
  },
  "systemInstruction": {
    "parts": [
      {
        "text": "你是一个乐于助人的助手。"
      }
    ]
  }
}
```

### 响应格式（非流式）

```json
{
  "candidates": [
    {
      "content": {
        "parts": [
          {
            "text": "我很好，谢谢你的关心！"
          }
        ],
        "role": "model"
      },
      "finishReason": "STOP",
      "index": 0,
      "safetyRatings": [...]
    }
  ],
  "usageMetadata": {
    "promptTokenCount": 10,
    "candidatesTokenCount": 15,
    "totalTokenCount": 25
  }
}
```

### 流式响应格式

```
data: {"candidates": [{"content": {"parts": [{"text": "Hello"}],"role": "model"}}]}

data: {"candidates": [{"content": {"parts": [{"text": " there"}],"role": "model"}}]}

data: {"candidates": [{"content": {"parts": [{"text": "!"}],"role": "model"}],"finishReason": "STOP","usageMetadata": {...}}]}
```

## Anthropic → Gemini 转换

### 角色映射
```
Anthropic → Gemini
user      → user
assistant → model
```

### 内容块映射

**文本：**
```rust
// Anthropic
ContentBlock::Text { text: "Hello" }

// Gemini
{ "text": "Hello" }
```

**图片：**
```rust
// Anthropic
ContentBlock::Image {
  source: ImageSource {
    type: "base64",
    media_type: "image/png",
    data: "iVBORw0KG..."
  }
}

// Gemini
{
  "inline_data": {
    "mime_type": "image/png",
    "data": "iVBORw0KG..."
  }
}
```

**思考（扩展思考）：**
```rust
// Anthropic
ContentBlock::Thinking {
  thinking: "让我想一想...",
  signature: "..."
}

// Gemini（转换为文本）
{ "text": "让我想一想..." }
```

### 系统提示词
```rust
// Anthropic
request.system = Some("你是一个乐于助人的助手")

// Gemini
{
  "systemInstruction": {
    "parts": [
      { "text": "You are a helpful assistant" }
    ]
  }
}
```

### 生成配置
```rust
// Anthropic → Gemini
max_tokens       → maxOutputTokens
temperature      → temperature
top_p            → topP
stop_sequences   → stopSequences

// Gemini 特有
topK: 40（默认值）
```

### 工具（函数调用）

**Anthropic：**
```json
{
  "tools": [
    {
      "name": "get_weather",
      "description": "获取天气信息",
      "input_schema": {
        "type": "object",
        "properties": {
          "location": { "type": "string" }
        }
      }
    }
  ]
}
```

**Gemini：**
```json
{
  "tools": [
    {
      "functionDeclarations": [
        {
          "name": "get_weather",
          "description": "获取天气信息",
          "parameters": {
            "type": "object",
            "properties": {
              "location": { "type": "string" }
            }
          }
        }
      ]
    }
  ]
}
```

## 模型名称参考

### Gemini 提供商模型

对于 `provider_type: "gemini"`（OAuth 或 API Key 认证），使用简单的模型名称：

**稳定模型（推荐）：**
```
gemini-2.5-pro              # 最新生产模型
gemini-2.0-flash-exp        # 快速实验模型
gemini-1.5-pro              # 上一代稳定模型
gemini-1.5-flash            # 快速稳定模型
gemini-1.5-flash-8b         # 超快速模型
```

**预览模型（受限访问）：**
```
gemini-3-pro-preview        # 需要预览权限（可能返回 404）
gemini-exp-1206             # 实验性模型
```

⚠️ **注意**：像 `gemini-3-pro-preview` 这样的预览模型需要特殊访问权限，可能并非所有用户可用。如果您收到 404 错误，请改用 `gemini-2.5-pro` 或 `gemini-2.0-flash-exp`。

### Vertex AI 提供商模型

对于 `provider_type: "vertex-ai"`，使用带有 `publishers/{publisher}/models/` 前缀的完整模型路径。

**注意：** 本节已在上面"Vertex AI 提供商 → 支持的模型"部分记录。

## 错误处理

### 常见错误

**401 未授权：**
```json
{
  "error": {
    "code": 401,
    "message": "请求缺少必需的身份验证凭据。",
    "status": "UNAUTHENTICATED"
  }
}
```

**403 禁止访问：**
```json
{
  "error": {
    "code": 403,
    "message": "用户没有访问此资源的权限。",
    "status": "PERMISSION_DENIED"
  }
}
```

**429 速率限制：**
```json
{
  "error": {
    "code": 429,
    "message": "资源已耗尽（例如检查配额）。",
    "status": "RESOURCE_EXHAUSTED"
  }
}
```

## 实现检查清单

### Gemini 提供商（`provider_type: "gemini"`）
- [x] 支持 OAuth/API Key 的 GeminiProvider 结构体
- [x] auth/oauth.rs 中的 OAuth 配置
- [x] OAuth 令牌刷新逻辑
- [x] 请求转换（Anthropic → Gemini）
- [x] 响应转换（Gemini → Anthropic）
- [x] **通过 loadCodeAssist API 获取项目 ID**（仅 OAuth）
- [x] **带有 project_id 字段的令牌存储**（仅 OAuth）
- [x] **Code Assist API 集成**（OAuth）
- [x] **公共 Gemini API 集成**（API Key）
- [x] **工具的 JSON Schema 元数据清理**
- [ ] 流式支持
- [x] 工具/函数调用支持
- [x] 图片支持（inline_data）
- [x] 错误处理和映射
- [x] 管理界面集成
  - [x] OAuth 流程界面（类似于 Anthropic/OpenAI）
  - [x] API Key 输入
  - [x] 提供商类型选择器显示 "📱 Google Gemini"

### Vertex AI 提供商（`provider_type: "vertex-ai"`）
- [x] 注册表中独立的提供商类型
- [x] 复用带 ADC 认证的 GeminiProvider
- [x] Project ID 和 Location 配置字段
- [x] ADC 令牌获取逻辑
- [x] Vertex AI API 端点构建
- [x] 管理界面集成
  - [x] 独立的提供商类型选择器 "Vertex AI"
  - [x] Project ID 输入字段
  - [x] Location 下拉菜单（11 个区域）
  - [x] ADC 认证说明
  - [x] 提供商卡片上的 Vertex AI 徽章
  - [x] 支持编辑/更新 Vertex AI 提供商
- [x] 多模型支持（Gemini、Claude、Llama）
- [x] 带有 `publishers/` 前缀的模型名称验证

## 参考资料

- Gemini CLI OAuth：`/tmp/gemini-cli/packages/core/src/code_assist/oauth2.ts`
- Gemini API 文档：https://ai.google.dev/gemini-api/docs
- Vertex AI 文档：https://cloud.google.com/vertex-ai/docs/generative-ai/start/quickstarts/api-quickstart
