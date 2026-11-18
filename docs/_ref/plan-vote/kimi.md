# 议员方案：kimi-k3.1-pro#max

## 1. 技术栈

**主选：Rust + `windows` crate + Win32 原生控件，Direct2D/DirectWrite 仅负责列表和装饰。** 搜索、正文编辑使用 `EDIT`/`RICHEDIT`，直接获得中文 IME、选区、无障碍支持；用 `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`、`GetDpiForWindow`、`WM_DPICHANGED`、`GetMonitorInfoW` 处理多屏 DPI。

Release 开 `lto=true`、`codegen-units=1`、`opt-level="z"`、`panic=abort`。工程预算：EXE **2–5 MB**，空闲 Working Set **15–28 MB**；不是实测值，M0 必须验证，超 **15 MB/35 MB** 即停止增加 UI 特效并分析依赖、堆栈和 GPU 资源。

**备选：Rust + Slint 1.x + software renderer。** 面板、管理窗口可少 **4 人日**，预算 EXE **4–8 MB**、内存 **25–45 MB**；代价是内存可能超标，Windows IME、原生焦点及无障碍行为仍需专项验证，因此不作为主线。

不选 C# WinForms/WPF：自包含单文件通常 **60–90 MB**、空闲内存常超 **35 MB**；框架依赖包虽小，却引入外部 .NET Runtime。C++ Win32 性能相近，但开发、内存安全成本高于 Rust。

## 2. 注入设计

- 正文按 UTF-16 code unit 生成 `INPUT_KEYBOARD`；Emoji 代理对必须在**同一次** `SendInput` 中连续发送 key-down/up，禁止拆开。
- 存储内容不变；注入前把 `CRLF`、单独 `LF` 统一为 **CR/U+000D**。LF 不可移植；CR 等价回车，可能触发聊天发送或终端执行。
- `Tab/U+0009` 默认拒绝 Unicode 注入，防止切换焦点；不得静默替换成空格，只允许已验证的粘贴模式或“复制文本”。
- Unicode 模式限 **≤2,000 个 UTF-16 单元、单批一次 `SendInput`**，避免分批期间用户按键穿插；不使用 `BlockInput`。**2,001–10,000** 只能使用用户预先启用的兼容粘贴，或复制，不自动降级。
- 多行仅允许进入 M0 验证过的编辑器；未验证聊天软件只复制。`WindowsTerminal/conhost/OpenConsole/mintty` 多行一律只复制，禁止模拟粘贴执行命令。
- 返回事件数不等于请求数、`ERROR_ACCESS_DENIED`、前台再次变化均立即终止；提示可能已部分输入，禁止重试或补发粘贴。

## 3. 焦点校验

弹窗前依次记录：

1. `GetForegroundWindow`：顶层 HWND；`GetWindowThreadProcessId`：PID、线程 ID。
2. `GetGUIThreadInfo`：`hwndActive/hwndFocus/hwndCaret/rcCaret`；`ClientToScreen` 仅用于面板定位。
3. `IUIAutomation::GetFocusedElement`、`GetRuntimeId`、`ControlType`、`IsPassword`；浏览器/Electron 必须取得 UIA 身份，否则禁止自动注入。
4. `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` 记录上下文代数；出现非原窗口、非面板的前台切换即作废。

选择后：隐藏面板 → 检查 `IsWindow`、PID、上下文代数 → `SetForegroundWindow` → `AttachThreadInput` + `SetFocus` → 分离。每 **10 ms** 轮询，最长 **150 ms**；要求前台根窗口一致、`GetGUIThreadInfo.hwndFocus` 一致，浏览器再用 `CompareRuntimeIds` 一致。任一项失败、句柄为空或超时即 `FAILED_SAFE`，绝不改注当前活动窗口。

## 4. 风险定级

| 风险 | 处理及边界 |
|---|---|
| UIPI/管理员/UAC | `OpenProcessToken`、`TokenIntegrityLevel` 检测；普通进程对高完整性目标只复制。不提权、不过滤消息绕过；安全桌面不支持。 |
| 杀软误报 | 无键盘钩子、无网络、固定构建并 Authenticode 签名。Microsoft Defender 任一报毒或 VirusTotal **≥3 引擎**报毒暂停发布；误报无法工程根除。 |
| 浏览器/Electron | HWND 只能定位页面宿主；用 UIA RuntimeId 验证 DOM 焦点。`contenteditable` 无稳定 UIA 身份时声明不支持自动输入。 |
| IME 组合态 | 面板 `EDIT` 处理 `WM_IME_COMPOSITION`，组合期间 Enter 只确认候选。原应用组合态用 `ImmGetCompositionStringW(GCS_COMPSTR)` 检测，非空或无法确认则拒绝。 |
| 游戏/受保护进程/RDP | 独占全屏、反作弊、安全桌面及远程焦点无法可靠验证，M1 明确不支持，不提权绕过。 |

