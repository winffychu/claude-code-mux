# Claude Code Mux 截图指南

本指南将帮助您为 README 和文档捕获高质量的截图。

## 前置条件

1. 服务器必须处于运行状态：`ccm start`
2. 浏览器访问 `http://127.0.0.1:13456`
3. 配置包含真实的 provider 和 model（非空）
4. 推荐分辨率：1920x1080 或更高
5. 浏览器缩放：100%（默认）

## 截图规格

- **格式**：PNG
- **存放位置**：`docs/images/`
- **命名规则**：小写字母加连字符（例如 `dashboard.png`）
- **推荐工具**：macOS 截图 (Cmd+Shift+4)、Windows 截图工具 或 Linux 截图

## 需要截取的截图

### 1. 仪表盘概览（`dashboard.png`）

**URL**：`http://127.0.0.1:13456/?tab=overview`

**需要捕获的内容**：
- 显示 Overview 选项卡的完整浏览器窗口
- Router Configuration 摘要卡片
- Providers 列表（显示 5-6 个 provider）
- Models 列表（显示 3-4 个 model）
- 顶部导航栏，所有选项卡可见
- "💾 Save to Server" 和 "🔄 Save & Restart" 按钮

**提示**：
- 确保数据已加载（不要出现 "No providers" 或 "No models"）
- 确保 router 配置显示所有 4 种路由选项（Default、Think、Background、WebSearch）

---

### 2. Provider 管理（`providers.png`）

**URL**：`http://127.0.0.1:13456/?tab=providers`

**需要捕获的内容**：
- 完整的 providers 列表视图
- 至少 5-6 个 provider 卡片，显示：
  - Provider 名称（例如 "zai coding plan"、"openrouter"、"kimi-for-coding"、"zenmux"、"openai"）
  - Provider 类型
  - 启用状态
  - 编辑/删除按钮
- "Add Provider" 按钮清晰可见

**提示**：
- 展示多种 provider 类型（兼容 Anthropic、兼容 OpenAI）
- 确保 provider 已启用（绿色勾选标记或指示器）

---

### 3. 添加 Provider 表单（`provider-add.png`）

**URL**：`http://127.0.0.1:13456/?tab=providers&view=add`

**需要捕获的内容**：
- 完整的添加 provider 表单
- Provider 类型选择卡片，显示：
  - Anthropic
  - z.ai
  - **ZenMux**（突出显示这个新功能！）
  - Minimax
  - OpenAI
  - OpenRouter
  - Groq
  - Together AI
  - Fireworks AI
  - Deepinfra
  - Cerebras
  - Nebius
  - NovitaAI
  - Baseten
- 下方的表单字段（Provider Name、API Key、Base URL）
- "Cancel" 和 "Add Provider" 按钮

**提示**：
- 突出展示卡片选择网格
- 不要填写表单字段（显示占位符文本）
- 突出现代卡片式 UI 设计

---

### 4. Model 映射（`models.png`）

**URL**：`http://127.0.0.1:13456/?tab=models`

**需要捕获的内容**：
- 完整的 models 列表视图
- 至少 3-4 个 model 卡片，显示：
  - Model 名称（例如 "glm-4.6"、"gpt-5.1"、"kimi-for-coding"）
  - Provider 映射及优先级徽章：
    - "Priority 1" 徽章（蓝色/主色）
    - "Priority 2" 徽章（灰色/次要色）- 用于降级备用
  - Provider → Actual model 映射关系
- "Add Model" 按钮

**示例 model 展示**：
```
glm-4.6
  - Priority 1: zai → glm-4.6
  - Priority 2: openrouter → z-ai/glm-4.6

gpt-5.1
  - Priority 1: zenmux → openai/gpt-5.1

kimi-for-coding
  - Priority 1: kimi-for-coding → claude-sonnet-4-5-20250929
```

**提示**：
- 至少展示一个带有降级备用（Priority 2）的 model
- 确保优先级徽章可见且颜色正确

---

### 5. 添加 Model 表单（`model-add.png`）

**URL**：`http://127.0.0.1:13456/?tab=models&view=add`

**需要捕获的内容**：
- 完整的添加 model 表单
- Model Name 输入框（显示占位符）
- Provider Mappings 部分，包含：
  - "Priority 1" 映射卡片（蓝色/高亮）
  - Provider 下拉菜单
  - Actual Model 输入框
  - 映射控制按钮（上移/下移/删除箭头）
- "+ Fallback Provider Add" 按钮
- 底部的 "Cancel" 和 "Add Model" 按钮

