# 基于 URL 的状态管理

管理后台 UI 使用 URL 作为状态管理的单一数据源。

## 概念

基于 URL 的状态管理是一种将应用程序状态编码到 URL 参数中的模式。该模式提供以下优势：

- **可共享的 URL**：特定视图状态可通过 URL 共享
- **浏览器历史记录集成**：前进/后退按钮自然工作
- **页面刷新后持久化**：刷新后状态依然保持
- **可添加书签**：特定视图可收藏为书签
- **简单的状态管理**：无需复杂的状态管理库

## 参考

本实现遵循以下文章中描述的模式：
https://alfy.blog/2025/10/31/your-url-is-your-state.html

## 实现

### URL 参数结构

```
?tab=<tab-name>&view=<view-name>
```

- `tab`：当前激活的标签页（overview, models, providers, router, settings）
- `view`：标签页内的视图状态（add, edit 等；若缺省则默认为列表视图）

#### 示例

```
# 概览标签页
http://localhost:13456/admin

# 提供商列表
http://localhost:13456/admin?tab=providers

# 添加提供商视图
http://localhost:13456/admin?tab=providers&view=add

# 模型列表
http://localhost:13456/admin?tab=models

# 添加模型视图
http://localhost:13456/admin?tab=models&view=add
```

### 核心函数

#### `getURLParams()`
解析当前 URL 的查询参数。

```javascript
function getURLParams() {
    return new URLSearchParams(window.location.search);
}
```

#### `updateURL(params, replace)`
更新 URL 并触发路由。

```javascript
function updateURL(params, replace = false) {
    const url = new URL(window.location);
    Object.entries(params).forEach(([key, value]) => {
        if (value === null || value === undefined) {
            url.searchParams.delete(key);
        } else {
            url.searchParams.set(key, value);
        }
    });

    if (replace) {
        window.history.replaceState({}, '', url);
    } else {
        window.history.pushState({}, '', url);
    }

    handleRoute();
}
```

#### `navigate(params)`
使用新历史记录条目进行导航。

```javascript
function navigate(params) {
    updateURL(params, false);
}
```

#### `handleRoute()`
读取 URL 并相应更新 UI。

```javascript
function handleRoute() {
    const params = getURLParams();
    const tab = params.get('tab') || 'overview';
    const view = params.get('view');

    // 隐藏所有标签页
    document.querySelectorAll('.tab-content').forEach(el => el.classList.add('hidden'));
    document.querySelectorAll('[id^="tab-"]').forEach(el => {
        el.classList.remove('tab-active');
        el.classList.add('text-gray-600');
    });

    // 显示选中的标签页
    document.getElementById('content-' + tab).classList.remove('hidden');
    const tabBtn = document.getElementById('tab-' + tab);
    if (tabBtn) {
        tabBtn.classList.add('tab-active');
        tabBtn.classList.remove('text-gray-600');
    }

    // 根据标签页和视图参数处理视图
    if (tab === 'providers') {
        if (view === 'add') {
            // 显示添加提供商视图
        } else {
            // 显示提供商列表
        }
    } else if (tab === 'models') {
        if (view === 'add') {
            // 显示添加模型视图
        } else {
            // 显示模型列表
        }
    }
}
```

### 使用示例

#### 标签页切换

```javascript
function showTab(tabName) {
    navigate({ tab: tabName, view: null });
}
```

#### 视图切换

```javascript
function showAddProvider() {
    navigate({ tab: 'providers', view: 'add' });
}

function showProvidersList() {
    navigate({ tab: 'providers', view: null });
}
```

#### 表单提交后的导航

```javascript
document.getElementById('add-provider-form').addEventListener('submit', async function(e) {
    e.preventDefault();

    // ... API 调用 ...

    if (saveResponse.ok) {
        notifySuccess('Provider added');
        e.target.reset();
        navigate({ tab: 'providers', view: null });  // 返回列表
        loadOverview();
    }
});
```

### 事件监听

#### 页面加载时路由

```javascript
window.addEventListener('DOMContentLoaded', () => {
    handleRoute();  // 基于 URL 设置初始状态
    loadOverview();
    updateLastSaved();
});
```

#### 浏览器历史记录支持

```javascript
// 支持前进/后退按钮
window.addEventListener('popstate', handleRoute);
```

## 优势

### 1. 声明式导航
```javascript
// 之前：手动 DOM 操作
document.getElementById('providers-list-view').classList.remove('hidden');
document.getElementById('providers-add-view').classList.add('hidden');

// 之后：声明式导航
navigate({ tab: 'providers', view: null });
```

### 2. 自动历史记录管理
浏览器的前进/后退按钮自动生效。

### 3. 可共享的深层链接
```
http://localhost:13456/admin?tab=providers&view=add
```
分享此 URL 可将接收者直接带到添加提供商的页面。

### 4. 易于测试
通过 URL 直接访问特定状态使测试更加简便。

## 最佳实践

### 1. 清理参数
通过将参数设置为 `null` 或 `undefined` 来移除不必要的参数。

```javascript
navigate({ tab: 'providers', view: null });  // 移除 view 参数
```

### 2. 处理默认值
当 URL 中缺少参数时使用默认值。

```javascript
const tab = params.get('tab') || 'overview';  // 默认值：overview
```

### 3. 与数据加载分离
将视图切换与数据加载分离。

```javascript
function handleRoute() {
    // ...
    if (view === 'add') {
        showAddView();
        loadAddViewData();  // 在单独的函数中加载数据
    }
}
```

### 4. Replace 与 Push 的区别
- **Push**：普通导航（添加到历史记录）
- **Replace**：替换当前历史记录（重定向等）

```javascript
navigate({ tab: 'providers' });           // pushState
updateURL({ tab: 'providers' }, true);    // replaceState
```

## 注意事项

### 1. 初始化顺序
在页面加载时，先调用 `handleRoute()` 再执行其他初始化。

```javascript
window.addEventListener('DOMContentLoaded', () => {
    handleRoute();      // 先基于 URL 设置状态
    loadOverview();     // 再加载数据
    updateLastSaved();
});
```

### 2. 防止无限循环
不要在 `handleRoute()` 内部调用 `navigate()`。

```javascript
// 错误做法
function handleRoute() {
    navigate({ tab: 'overview' });  // 无限循环！
}

// 正确做法
function handleRoute() {
    // 仅读取 URL 并更新 UI
}
```

### 3. 敏感数据
不要在 URL 中包含敏感数据。仅包含 ID 或状态信息。

```javascript
// 错误做法
navigate({ tab: 'providers', apiKey: 'sk-...' });

// 正确做法
navigate({ tab: 'providers', view: 'add' });
```

## 文件位置

实现位于：
- `src/server/admin.html`（第 586-659 行，第 1425-1433 行）

## 未来改进

- 使用 TypeScript 增加类型安全
- 增加 URL 参数验证
- 支持嵌套路由
- 查询参数加密/压缩（用于复杂状态）