## 5. 数据模型

根对象：`schemaVersion`、`revision`、`categories[]`、`snippets[]`。分类含 `id/name/sortOrder/createdAt/updatedAt/deletedAt`；短语含 `id/title/content/categoryId/pinned/sortOrder/createdAt/updatedAt/useCount/lastUsedAt/deletedAt`。回收站软删除 **30 天**，保留原分类及排序。

默认排序：`pinned DESC → lastUsedAt DESC → updatedAt DESC → sortOrder ASC → id ASC`。检索权重：标题精确 **400**、前缀 **300**、子串 **200**、正文 **100**，再套默认排序。

导入上限 **20 MB/10,000 条**；同 UUID 且核心字段一致则跳过，同 UUID 内容不同则要求“跳过/另存副本”，副本重新生成 UUID，禁止覆盖。高版本文件只读拒绝；迁移按 `vN→vN+1`，先备份再执行。保存采用同目录临时文件、`FlushFileBuffers`、重新解析校验、`ReplaceFileW/MoveFileExW` 原子替换，保留最近 **5 份可解析备份**。

## 6. 模块、验收与人日

| 阶段/模块 | 验收标准 | 人日 |
|---|---|---:|
| M0 热键/托盘 | 注册冲突可见；退出释放；单实例 | 2 |
| M0 上下文/恢复 | Alt+Tab、关窗均零误投 | 3 |
| M0 固定面板 | 三条短语、键盘操作、P95≤150 ms | 2 |
| M0 双模式注入 | Unicode/剪贴板边界及失败安全通过矩阵 | 3 |
| M0 测试报告 | 兼容表、性能数据、路线结论齐全 | 4 |
| M1 JSON/迁移/备份 | 100 次写入故障注入无空库覆盖 | 4 |
| M1 CRUD/分类/回收站 | 重启一致、撤销及恢复正确 | 5 |
| M1 搜索/面板 | 1,000 条检索 P95≤50 ms | 3 |
| M1 设置/托盘 | 热键、自启、单实例、主题完整 | 2 |
| M1 导入导出 | 冲突、坏文件、超限均不破坏原库 | 2 |
| M1 DPI/主题/a11y | 双屏 100/125/150/200% 无越界、截断 | 3 |
| M1 剪贴板完善 | 外部变化不覆盖、失败可显式恢复 | 2 |
| M1 打包/回归 | 冷启动≤1.5 s，体积/内存达标 | 4 |

合计 **39 人日**，另留 **20%（8 人日）**集成缓冲；Slint 路线约 **43 人日**，主路线约 **47 人日**。

## 7. M0 测试矩阵

至少 **14 类宿主 ×10 场景＝140 组**：记事本、Word、VS Code、微信、Windows Terminal、conhost、mintty、管理员记事本，加 Chrome/Edge 各自的 input、textarea、contenteditable。场景为光标中插入、选区替换、中文、Emoji、多行、Tab、2,000/10,000 字、IME 候选、Alt+Tab、连续 20 次。Windows 11 全量，Windows 10 抽核心 **40 组**；另跑双屏三档 DPI。

**任一错误窗口/控件写入、聊天自动发送、终端执行、原文本丢失均为阻断。** 核心宿主各跑 100 次，成功写入不足 **99 次**或误目标非零，禁止进入 M1；不支持环境必须安全失败而非勉强注入。

## 8. 最可能翻车的 3 个假设

1. **HWND 能代表原输入控件**：浏览器和 Electron 的 DOM 焦点、选区不由 HWND 表达。
2. **Unicode 注入适合作为普遍默认**：CR、Tab、长文本、终端及聊天软件会直接破坏该假设。
3. **粘贴后可可靠自动恢复剪贴板**：Ctrl+V 没有通用“目标已完成”通知，`GetClipboardSequenceNumber` 只能发现剪贴板变化，不能证明粘贴完成。

## 我不同意 PRD 的地方

- “默认 Unicode”应改成**带长度、控制字符、宿主名单约束的默认 Unicode**，不能失败后自动粘贴。
- 兼容模式不应承诺自动恢复原剪贴板；保存本次文本备份，提供**用户显式恢复**，检测到序号变化绝不覆盖。
- “空闲内存”须定义为启动后静置 **60 秒的 Working Set**；性能指标固定基准机并跑 **100 次取 P95**，否则无法验收。