**提示**：
- 至少显示 1-2 张映射卡片
- 不要填写表单字段（显示占位符）
- 突出拖拽排序或优先级控制功能

---

### 6. Router 配置（`routing.png`）

**URL**：`http://127.0.0.1:13456/?tab=router`

**需要捕获的内容**：
- 完整的 router 配置表单，显示：
  - **Default Model** 下拉菜单（已填充）
  - **Think Model** 下拉菜单（已填充）
  - **Background Model** 下拉菜单（已填充）
  - **WebSearch Model** 下拉菜单（已填充）
  - **Auto-map Regex Pattern** 输入框（新功能！）
    - 示例值：`^claude-`
    - 解释该功能的辅助文本
- 没有提交按钮（自动保存功能）
- 可选：如果可见，显示 "✓ Auto-saved" 指示器

**提示**：
- 所有下拉菜单都填入真实的 model 名称
- 显示 Auto-map Regex Pattern 字段及示例值 `^claude-`
- 突出显示没有 "Save" 按钮（自动保存功能）
- 如有可能，触发自动保存并捕获 "✓ Auto-saved" 通知

---

### 7. 实时测试界面（`testing.png`）

**URL**：`http://127.0.0.1:13456/?tab=test`

**需要捕获的内容**：
- 完整的测试界面，显示：
  - Model 选择下拉菜单
  - 消息输入文本框及示例文本
  - "Send Message" 和 "Clear" 按钮
  - 响应容器，显示：
    - 响应文本
    - 响应时间
    - 使用的 Model
    - Token 用量（输入/输出）

**提示**：
- 先发送一条测试消息，然后捕获响应
- 示例消息："Hello, how are you?"
- 显示完整的响应及所有元数据
- 确保响应来自可正常工作的 provider

---

### 8. 自动保存指示器（可选）（`auto-save.png`）

**URL**：`http://127.0.0.1:13456/?tab=router`

**需要捕获的内容**：
- Router 选项卡，右上角可见 "✓ Auto-saved" 通知
- 绿色背景通知及勾选标记

**触发方法**：
1. 前往 Router 选项卡
2. 更改任意下拉菜单的值
3. 等待 500ms
4. 捕获绿色 "✓ Auto-saved" 通知

**提示**：
- 这是一张可选但推荐拥有的截图
- 展示实时自动保存功能的实际效果
- 需在 2 秒内捕获，防止通知消失

---

## 图片优化

截取截图后：

```bash
# 创建图片目录
mkdir -p docs/images

# 移动截图
mv ~/Desktop/dashboard.png docs/images/
mv ~/Desktop/providers.png docs/images/
mv ~/Desktop/provider-add.png docs/images/
mv ~/Desktop/models.png docs/images/
mv ~/Desktop/model-add.png docs/images/
mv ~/Desktop/routing.png docs/images/
mv ~/Desktop/testing.png docs/images/

# 可选：优化图片（需要 imageoptim 或类似工具）
# 可在不损失质量的情况下减小文件体积
```

## 检查清单

发布前，请确保：

- [ ] 所有 7 张必需截图已捕获
- [ ] 图片为 PNG 格式
- [ ] 图片位于 `docs/images/` 目录中
- [ ] 图片展示真实数据（非空状态）
- [ ] 浏览器界面整洁（无开发者工具，书签栏最小化）
- [ ] 文本清晰可读（不要太小）
- [ ] 无敏感 API 密钥可见
- [ ] Router 截图中突出显示了自动保存功能
- [ ] Provider 选择中可见 ZenMux provider
- [ ] Models 中的降级备用优先级徽章清晰可见

## 截图汇总

| 文件 | 用途 | 关键元素 |
|------|------|----------|
| `dashboard.png` | 主概览 | 所有 4 种路由配置、provider 列表、model 列表 |
| `providers.png` | Provider 管理 | 5-6 个 provider，带编辑/删除按钮 |
| `provider-add.png` | 添加 Provider 界面 | 卡片选择、表单字段、突出显示 ZenMux |
| `models.png` | Model 映射 | 3-4 个 model，带 Priority 1/2 徽章 |
| `model-add.png` | 添加 Model 界面 | 映射配置、降级备用按钮 |
| `routing.png` | Router 配置 | 所有 4 个下拉菜单 + Auto-map Regex 字段 |
| `testing.png` | 实时测试 | 请求/响应及元数据 |
| `auto-save.png`（可选） | 自动保存功能 | 绿色通知指示器 |

---

**准备好截图了吗？启动您的服务器，按照本指南操作！**

```bash
ccm start
# 打开 http://127.0.0.1:13456 开始截图！
```
