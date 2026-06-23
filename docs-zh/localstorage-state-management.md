# 基于 localStorage 的状态管理

管理后台 UI 使用 localStorage 管理客户端状态，仅在显式保存时与服务器同步。

## 概念

基于 localStorage 的状态管理利用浏览器 localStorage 作为客户端缓存，最大程度减少与服务端的不必要通信。

### 为什么采用此方案？

服务端读取 TOML 配置文件，**在重启之前不会重新加载**。因此：

1. ❌ **错误方式**：每次操作后重新从服务器获取
   - 保存配置到服务器 → 再次从服务器获取
   - 问题：服务器在重启前返回的是**过时数据**

2. ✅ **正确方式**：使用 localStorage 作为客户端缓存
   - 页面加载时从服务器获取一次 → 保存到 localStorage
   - 所有操作（增/删/改）仅更新 localStorage
   - 仅在用户点击“保存”按钮时同步到服务器

## 架构

```
页面加载
    ↓
从服务器获取配置 (/api/config/json)
    ↓
保存到 localStorage（缓存）
    ↓
渲染 UI
    ↓
用户操作（添加/删除 Provider、添加/删除 Model）
    ↓
仅更新 localStorage（不更新服务器）
    ↓
立即更新 UI
    ↓
用户点击“保存”或“保存并重启”按钮
    ↓
同步 localStorage → 服务器 (/api/config/json POST)
    ↓
（可选）重启服务器 (/api/restart POST)
```

## 实现

### 全局状态

```javascript
const appState = {
    config: null,
    loaded: false
};
```

### 核心函数

#### `saveToLocalStorage(config)`
将配置保存到 localStorage。

```javascript
function saveToLocalStorage(config) {
    try {
        localStorage.setItem('ccm_config', JSON.stringify(config));
        return true;
    } catch (error) {
        console.error('Failed to save to localStorage:', error);
        return false;
    }
}
```

#### `loadFromLocalStorage()`
从 localStorage 加载配置。

```javascript
function loadFromLocalStorage() {
    try {
        const stored = localStorage.getItem('ccm_config');
        return stored ? JSON.parse(stored) : null;
    } catch (error) {
        console.error('Failed to load from localStorage:', error);
        return null;
    }
}
```

#### `loadConfig()` - 页面加载时调用一次
从服务器获取配置并保存到 localStorage 和 appState。

```javascript
async function loadConfig() {
    try {
        const response = await fetch('/api/config/json');
        const config = await response.json();
        appState.config = config;
        appState.loaded = true;
        saveToLocalStorage(config);
        return config;
    } catch (error) {
        console.error('Failed to load config:', error);
        notifyError('Failed to load configuration');
        return null;
    }
}
```

#### `syncToServer()` - 仅从保存按钮调用
将 localStorage 中的配置发送到服务器。

```javascript
async function syncToServer() {
    try {
        const response = await fetch('/api/config/json', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(appState.config)
        });
        if (response.ok) {
            saveToLocalStorage(appState.config);
        }
        return response.ok;
    } catch (error) {
        console.error('Failed to sync to server:', error);
        return false;
    }
}
```

### 初始化

```javascript
window.addEventListener('DOMContentLoaded', async () => {
    await loadConfig();  // 从服务器获取一次
    handleRoute();
    renderOverview();
    updateLastSaved();
});
```

## 使用示例

### 删除 Provider

```javascript
async function deleteProvider(index) {
    if (!confirm('Are you sure you want to delete this Provider?')) {
        return;
    }

    try {
        // 1. 从状态中删除
        appState.config.providers.splice(index, 1);

        // 2. 仅保存到 localStorage（不保存到服务器）
        saveToLocalStorage(appState.config);

        // 3. 立即更新 UI
        renderProvidersList();
        renderOverview();

        // 4. 提示需要保存
        notifySuccess('Provider deleted (click Save to apply)');
    } catch (error) {
        console.error('Failed to delete provider:', error);
        notifyError('Failed to delete Provider');
    }
}
```

### 添加 Model

```javascript
document.getElementById('add-model-form').addEventListener('submit', async function(e) {
    e.preventDefault();

    const newModel = {
        // ... 收集表单数据
    };

    try {
        // 1. 添加到状态
        if (!appState.config.models) {
            appState.config.models = [];
        }
        appState.config.models.push(newModel);

        // 2. 仅保存到 localStorage（不保存到服务器）
        saveToLocalStorage(appState.config);

        // 3. 重置表单并导航
        notifySuccess('Model added (click Save to apply)');
        e.target.reset();
        navigate({ tab: 'models', view: null });
    } catch (error) {
        console.error('Failed to add model:', error);
        notifyError('Error adding model');
    }
});
```

### 全部保存

```javascript
async function saveAllConfig() {
    console.log('Saving all configuration...');

    try {
        // 同步 localStorage → 服务器
        const success = await syncToServer();

        if (success) {
            updateLastSaved();
            notifySuccess('All settings saved');
            renderOverview();
        } else {
            notifyError('Save failed');
        }
    } catch (error) {
        console.error('Failed to save all config:', error);
        notifyError('Error during save');
    }
}
```

### 保存并重启

```javascript
async function saveAndRestart() {
    if (!confirm('Save settings and restart server?')) return;

    try {
        // 1. 先保存
        await saveAllConfig();

        // 2. 稍等片刻后重启
        setTimeout(async () => {
            await fetch('/api/restart', { method: 'POST' });
            notifySuccess('Server restarted');
        }, 500);
    } catch (error) {
        console.error('Failed to save and restart:', error);
        notifyError('Failed to save and restart');
    }
}
```

