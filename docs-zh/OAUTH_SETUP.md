# OAuth 身份认证设置

claude-code-mux 现在支持通过 OAuth 进行 Claude Pro/Max 订阅身份认证，让你无需 API 密钥即可使用 Claude 订阅！

## 特性

- ✅ **零成本**：Max 套餐用户 API 调用费用为 $0
- ✅ **PKCE 安全**：使用 PKCE（授权码交换证明密钥）保障 OAuth 2.0 安全
- ✅ **自动刷新**：令牌过期后自动刷新
- ✅ **持久化存储**：令牌安全存储在 `~/.claude-code-mux/oauth_tokens.json`

## 快速开始

### 1. 获取授权 URL

```rust
use claude_code_mux::auth::{OAuthClient, OAuthConfig, TokenStore};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 初始化
    let config = OAuthConfig::anthropic();
    let token_store = TokenStore::default()?;
    let oauth_client = OAuthClient::new(config, token_store);

    // 获取授权 URL
    let auth_url = oauth_client.get_authorization_url();

    println!("Go to: {}", auth_url.url);
    println!();
    println!("After authorization, you'll receive a code.");
    println!("Enter the code here:");

    // 从用户读取授权码
    let mut code = String::new();
    std::io::stdin().read_line(&mut code)?;
    let code = code.trim();

    // 用授权码换取令牌
    let token = oauth_client.exchange_code(
        code,
        &auth_url.verifier.verifier,
        "anthropic-max"  // Provider ID
    ).await?;

    println!("✅ Authentication successful!");
    println!("Access token expires at: {}", token.expires_at);

    Ok(())
}
```

### 2. 配置提供方

创建 `config/default.toml`：

```toml
[server]
host = "127.0.0.1"
port = 13456

[router]
default = "claude-sonnet-4.5"

# OAuth Provider
[[providers]]
name = "claude-max"
provider_type = "anthropic"
auth_type = "oauth"  # 使用 OAuth 代替 api_key
oauth_provider = "anthropic-max"  # 必须与 exchange_code 中使用的 provider_id 一致
enabled = true
models = []

[[models]]
name = "claude-sonnet-4.5"

[[models.mappings]]
actual_model = "claude-sonnet-4-5-20250929"
priority = 1
provider = "claude-max"
```

### 3. 启动服务

```bash
cargo run -- start
```

## OAuth 配置选项

### OAuthConfig::anthropic()

适用于 Claude Pro/Max 用户：
- **客户端 ID**：`9d1c250a-e61b-44d9-88ed-5944d1962f5e`
- **授权 URL**：`https://claude.ai/oauth/authorize`
- **令牌 URL**：`https://console.anthropic.com/v1/oauth/token`
- **作用域**：`org:create_api_key user:profile user:inference`

### OAuthConfig::anthropic_console()

通过 OAuth 创建 API 密钥（备选流程）：
- 使用 console.anthropic.com 进行授权
- OAuth 完成后自动创建 API 密钥
- 适用于需要传统 API 密钥工作流的场景

## 令牌存储

令牌以 JSON 格式存储在 `~/.claude-code-mux/oauth_tokens.json`：

```json
{
  "anthropic-max": {
    "provider_id": "anthropic-max",
    "access_token": "ey...",
    "refresh_token": "rt_...",
    "expires_at": "2025-11-18T15:30:00Z",
    "enterprise_url": null
  }
}
```

文件权限自动设置为 `0600`（仅所有者可读写）以确保安全。

## API 端点（未来）

以下 API 端点将被添加到管理服务器：

- `POST /api/oauth/authorize` - 获取授权 URL
- `POST /api/oauth/exchange` - 用授权码换取令牌
- `GET /api/oauth/tokens` - 列出所有 OAuth 提供方
- `DELETE /api/oauth/tokens/:provider` - 删除 OAuth 令牌

## 使用示例

```rust
use claude_code_mux::auth::{OAuthClient, OAuthConfig, TokenStore};

let config = OAuthConfig::anthropic();
let token_store = TokenStore::default()?;
let client = OAuthClient::new(config, token_store);

// 获取有效令牌（过期时自动刷新）
let access_token = client.get_valid_token("anthropic-max").await?;

// 在 HTTP 请求中使用
let response = reqwest::Client::new()
    .post("https://api.anthropic.com/v1/messages")
    .header("Authorization", format!("Bearer {}", access_token))
    .header("anthropic-version", "2023-06-01")
    .json(&request_body)
    .send()
    .await?;
```

## 安全说明

1. **切勿提交令牌**：`oauth_tokens.json` 文件包含敏感凭据
2. **文件权限**：始终以 `0600` 权限存储（Unix）
3. **PKCE**：使用 SHA-256 挑战码提供额外安全保障
4. **自动刷新**：令牌在过期前 5 分钟自动刷新

## 对比：API 密钥 vs OAuth

| 特性 | API 密钥 | OAuth（Max 套餐） |
|---------|---------|------------------|
| 设置 | 简单 | 一次性 OAuth 流程 |
| 费用 | 按 Token 计费 | $0（包含在订阅中） |
| 安全性 | 静态密钥 | 轮换令牌 + PKCE |
| 共享 | 容易（但不安全） | 按用户认证 |
| 过期 | 永不过期 | 自动刷新 |

## 故障排除

### "令牌刷新失败"
- 检查网络连接
- 确认 Max 订阅处于有效状态
- 重新认证：删除令牌并重新执行 OAuth 流程

### "未找到提供方的令牌"
- 先运行 OAuth 授权流程
- 检查配置中的 `oauth_provider` 是否与 TokenStore 中的 provider_id 一致

### "未找到环境变量"
- OAuth 不使用环境变量
- 确保提供方配置中设置了 `auth_type = "oauth"`

## 相关资源

- [OpenCode Anthropic Auth](https://github.com/sst/opencode-anthropic-auth) - 本实现的灵感来源
- [Anthropic OAuth 文档](https://docs.anthropic.com/claude/reference/oauth)
