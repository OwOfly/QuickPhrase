# QuickPhrase 技术方案（主席稿 · DeepSeek-V4.1-Flash）

> 与三家议员独立产出，汇总时按 §7 规则合并。日期 2026-11-18。

## 1. 技术栈

**主选：Rust（MSVC toolchain）+ `windows` crate + Slint UI。**

| 项 | 估算 | 依据 |
| --- | --- | --- |
| EXE | 5–9 MB | `windows` 只开所需 feature；Slint femtovg 渲染器；strip + LTO |
| 空闲内存 | 18–30 MB | 单进程、无 Chromium、无 GC |
| 开发量 | 中高 | 注入/焦点/存储有现成 crate，UI 声明式 |
| 中文 IME | 需 spike | Slint 走 winit IME 事件，TextInput 组合态是唯一不确定项 |
| 高 DPI | 内建 | Slint 逻辑像素 + PerMonitorV2 manifest |

模块：`hotkey`（RegisterHotKey）、`window_context`（前台/焦点快照与校验）、`inserter`（SendInput）、`snippet_store`（JSON 原子写）、`tray`（Shell_NotifyIcon）、`ui`（Slint）。

**备选：C# WPF。** 开发最快，三栏管理界面能省两周；但 self-contained ≈ 55–70 MB，framework-dependent 要装 .NET 8 Desktop Runtime（Win11 不自带），与"解压即用 + ≤15 MB"冲突。只有在你放弃体积目标时选它。

**逃生口**：若 Slint 的 IME 组合态修不好，退 Rust + Win32 原生控件（`EDIT`/`ListBox`/`ListView`）。丑，但中文 IME、无障碍、DPI 全部白送，注入链路一行不改。

排除：Tauri/Electron（WebView2 多进程，空闲 80–150 MB）；egui（IME 与无障碍都弱于 Slint）。

## 2. 注入设计

| 场景 | 策略 |
| --- | --- |
| 普通单行/多行文本 | `SendInput` + `KEYEVENTF_UNICODE`，UTF-16 码元逐个 down/up |
| 换行 | 发 `CR`（0x0D），**不发** `LF`（经典 EDIT 忽略 LF，RichEdit 视 CR 为段落）。绝不发 `VK_RETURN`，否则微信/钉钉会直接发送消息 |
| Emoji / 非 BMP | 代理对拆成两次 down/up，中间不插入其他事件；单独进测试矩阵 |
| Tab | 默认替换为 4 空格。`0x09` 在对话框里会切焦点，在编辑器里会触发补全 |
| 长文本 | > 1,000 字符 或 > 200 行 → 建议粘贴模式；用户坚持 Unicode 则按 200 字符分块、每块间 8 ms，逐块校验前台窗口未变，变了立即中止 |
| 注入期间用户按键 | 无法在不挂钩子的前提下拦截；靠"注入窗口 < 300 ms + 逐块校验前台"把影响压到最小，失败不回滚、不重试 |
| 终端宿主 | 检测 `WindowsTerminal.exe` / `conhost.exe` / `OpenConsole.exe` / `mintty.exe` / `putty.exe` / `cmd.exe` / `powershell.exe` / `pwsh.exe`：**禁止 Unicode 换行**，强制粘贴模式（Windows Terminal 支持 bracketed paste，多行不会逐行执行） |
| 聊天软件 | 多行先用 CR 验证；若宿主把换行当发送，降级为单行 + 空格，或粘贴模式 |
| 返回值 | `SendInput` 返回事件数必须等于请求数，不等即 FAILED_SAFE，不猜 |

## 3. 焦点校验

弹窗前（热键回调线程内，先于任何窗口显示）：