## 数据流

### 页面加载
```
用户访问 /admin
    ↓
调用 loadConfig()
    ↓
GET /api/config/json
    ↓
appState.config = response
    ↓
saveToLocalStorage(config)
    ↓
renderOverview()
```

### 添加 Provider/Model
```
用户提交添加表单
    ↓
appState.config.providers.push(newProvider)
    ↓
saveToLocalStorage(appState.config)
    ↓
navigate({ tab: 'providers', view: null })
    ↓
renderProvidersList()（从 localStorage 读取）
```

### 删除 Provider/Model
```
用户点击删除按钮
    ↓
appState.config.providers.splice(index, 1)
    ↓
saveToLocalStorage(appState.config)
    ↓
renderProvidersList()（从 localStorage 读取）
```

### 保存按钮点击
```
用户点击“全部保存”
    ↓
调用 syncToServer()
    ↓
POST /api/config/json (appState.config)
    ↓
服务器保存到 TOML 文件
    ↓
notifySuccess('Saved')
```

### 保存并重启 Click
```
用户点击“保存并重启”
    ↓
saveAllConfig() → syncToServer()
    ↓
POST /api/config/json
    ↓
POST /api/restart
    ↓
服务器重启 → 加载新配置
```

## 优势

### 1. 解决服务器重启问题
```javascript
// ❌ 之前：服务器返回过时数据
appState.config.providers.push(newProvider);
await saveToServer();  // 保存到 TOML
const config = await fetch('/api/config/json');  // ⚠️ 过时数据！

// ✅ 之后：通过 localStorage 即时反映
appState.config.providers.push(newProvider);
saveToLocalStorage(appState.config);  // 即时反映！
renderProvidersList();  // 从 localStorage 读取
```

### 2. 性能提升
- 消除不必要的网络请求
- 即时 UI 更新
- 即使离线也可编辑配置

### 3. 更好的用户体验
- 所有更改立即在 UI 中反映
- 显式保存防止意外更改
- “未保存更改”状态清晰指示

### 4. 数据一致性
- localStorage 是唯一数据源
- 仅在显式保存时同步服务器
- 更改在页面刷新后仍然保留

## 注意事项

### 1. 需要服务器同步
用户必须点击“保存”按钮。否则：
- 更改仅存在于 localStorage
- 服务器重启后更改丢失
- 其他浏览器/设备无法看到更改

### 2. localStorage 容量限制
- 大多数浏览器：5-10MB 限制
- CCM 配置通常只有几十 KB，因此不成问题

### 3. 多标签页同步
当前实现假设为单标签页。多个标签页同时编辑时：
- 每个标签页使用独立的 localStorage
- 最后保存的会覆盖之前的保存
- 未来改进：通过 `storage` 事件同步标签页

### 4. 页面刷新
```javascript
// 页面刷新时 localStorage 优先级
window.addEventListener('DOMContentLoaded', async () => {
    // 即使有 localStorage 数据，也重新从服务器获取
    // 原因：其他用户可能已保存更改
    await loadConfig();  // 从服务器获取 → 更新 localStorage
    renderOverview();
});
```

### 5. 错误处理
```javascript
try {
    saveToLocalStorage(appState.config);
} catch (error) {
    // QuotaExceededError、SecurityError 等
    console.error('Failed to save to localStorage:', error);
    notifyError('Failed to save locally');
}
```

## 最佳实践

### 1. 始终显示通知
```javascript
notifySuccess('Provider added (click Save to apply)');
```
清晰告知用户需要保存。

### 2. 保持状态一致性
```javascript
// 状态更新 → localStorage 保存 → UI 更新
appState.config.providers.push(newProvider);
saveToLocalStorage(appState.config);
renderProvidersList();
```

### 3. 出错时恢复
```javascript
const backup = JSON.parse(JSON.stringify(appState.config));
try {
    appState.config.providers.splice(index, 1);
    saveToLocalStorage(appState.config);
    renderProvidersList();
} catch (error) {
    appState.config = backup;  // 恢复
    notifyError('Operation failed');
}
```

### 4. 确认服务器同步
```javascript
async function syncToServer() {
    const response = await fetch('/api/config/json', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(appState.config)
    });

    if (response.ok) {
        // 仅在成功时更新 localStorage
        saveToLocalStorage(appState.config);
    }

    return response.ok;
}
```

## 文件位置

实现位于：
- `src/server/admin.html` (lines 592-647, 802-827, 939-964, 1218-1273, 1319-1337)

## 相关文档

- [基于 URL 的状态管理](./url-state-management.md) - 使用 URL 参数的导航状态管理

## 未来改进

1. **更改追踪**
   - 比较 localStorage 与服务器状态
   - 添加“未保存更改”指示器

2. **自动保存**
   - 选项：每隔 N 秒自动保存
   - 页面退出时发出警告

3. **多标签页同步**
   ```javascript
   window.addEventListener('storage', (e) => {
       if (e.key === 'ccm_config') {
           appState.config = JSON.parse(e.newValue);
           renderOverview();
       }
   });
   ```

4. **撤销/重做**
   - 保存更改历史
   - 支持 Ctrl+Z / Ctrl+Shift+Z

5. **乐观 UI 更新**
   - 在服务器响应前更新 UI
   - 失败时回滚