1. `GetForegroundWindow()` → `target`；等于自身面板/托盘窗口则中止。
2. `GetWindowThreadProcessId(target)` → `pid`,`tid`。
3. `GetGUIThreadInfo(tid, &gti)` → 存 `hwndFocus`、`hwndCaret`、`rcCaret`（`rcCaret` 仅部分控件有效，只用于定位面板，不用于校验）。
4. `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `GetTokenInformation(TokenIntegrityLevel)`：目标完整性级别高于本进程 → 直接判 `UNSUPPORTED_ELEVATED`，不注入。

选择后：

1. `IsWindow(target)`。
2. `SetForegroundWindow(target)`（热键触发时本进程持有最近输入事件，调用合法）；每 10 ms 轮询 `GetForegroundWindow()`，上限 300 ms。
3. `GetGUIThreadInfo(tid)` 再取一次：`hwndFocus` 必须非空、是 `target` 的后代；与快照不一致时尝试一次 `AttachThreadInput` + `SetFocus(savedFocus)` + 立即 detach，再复核。
4. 仍不一致 → `FAILED_SAFE`：不注入，弹出"复制文本 / 重试"，并把失败原因写入本地日志。
5. 一致 → 等 40 ms 让目标应用处理 `WM_SETFOCUS`，再注入。

判失败条件（任一命中即放弃）：前台窗口 ≠ target；`hwndFocus` 为空或不属 target；目标进程已退出；`SendInput` 返回数不符；目标完整性高于自身。

## 4. 风险定级

| 风险 | 可工程解决 | 处置 |
| --- | --- | --- |
| 管理员/UIPI | 是（检测） | 令牌比较，超权直接拒绝并提示 |
| 终端换行执行 | 是 | 宿主白名单 + 粘贴模式 |
| Electron/contenteditable 焦点恢复 | 部分 | 校验 + FAILED_SAFE + 复制退路，不承诺 |
| IME 组合态（面板内） | 是 | `ImmGetContext`/`ImmGetCompositionString` 判组合态，组合中 `Enter` 不选择 |
| IME 组合态（目标应用） | 否 | 恢复焦点后若目标仍在组合，放弃并提示 |
| 杀软误报 | 部分 | 不用全局钩子、不写启动项除非用户开启、代码签名、提交 360/火绒白名单 |
| 游戏/受保护进程 | 否 | 声明不支持，检测到即拒绝 |

## 5. 数据模型

```jsonc
{
  "schemaVersion": 1,
  "categories": [{ "id": "uuid", "name": "工作沟通", "sortOrder": 0, "icon": "chat", "color": "#7655E9", "createdAt": "..." }],
  "snippets": [{
    "id": "uuid", "title": "…", "content": "…", "categoryId": "uuid|null",
    "pinned": false, "sortOrder": 0,
    "createdAt": "…", "updatedAt": "…",
    "lastUsedAt": "…|null", "useCount": 0,
    "deletedAt": "…|null"
  }],
  "settings": { "hotkey": "Ctrl+Alt+Q", "insertionMode": "unicode|clipboard|perApp", "theme": "system|light|dark", "startup": false },
  "appRules": [{ "exe": "WindowsTerminal.exe", "mode": "clipboard", "newlinePolicy": "block" }]
}
```

规则：

- 排序：`pinned` 组优先 → `lastUsedAt` 降序（null 最后）→ `updatedAt` 降序 → 标题升序。
- 检索：保存时预计算 `searchBlob`（标题 + 正文小写 + 拼音首字母 + 全拼），1,000 条线性扫描足够；标题命中排在正文命中之前。
- 软删除：`deletedAt` 非空进回收站，30 天后清理；回收站恢复/彻底删除。
- 导入：按 `id` 匹配 → 更新；同标题不同 `id` → 让用户选跳过/副本；其余新增。上限 20 MB、5,000 条。
- 迁移：`schemaVersion` 高于当前 → 拒绝加载并提示升级程序；低 → 先备份再逐版迁移。
- 原子写：写 `.tmp` → `FlushFileBuffers` → `ReplaceFile`（保留上一版为 `.bak`）→ 轮转 5 份；写失败绝不动原文件。

## 6. 模块划分

**M0（5 人日）**

| 模块 | 职责 | 验收 |
| --- | --- | --- |
| M0-1 骨架 | 单实例互斥、托盘、`RegisterHotKey`、崩溃日志 | 冷启动 < 1.5 s；退出后热键释放 |
| M0-2 面板 | 固定 3 条数据、搜索框、上下键、Enter/Esc、多屏定位 | 520×420 在 125/150/200% 不越界 |
| M0-3 上下文 | 快照、恢复、校验 | 校验失败时 0 次误注入 |
| M0-4 注入 | Unicode 注入、CR 换行、分块、失败安全 | 8 个宿主矩阵出报告 |
| M0-5 报告 | 兼容矩阵 + 失败归因 + 技术路线结论 | 阻断项逐条有结论 |

**M1（18 人日）**

| 模块 | 职责 | 人日 |
| --- | --- | --- |
| 存储层 | schema、原子写、备份、迁移、导入导出 | 3 |
| 检索层 | 关键词、拼音首字母、排序、高亮 | 2 |
| 管理界面 | 三栏、CRUD、分类、回收站、校验、未保存拦截 | 5 |
| 设置 | 快捷键录制与冲突、输入模式、主题、开机启动、路径 | 2 |
| 托盘与生命周期 | 菜单、暂停/恢复、第二实例唤起、退出清理 | 2 |
| 注入加固 | 终端检测、按应用规则、长文本降级、提权检测 | 2 |
| 体验细节 | 空态、撤销删除、错误反馈、首次引导 | 1 |
| 打包 | manifest、图标、版本信息、README、免安装 zip | 1 |

## 7. M0 测试矩阵

宿主 8：记事本、Chrome（input / textarea / contenteditable）、Edge、VS Code、Windows Terminal、微信、Word、资源管理器重命名。
场景 6：中文单行、英文+全角、Emoji、多行、2,000 字、选区替换。
另加：IME 组合态、管理员窗口、面板期间切窗、150% DPI、热键冲突、连按 20 次。

共 48 格 + 6 个专项。阻断项 12 格：Chrome 三个输入类型 ×（中文/多行）、VS Code 中文/多行、记事本中文/多行、微信多行、终端任意换行、错误窗口注入 = 0。

## 8. 最可能翻车的三个假设

1. **"Unicode 注入在主流应用 ≥99% 成功"**——Electron、终端、部分 Java/Qt 控件大概率低于这个数，验收口径要先改。
2. **"焦点能恢复到原控件"**——浏览器与 Electron 的 `hwndFocus` 恢复不可靠，实际可用率取决于 FAILED_SAFE 的退路体验。
3. **"15 MB + 好看 UI 能兼得"**——能做，但毛玻璃、阴影、动效要么砍，要么改用 DWM 系统材质，不能按 HTML 原型 1:1 复刻。
