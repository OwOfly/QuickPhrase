# QuickPhrase 最终方案（final-plan v4.1）

- 汇总：DeepSeek-V4.1-Flash（主席）
- 日期：2026-11-18
- 状态：**M0-1 可开工**；M0-2/M0-4 待本文修订项冻结后放行
- 修订依据：v1 独立复核（B1–B3、M1–M10）、v2 定向复核（B4、B5、M11–M14）、v3 三模型 focused review、v4 独立增量复核（B6、B7、M15–M22）
- 目标仓库：`D:\Code\AIProject\quickphrase`
- 规格来源：`prd-v1.1.md`
- 历史版本：`final-plan-v1.md` … `final-plan-v4.md`
- 复核记录：`reviews/2026-11-18-findings-register.md`、`reviews/chair-closure-v3.md`、`reviews/2026-11-18-v3-review-record.md`

## 0. v4.1 增量（对应 v4 复核 B6/B7、M15–M22）

| 项 | 问题 | v4.1 处置 | 落点 |
| --- | --- | --- | --- |
| B6 | UIA 子元素的 `NativeWindowHandle` 可能为 0 | 非零 → `GetAncestor(GA_ROOT)` 必须等于 target；为零 → TreeWalker 上溯到 target 的顶层 UIA 窗口；无法确认 → `IDENTITY_UNKNOWN`、只复制 | §5.2、§5.5、T18 |
| B7 | 只验证“原控件”，未验证“原插入位置” | 对外契约改为“**原控件 + 提交时当前光标**”；原生 `EM_GETSEL`、UIA TextRange 能比则比，不一致拒绝；比不了记 `POSITION_UNVERIFIED` | §4 D24、§5.5、§7 G11 |
| M15 | `IsReadOnly` 不是统一元素属性 | 分支矩阵：`ES_READONLY` / `ValuePattern.CurrentIsReadOnly` / TextRange `IsReadOnlyAttribute`；未知即只复制 | §5.5、T20 |
| M16 | `RuntimeId` 可能被复用 | `CompareElements` 唯一权威；RuntimeId 只做诊断与日志 | §4 D10、§7 G6、T21 |
| M17 | worker 无法被“销毁” | 请求 ID + 期限；放弃线程计数上限 2；超限进入 `UIA_UNAVAILABLE`，全程只复制 | §5.4、T22 |
| M18 | 终端单行仍可能含控制字符 | 全局控制字符闸门：C0/C1（除按策略处理的 Tab/CR）、U+2028/U+2029 → 只复制并说明 | §6.2、G12、T23 |
| M19 | 首写无目标文件 | 首写 `MoveFileExW`，覆盖才用 `ReplaceFileW` | §6.1、T24 |
| M20 | 前台代数缺实现 | 面板生命周期内 `EVENT_SYSTEM_FOREGROUND` 监听 + 允许清单；非键盘钩子 | §5.2、G3、T25 |
| M21 | COPY_ONLY 也会覆盖剪贴板 | 明确写剪贴板、快照上一份纯文本、提示、可恢复；非文本原内容明确告知无法恢复 | §6.5、T26 |
| M22 | UI 验收不足 | 补 100/125/150/200%、IME 组合态、深色对比、键盘高亮、托盘图标 | §6.4 |

**对复核意见的一处选择**

B7 给了两条路。v4.1 选第 2 条（当前插入点契约），并保留 best-effort 的严格对比；不选“无法验证位置就一律只复制”。理由：UIA TextRange 在 Chromium 上的稳定性未验证，选第 1 条会把浏览器单行也砍成复制，与 §6.3 冲突。

---

## 1. 背景与目标

Windows 桌面全局快捷短语输入器：热键唤起面板 → 搜索选中 → 文本写入触发前的原输入控件。单机、离线、无账号。

**第一版只验证三件事**：短语能维护、随时能唤起、文字准确进入原位置。跨应用输入不可靠时，宁可不输入。

**位置承诺（v4.1 修订）**：回到**原输入控件**，在**提交时的当前光标/选区**插入。不承诺冻结热键时刻的字符偏移。原生编辑控件与可用的 UIA TextRange 会做严格对比，不一致则拒绝。

**交付形态**：免安装单 EXE（zip），数据在 `%LocalAppData%\QuickPhrase`。

## 2. 范围与非目标

**M1（MVP）范围**

- 全局热键（默认 `Ctrl+Alt+Q`，可改、冲突提示）
- 快捷面板：搜索、分类筛选、键盘导航、置顶排序、空态/无结果/只读状态
- 短语 CRUD、分类、置顶、回收站（软删除 30 天）
- 设置：热键录制、输入方式、按应用规则（仅收紧）、主题、开机启动、数据路径
- 持久化：JSON + 原子写 + 5 份滚动备份 + 损坏恢复
- 导入/导出 JSON（冲突预览、不覆盖）
- 输入：单行 Unicode 全覆盖；多行仅限已验证的 `TEXT_EDITOR` 区域；其余只复制
- 托盘常驻、单实例、第二实例唤起
- 本地日志与诊断包导出（不含短语内容）

**非目标（V1 不做）**

云同步、团队协作、AI 改写、富文本/图片、缩写自动展开、批量宏、读取输入框内容、OCR、移动端、浏览器插件、密码保险箱、自动更新、终端自动粘贴、**浏览器多行自动输入**。

**声明不支持**：UAC 安全桌面、锁屏、独占全屏/反作弊游戏、提权目标窗口、跨会话（RDP）、无 UIA 且无 HWND 焦点的自绘控件、Document 级控件、ValuePattern-only 控件、只读控件。

### 2.1 隐私与安全边界

- 不联网；无遥测、无崩溃上传。
- 不安装键盘钩子，不采集击键历史，不读取输入框内容，不记录剪贴板历史。
- 面板显示期间安装 `EVENT_SYSTEM_FOREGROUND` 只读监听（`WINEVENT_OUTOFCONTEXT`），仅记录前台窗口句柄与序号；面板关闭即卸载。该监听不拦截输入，不读取窗口内容。
- **不做焦点事件缓存**；所有 UIA 查询发生在热键后的单次捕获与提交前的单次复验。
- 日志只记元数据（exe、窗口类、区域类别、失败码、字符数、耗时、UIA 状态），不记短语正文。
- 数据只落 `%LocalAppData%\QuickPhrase`。
- 密码框与只读控件检测到即拒绝。
- 不申请管理员权限；提权目标直接拒绝。

## 3. 侦察结论（含实证）

| 事实 | 证据 | 影响 |
| --- | --- | --- |
| Chromium 系 `hwndFocus` 等于顶层窗口，`hwndCaret=NULL` | 2026-11-18 本机实测 Electron：`Focus=0x40A80 [Chrome_WidgetWin_1]`，Caret NULL | HWND 只能定位窗口 |
| UIA 子元素的 `NativeWindowHandle` 可能为 0 | UIA 属性文档 | 不能用“句柄属于 target”作为唯一前置校验 |
| `RuntimeId` 可能随时间复用 | UIA 属性文档 | 不能单独作为跨时刻身份依据 |
| `IsReadOnly` 不是统一的元素属性 | `ValuePattern.CurrentIsReadOnly`、TextRange `IsReadOnlyAttribute` 各自定义 | 只读判断必须按 Pattern 分支 |
| `GetFocusedElement` 返回系统当前焦点 | UIA 文档 | 面板激活后调用会拿到自己 → 捕获必须先于激活 |
| UIA 调用不可硬取消 | UIA 线程模型 | 超时只能丢弃结果，不能承诺销毁线程 |
| `ReplaceFileW` 要求目标已存在 | Win32 文档 | 首次写入必须走 `MoveFileExW` |
| `SendInput` 被 UIPI 阻断时失败，错误码不说明原因 | MS Learn | 令牌预检 |
| `SendInput` 不重置键盘状态；事件串行 | 同上 | 等物理键释放 |
| WinForms/WPF + NativeAOT 非官方支持 | `NETSDK1175` | C# 备选不可行 |
| 依赖版本 | crates.io：`slint 1.19.0`、`windows 0.62.2` | 版本以 crates.io 为准 |
| 本机工具链 | rustc 1.93.0-nightly、.NET 8.0.427 | Rust 可开工 |

## 4. 决策点表

| # | 决策 | 裁决 | 理由 | 残余风险 |
| --- | --- | --- | --- | --- |
| D1 | 运行时 | **Rust + `windows` 0.62** | 体积/内存/无运行时依赖 | UI 开发量 |
| D2 | UI | **原生控件 + D2D 装饰** | IME/无障碍 | 视觉需 Spec |
| D3 | 视觉 | DWM 圆角 + 系统材质 + 自绘阴影 | ≤35 MB | Win10 降级 |
| D4 | 注入 | `SendInput` + `KEYEVENTF_UNICODE`，UTF-16 逐码元，单次调用 | 不占剪贴板 | 控件丢事件 |
| D5 | 换行 | **V1 一律 CR(0x0D)**；浏览器 LF 推迟 M2 | EDIT 认 CR；VK_RETURN 会发送 | 浏览器多行不可用 |
| D6 | Tab | 默认 4 空格，设置可见；保留 Tab 仅粘贴模式 | 避免焦点跳转 | 缩进语义 |
| D7 | 长文本 | ≤2,000 UTF-16 单元；超出只复制 | 单次注入过长 | 体验 |
| D8 | 终端 | 含任何换行 → 只复制；单行仍受 D25 控制字符闸门约束 | 换行=执行 | 多行受限 |
| D9 | 聊天 | 单行 Unicode；多行只复制 | 防误发送 | 宿主升级 |
| D10 | 控件身份 | 原生 `hwndFocus` 相等；UIA **仅 `CompareElements` 为权威**；RuntimeId 只做日志 | RuntimeId 可复用 | UIA 失败即复制 |
| D11 | 身份三态 | `VERIFIED / MISMATCH / UNKNOWN`；Document、ValuePattern-only、只读、无插入点均不可用 | 身份优先 | 可用性下降 |
| D12 | 提权 | 令牌完整性比较；失败或更高即拒绝 | 返回值不可信 | 受保护进程 |
| D13 | 剪贴板 | 默认不碰；兼容模式见 §6.5 | 无损不可能 | 用户丢失原内容 |
| D14 | 数据 | `categories` + `snippets` + `settings` + `rules-user` + `schemaVersion` + `revision` | 可维护 | — |
| D15 | 回收站 | M1 软删除 30 天 | 原型有入口 | 1 人日 |
| D16 | 排序/检索 | pinned → lastUsedAt → updatedAt → sortOrder → id；标题 400/300/200，正文 100；前 50 | 可验收 | 拼音 M2 |
| D17 | 指标 | 静置 60 s Working Set；100 次 P95；固定基准机 | 可复现 | 机器差异 |
| D18 | 工期 | M0 10 + M1 22 + 缓冲 4 = 36 人日；v4.1 新增量由缓冲吸收 | §8 | 排期 |
| D19 | 浏览器覆盖率 | M0 只统计；V1 浏览器多行固定只复制 | 不做无证据放行 | 体验 |
| D20 | UI 验收 | §6.4 Spec + 截图 + DPI/IME/深色检查 | 视觉是产品 | 还原度 |
| D21 | 区域分类 | `TERMINAL / TEXT_EDITOR / UNKNOWN`；UNKNOWN 多行只复制；区域升级只能来自内置标记包 | 嵌入式终端 | 识别率 |
| D22 | 捕获时机 | UIA 捕获在面板激活前同步完成（60 ms）；超时只复制 | 异步必拿面板自身 | 延迟 +≤60 ms |
| D23 | 浏览器多行 | V1 只复制；LF 实验推迟 M2 | 网页发送/执行 | 体验 |
| D24 | 位置契约 | **原控件 + 提交时当前光标**；可验证时做严格对比，不一致拒绝 | 见 §0 说明 | 位置漂移 |
| D25 | 控制字符 | C0/C1（Tab/CR 按策略）、U+2028/U+2029 → 只复制并说明 | 终止符/换行语义不可控 | 少数内容无法自动输入 |
| D26 | COPY_ONLY 剪贴板 | 明确写剪贴板、快照上一份纯文本、提示、可恢复 | 复制必然覆盖剪贴板 | 原内容为非文本时不可恢复 |
| D27 | UIA worker | 请求 ID + 期限；放弃线程上限 2；超限 `UIA_UNAVAILABLE` | COM 不可中断 | 会话内降级为只复制 |

## 5. 架构

### 5.1 线程

| 线程 | 职责 | 约束 |
| --- | --- | --- |
| UI | 消息循环、窗口、托盘、`WM_HOTKEY` | 捕获阶段最多阻塞 60 ms |
| 注入 | 恢复、校验、`SendInput`、剪贴板 | 不阻塞 UI |
| UIA（MTA，常驻） | 捕获、复验、属性、区域判定 | 队列上限 1；请求 ID；超时丢弃 |
| 前台监听 | 面板生命周期内的 WinEvent 回调 | 只记录句柄与序号，回调内不做 UIA |
| 存储 | 原子写、备份、导入导出 | 独立句柄 |

### 5.2 热键 → 捕获 → 面板

```
t0  WM_HOTKEY
    ├─ GetForegroundWindow / pid / 创建时间 / hwndFocus / 令牌
    ├─ 若前台是自己 → 中止
    ├─ 安装 EVENT_SYSTEM_FOREGROUND 监听（记录 seq/hwnd/tick）
    ├─ UIA 捕获请求，deadline = t0 + 60 ms
    │    GetFocusedElement
    │    ├─ NativeWindowHandle != 0 → GetAncestor(GA_ROOT) 必须 == target
    │    ├─ NativeWindowHandle == 0 → TreeWalker 上溯，命中 target 的顶层 UIA 窗口
    │    ├─ 属性：ControlType / ClassName / IsEnabled / IsPassword
    │    ├─ 只读：按 Pattern 分支（见 §5.5）
    │    ├─ 插入点：TextPattern2.GetCaretRange / TextPattern.GetSelection（尽力）
    │    └─ 区域判定
    ├─ CAPTURED_VERIFIED / CAPTURED_UNSUPPORTED / CAPTURED_TIMEOUT
    └─ 之后才 SetForegroundWindow(panel)
```

- 绝不在面板激活后查询“当前焦点”当原始快照。
- 超时或失败：面板照常打开，本次只能复制，页脚提示“此应用仅支持复制”。
- 启动时预热 `CUIAutomation`，避免首次调用付 COM 初始化成本。
- `NativeWindowHandle==0` 是正常情况，不是拒绝理由；只有上溯失败才判 UNKNOWN。

### 5.3 提交 → 恢复 → 注入

1. 等物理键释放（Ctrl/Alt/Shift/Win/Enter），≤300 ms。
2. 关闭面板，卸载前台监听。
3. `SetForegroundWindow(target)`；10 ms 轮询，≤300 ms。
4. 复核 G1–G12。
5. 通过后单次 `SendInput`。
6. 按返回值进入 `API_EVENTS_ACCEPTED` 或 `INPUT_PARTIAL_OR_UNKNOWN`。

### 5.4 UIA 客户端约束（M17）

- 单例 `CUIAutomation`，只在常驻 MTA 线程使用。
- 每个请求带自增 ID 与 deadline；过期结果直接丢弃，不写回任何状态。
- 队列上限 1；新请求覆盖旧请求。
- COM 调用**不可取消**。超时后放弃等待，但线程可能仍在目标进程内阻塞。
- 放弃线程计数上限 2；超过后本次会话进入 `UIA_UNAVAILABLE`：所有注入降级为只复制，UI 与存储不受影响。
- 不对挂起线程做 `join`；仅关闭句柄，进程退出统一回收。
- 可评估 `IUIAutomation2::put_TransactionTimeout`，但它只是建议值，不构成 60 ms 硬保证。

### 5.5 能力矩阵

| 目标类型 | 身份 | 可编辑 / 只读 | 插入点 | 位置对比 | 允许 |
| --- | --- | --- | --- | --- | --- |
| 原生 `Edit`/`RichEdit` | `hwndFocus` 相等 | `ES_PASSWORD` 拒绝；`ES_READONLY` 拒绝 | `EM_GETSEL` / `rcCaret` | 可严格对比 | 是 |
| UIA Edit + TextPattern2 | `CompareElements` | `IsEnabled`；只读查 TextRange `IsReadOnlyAttribute`，未知即拒绝 | `GetCaretRange().isActive` | 尽力对比 | 是 |
| UIA Edit + TextPattern | `CompareElements` | 同上 | `GetSelection().Length ≥ 1` | 尽力对比 | 是 |
| UIA Document（整页） | 不可细分 | — | — | — | **否，只复制** |
| ValuePattern-only | `CompareElements` | `CurrentIsReadOnly`；未知即拒绝 | 无光标 | — | **否，只复制** |
| 元素失效/未知 | — | — | — | — | **否，只复制** |

**只读判定分支（M15）**

1. 原生：`GetWindowLong(GWL_STYLE) & ES_READONLY`。
2. 有 `ValuePattern`：`CurrentIsReadOnly`。
3. 有 `TextPattern`：对当前 range 取 `UIA_IsReadOnlyAttributeId`。
4. 多来源冲突：任一为只读即拒绝；全部未知按只读处理。

**位置对比（B7 / D24）**

- 捕获时记录：原生 `EM_GETSEL`；UIA `GetSelection()` 的 range 集合，或 `GetCaretRange()`。
- 提交时重新读取并用 `CompareEndpoints` / 选区端点比较。
- 两端都可用且不一致 → `REJECTED_BEFORE_INPUT`。
- 任一端不可用 → `POSITION_UNVERIFIED`，按“当前光标”契约继续，但写日志与统计。

区域判定：

| 区域 | 判据 | 多行 | 单行 |
| --- | --- | --- | --- |
| `TERMINAL` | `ConsoleWindowClass`；已知终端 exe；内置标记包命中的终端区域 | 只复制 | Unicode（过 D25） |
| `TEXT_EDITOR` | 原生 `Edit`/`RichEdit`/`_WwG`；内置标记包命中的编辑器区域 | Unicode + CR | Unicode |
| `UNKNOWN` | 其余（浏览器、Electron、Excel 网格、混合宿主未识别区域） | 只复制 | Unicode |

**不按 exe 放行多行。** `Code.exe` 默认 UNKNOWN；只有内置验证标记包命中编辑器区域才升级。

## 6. 数据契约

### 6.1 文件与备份

| 文件 | 内容 | 写入 | 备份 |
| --- | --- | --- | --- |
| `data.json` | schema/revision/categories/snippets | 原子写 | 5 份 |
| `settings.json` | 热键/模式/Tab/主题/启动/暂停 | 原子写 | 5 份 |
| `rules-user.json` | 用户收紧规则 | 原子写 | 5 份 |
| `logs/qp-<date>.log` | 元数据日志 | 追加 | 3 × 500 KB |

统一流程：

1. 写同目录 `.tmp`。
2. `FlushFileBuffers`。
3. 重新解析校验。
4. 目标不存在 → `MoveFileExW` + `MOVEFILE_WRITE_THROUGH`；目标存在 → 先轮转备份，再 `ReplaceFileW`。
5. 任一步失败：保留原文件，删除 `.tmp`，提示可恢复。

损坏恢复按时间倒序尝试 5 份备份；全失败进入只读模式。

### 6.2 结构

```jsonc
// data.json
{
  "schemaVersion": 1,
  "revision": 42,
  "categories": [{ "id": "uuid", "name": "工作沟通", "sortOrder": 0, "createdAt": "...", "updatedAt": "..." }],
  "snippets": [{
    "id": "uuid", "title": "项目进度回复", "content": "您好……",
    "categoryId": "uuid|null", "pinned": true, "sortOrder": 0,
    "createdAt": "...", "updatedAt": "...",
    "useCount": 3, "lastUsedAt": "...|null",
    "lastInsertStatus": "accepted|partial|rejected|null",
    "deletedAt": "...|null",
    "searchBlob": "小写标题 + 换行 + 小写正文"
  }]
}
```

```jsonc
// settings.json
{ "schemaVersion": 1, "hotkey": "Ctrl+Alt+Q", "insertionMode": "unicode",
  "tabPolicy": "spaces4", "theme": "system", "startup": false, "paused": false, "dataDir": null }
```

```jsonc
// rules-user.json —— 只能收紧，不能放宽
{ "version": 1, "rules": [
  { "exe": "Code.exe", "action": "copy_only" },
  { "exe": "WINWORD.EXE", "action": "copy_only" }
] }
```

**规则动作语义**

| 动作 | 含义 | 是否允许用户设置 |
| --- | --- | --- |
| `copy_only` | 该 exe 一律只复制 | 是 |
| `disable` | 该 exe 不响应热键 | 是 |
| `tab_spaces` | 该 exe Tab 转 4 空格 | 是 |
| `allow_multiline` | 放宽多行 | **否** |
| `set_region` | 改区域分类 | **否** |

**内容闸门（D25）**

自动注入前扫描正文；出现以下任一字符即 `COPY_ONLY`，提示“包含不可自动输入的控制字符”：

- C0：U+0000–U+0008、U+000B、U+000C、U+000E–U+001F（Tab 按 D6 处理，CR/LF 按 D5 归一）。
- C1：U+007F–U+009F。
- Unicode 分隔符：U+2028、U+2029。

不做静默删除或替换。

安全硬规则（不可被用户覆盖）：终端换行、未验证区域多行、密码框、只读控件、提权目标、身份不匹配、Document/ValuePattern-only、控制字符。

其余规则：

- 标题 1–80 UTF-16 单元；正文 1–10,000 UTF-16 单元。
- 导入 ≤20 MB / ≤10,000 条；同 `id` 一致跳过、不同要求“跳过/另存副本”，禁止覆盖。
- `schemaVersion` 高于程序 → 只读导出，不写入。
- 迁移前写 `pre-migrate-v<N>-<ts>.json`。
- 软删除 30 天；恢复保留原 `id` 与分类。

### 6.3 内置宿主规则

| 宿主 | 单行 | 多行 | 备注 |
| --- | --- | --- | --- |
| 终端类（WindowsTerminal/OpenConsole/conhost/mintty/putty/cmd/powershell/pwsh） | Unicode（过 D25） | **只复制** | `ConsoleWindowClass` 直接判 TERMINAL |
| `Code.exe`（含集成终端） | Unicode | **只复制**，除非内置标记命中编辑器区域 | 混合宿主 |
| `idea64/pycharm64/devenv` | Unicode | 只复制，除非标记命中 | 可能内嵌终端 |
| `WeChat/QQ/DingTalk/Feishu` | Unicode | 只复制 | 防发送 |
| `WINWORD.EXE` / `OUTLOOK.EXE`（Word 编辑器） | Unicode | Unicode + CR，仅当 `_WwG` 命中 | — |
| `EXCEL.EXE` | Unicode | **只复制** | 网格内 CR=提交并下移 |
| 记事本 / 写字板 | Unicode | Unicode + CR | 原生编辑控件 |
| Chrome / Edge / Firefox | Unicode | **只复制**（V1 不做 LF 通道） | 网页可能发送/执行 |
| 资源管理器重命名 | Unicode | 不适用 | 单行 |
| 未登记 | Unicode | 只复制 | 首次提示未验证 |

### 6.4 UI Spec

| 项 | 值 |
| --- | --- |
| 面板 | 520 × 420 逻辑像素（PRD 14.3），允许 460–540 × 320–470 |
| 管理窗口 | 1040 × 650，最小 800 × 540 |
| 栅格 | 8 px；内边距 16/20；列表项 64 |
| 圆角 | 窗口 12，卡片 12，控件 8，标签 6 |
| 字体 | `Segoe UI Variable` → `Microsoft YaHei UI`；12/13/15/17/20 |
| 色板 | `#7655E9` `#5E45D7` `#F5F6FB` `#22243E` `#777D92` `#E9EBF3` `#F0EDFF` |
| 状态 | default / hover / selected / keyboard-focus / disabled / empty / no-result / error / loading / copy-only |
| 深色 | 独立 token；`DWMWA_USE_IMMERSIVE_DARK_MODE` |
| DPI | 100/125/150/200%，Per-Monitor V2 |
| 控件皮肤 | `EDIT` 去边框自绘；`ListView` owner-draw；滚动条原生 |
| 验收 | 6 张 150% 截图（面板/管理/设置 × 浅/深）；每状态至少 1 张 |

**M22 补充验收**

- 100/125/150/200% 四档布局检查：无截断、无重叠、无越界。
- IME 组合态：候选未上屏时 Enter 只确认候选，不触发选择。
- 深色模式：正文、次要文字、选中态对比度达标（正文 ≥ 4.5:1）。
- 键盘高亮：选中项与焦点状态不能只靠颜色区分。
- 托盘：100/150/200% 下图标清晰，菜单项不截断。
- 与 HTML 原型逐状态对照：允许观感差异，不允许状态缺失。

### 6.5 剪贴板行为（M21）

| 场景 | 行为 |
| --- | --- |
| Unicode 注入成功 | 全程不触碰剪贴板 |
| `COPY_ONLY`（不支持自动输入） | 将短语写入剪贴板 `CF_UNICODETEXT`；在内存保存上一份纯文本快照（≤1 MB）；提示“已复制，原剪贴板内容可在托盘恢复” |
| `COPY_ONLY` 且原剪贴板含非文本格式 | 明确提示“原剪贴板含非文本内容，无法恢复” |
| 用户主动切到 `clipboard` 兼容模式 | 必须先过警示弹窗，写明会替换剪贴板 |
| 粘贴后 | 比对 `GetClipboardSequenceNumber`；被外部改动则保留新内容，不覆盖 |
| 恢复 | 设置与托盘提供“恢复先前纯文本”，仅在快照仍有效时可用 |
| 终端/未验证区域/多行 | 禁止用剪贴板模式绕过区域策略 |

## 7. 状态机

```
IDLE
 └─ WM_HOTKEY → HOTKEY_RECEIVED
      └─ CAPTURING_ORIGINAL          # 同步，deadline 60 ms
           ├─ CAPTURED_VERIFIED      → PANEL_READY
           ├─ CAPTURED_UNSUPPORTED   → PANEL_READY（只复制）
           └─ CAPTURED_TIMEOUT       → PANEL_READY（只复制）
                ├─ CANCELLED
                └─ USER_COMMITTED
                     └─ RESTORE_AND_VERIFY
                          ├─ REJECTED_BEFORE_INPUT
                          └─ READY_TO_INSERT
                               └─ INPUT_STARTED
                                    ├─ API_EVENTS_ACCEPTED
                                    └─ INPUT_PARTIAL_OR_UNKNOWN
```

**守卫**

- G1 `IsWindow(target)`。
- G2 PID + 进程创建时间一致。
- G3 前台序列：面板生命周期内的 WinEvent 监听只允许 target、面板自身、托盘/隐藏窗口；出现其它 hwnd 即作废。提交时再比对当前前台。
- G4 300 ms 内 `GetForegroundWindow() == target`；不盲目强推。
- G5 `hwndFocus` 非空且属 target；原生路径要求与快照相等。
- G6 身份：原生 `hwndFocus` 相等；UIA 仅 `CompareElements` 为真；RuntimeId 不参与放行判定。
- G7 能力：按 §5.5 分支；Document、ValuePattern-only、只读、密码、无插入点 → 拒绝。
- G8 完整性 ≤ 本进程。
- G9 物理键释放（≤300 ms）。
- G10（调用后）`SendInput` 返回数 == 请求数。
- G11 位置：可对比且不一致 → 拒绝；不可对比 → `POSITION_UNVERIFIED` 继续。
- G12 控制字符：命中 D25 集合 → 拒绝自动注入，转 COPY_ONLY。

失败：G1–G9、G11、G12 → `REJECTED_BEFORE_INPUT`；G10 → `INPUT_PARTIAL_OR_UNKNOWN`，不重试、不补发粘贴。

**计数**：`useCount` = 到达 `API_EVENTS_ACCEPTED` 的发起次数；UI 文案“使用次数”，不宣称成功插入。

## 8. 实施计划

### M0（10 人日）

| 模块 | 内容 | 验收 | 人日 |
| --- | --- | --- | ---: |
| M0-1 骨架 | cargo、DPI manifest、单实例、托盘、热键、日志 | 冷启动 ≤1.5 s；退出释放热键 | 1.5 |
| M0-2 捕获 | 同步快照、UIA 60 ms、0-HWND 上溯、身份/能力/区域、只读分支、前台监听 | T7/T8/T12–T15、T18、T20–T22、T25 通过 | 3 |
| M0-3 面板 | 原生窗口 + EDIT + ListView，3 条硬编码，键盘导航，多屏 | 100 次 P95 ≤150 ms；DPI 不越界 | 1.5 |
| M0-4 注入与状态机 | Unicode/CR/Tab/代理对、提权、修饰键、三态、位置对比、控制字符 | T1–T6、T9–T11、T16、T17、T19、T23 通过 | 3 |
| M0-5 矩阵与实验 | 168 项检查；E2/E3/E4 | 报告可复现 | 1 |

**实验**

- **E2**：VS Code 编辑器 vs 集成终端的 UIA 区域标记是否可复现。
- **E3**：UIA 捕获延迟分布、`NativeWindowHandle==0` 比例、上溯成功率（每宿主 100 次）。
- **E4**：位置对比覆盖率（原生 + UIA），决定 M1 是否收紧“严格原位”。
- E1（浏览器 LF）推迟到 M2。

**新增工作量**（0-HWND 上溯、只读分支、位置对比、前台监听、worker 放弃策略）约 1 人日，从 4 人日缓冲中扣，不改变 36 人日总盘。

### M1（22 人日）

| 模块 | 内容 | 依赖 | 验收 | 人日 |
| --- | --- | --- | --- | ---: |
| M1-1 存储 | schema、原子写（含首写分支）、备份、迁移、恢复 | M0 | 100 次故障注入 | 3.5 |
| M1-2 检索 | blob、打分、排序、高亮 | M1-1 | 1,000 条 P95 ≤50 ms | 2 |
| M1-3 面板完整 | 分类、空态、失败态、只读态 | M0-3 | PRD 14.3 全覆盖 | 3 |
| M1-4 管理窗口 | 三栏、CRUD、分类、回收站、校验 | M1-1/2 | 全流程 | 5 |
| M1-5 设置 | 热键、模式、Tab、主题、启动、路径、剪贴板警示与恢复 | M1-1 | 重启一致；警示与恢复可用 | 2.5 |
| M1-6 托盘 | 菜单、暂停/恢复、第二实例、退出 | M0-1 | 无残留 | 1.5 |
| M1-7 注入加固 | 区域标记包、长文本、UIA 降级、剪贴板序列 | M0-4 | 回归通过 | 2.5 |
| M1-8 UI Spec | 皮肤、深色、DPI、IME、托盘、截图 | M1-3/4/5 | §6.4 清单 | 1 |
| M1-9 打包 | 图标、版本、README、zip、签名脚本 | 全部 | 新机即用 | 1 |

**M1 模块合计 22 人日；缓冲 4 人日单列。总计 10 + 22 + 4 = 36 人日。**

## 9. 测试矩阵与发布门

### 9.1 计数

- 15 宿主格 × 8 场景 = 120
- Win10 追加 Chrome / VS Code × 8 = 16
- 专项 32 项 = 横切 6 + T1–T26
- **合计 168 项**

原 159 项编号不变，T18–T26 为 v4.1 追加。

### 9.2 分类与计数

每格标 `AUTO_INSERT / COPY_ONLY / UNSUPPORTED`，并记录：
`VERIFIED`、`IDENTITY_UNKNOWN`、`IDENTITY_MISMATCH`、`NON_EDITABLE`、`READ_ONLY`、`POSITION_UNVERIFIED`、`TIMEOUT`、`REGION_UNKNOWN`、`UIA_HWND_ZERO`、`ANCESTOR_VERIFIED`、`AUTO_INSERT`、`COPY_ONLY`。

自动写入成功率只统计 `AUTO_INSERT` 格。

### 9.3 横切与反例

**横切 6 项**

1. IME 组合态（面板内输入中文，Enter 不误选）。
2. 面板期间切到第三方窗口再选择。
3. Alt+Tab 后选择。
4. 150% DPI 双屏边缘定位。
5. 热键冲突与重设。
6. 连续 20 次热键/选择。

| # | 反例 | 合格结果 |
| --- | --- | --- |
| T1 | Chrome 同页 A → B | 不写 B；不确定只复制 |
| T2 | 面板出现后 DOM/控件重建 | 拒绝注入 |
| T3 | 提交时 Enter/Ctrl/Alt 仍按下 | 不自动发送 |
| T4 | 终端含换行 | 只复制，不执行 |
| T5 | `SendInput` 部分接受 | 报“可能部分输入”，不重试 |
| T6 | 剪贴板含图片/HTML 或被改 | 不宣称无损，不覆盖 |
| T7 | 热键后 UIA 延迟 300–500 ms | 捕获超时 → 只复制 |
| T8 | UIA 永久阻塞 >2 s | UI 不卡；进入只复制；旧快照不复用 |
| T9 | 同一 `Code.exe`：编辑器 vs 集成终端 | 终端多行只复制 |
| T10 | 浏览器网页终端（xterm） | 多行不注入 |
| T11 | 原生 EDIT + ValuePattern-only + 只读控件 | 后两者只复制 |
| T12 | 捕获超时后用户继续操作 | 不复用旧快照 |
| T13 | 通知/弹窗抢焦点 | 捕获作废或提交拒绝 |
| T14 | 模态对话框出现 | 不注入到对话框 |
| T15 | 同一应用切标签/切文档 | 身份不匹配即拒绝 |
| T16 | 注入中目标窗口销毁或程序退出 | `INPUT_PARTIAL_OR_UNKNOWN`，不重试 |
| T17 | 恢复失败（最小化、被占前台、激活受限） | `REJECTED_BEFORE_INPUT`，只复制 |
| T18 | Chrome 内层 UIA 输入框 `NativeWindowHandle=0` | 上溯成功则 VERIFIED；失败则 UNKNOWN、只复制；不得把 0 当作“不属于目标” |
| T19 | 原控件未变，但光标/选区在面板期间被宿主改动 | 可对比且不一致 → 拒绝；不可对比 → 记 `POSITION_UNVERIFIED` 并继续 |
| T20 | TextPattern 可用但 ValuePattern 缺失的只读控件 | 不得把属性缺失当可编辑；只复制 |
| T21 | UIA 元素销毁重建，RuntimeId 数值相同或 provider 返回失效 | 不得凭 ID 放行；`CompareElements` 失败即拒绝 |
| T22 | UIA 调用长期挂起，重复热键触发 | 线程数有上限；UI 可用；结果不串请求 |
| T23 | 终端/聊天文本无 CR/LF 但含 ESC、U+2028 等 | 只复制或明确拒绝 |
| T24 | 首次运行、JSON 不存在 | 正确落盘，重启后数据仍在 |
| T25 | 面板期间前台 A → B → A | 不因最终 HWND 相同而误判未切走 |
| T26 | COPY_ONLY 覆盖原剪贴板 | 有明确提示；纯文本可恢复；非文本原内容明确告知不可恢复 |

### 9.4 发布阻断项

1. `AUTO_INSERT` 格出现错误窗口或错误控件写入。
2. `COPY_ONLY` / `UNSUPPORTED` 格发生注入。
3. 聊天软件自动发送消息。
4. 终端执行短语中的命令（含嵌入式终端）。
5. Esc / 点击外部后原输入框改变。
6. 提权目标“看似成功、实际未写入”。
7. 面板 P95 >150 ms 或静置 60 s Working Set >35 MB。

## 10. 风险登记

| 风险 | 概率 | 影响 | 处置 |
| --- | --- | --- | --- |
| 0-HWND 上溯失败导致可用控件降级 | 中 | 中 | E3 统计；只在无法确认时降级 |
| UIA TextRange 位置对比不可靠 | 中 | 中 | E4 覆盖率；不可比则记 UNVERIFIED |
| 浏览器多行 V1 不可用 | 高 | 中 | 写进 README 与 UI；M2 再评估 |
| VS Code 只能复制多行 | 高 | 中 | E2 实测；失败即接受 |
| 捕获 60 ms 超时 | 中 | 中 | 预热；E3 测量 |
| UIA 卡顿/挂起 | 中 | 中 | 放弃线程上限 2；会话降级只复制 |
| 杀软误报 | 高 | 高 | 签名（询价）、无钩子、提交厂商 |
| IME 异常 | 低 | 高 | 原生 EDIT；M0 必测 |
| 部分输入 | 中 | 中 | 三态提示；不重试 |
| 工期超支 | 中 | 中 | M0 门禁；缓冲吸收；M1 可砍动效 |

**回滚**：迁移前备份；升级只换 EXE；旧 EXE 遇新 schema 只读导出。

## 11. 与 PRD 的差异（回写 PRD）

1. 默认 Unicode 受长度/区域/控制字符约束。
2. 终端与未知区域多行只复制。
3. 聊天多行只复制。
4. 上限 2,000 UTF-16 单元（≈2,000 个 BMP 汉字）。
5. Tab 默认 4 空格。
6. 剪贴板不承诺无损；启用前警示；显式恢复。
7. 回收站 P1，进 M1。
8. 10,000 上限按 UTF-16 单元。
9. ≥99% 分母 = `AUTO_INSERT` 格。
10. 新增反例 T1–T26。
11. 主题三态。
12. 指标口径固定。
13. 输入结果三态。
14. 控件身份校验为 P0。
15. 终端自动粘贴不在 V1。
16. 多行按聚焦区域，不按 exe。
17. 浏览器多行 V1 不做。
18. 捕获先于面板激活。
19. Document / ValuePattern-only / 只读控件不支持。
20. 用户规则只能收紧。
21. **位置承诺改为“原控件 + 提交时当前光标”**，不再承诺冻结字符偏移。
22. 含控制字符的内容只复制。
23. COPY_ONLY 会写剪贴板，必须提示并可恢复。

## 12. 待确认问题

1. ~~浏览器多行 V1 只复制~~：**已确认**。
2. ~~Excel 多行只复制~~：**已确认**。
3. ~~VS Code 多行可能只能复制~~：**已确认**。
4. 签名：公开分发前是否采购证书（需询价）。
5. 开源还是私有。
6. 基准机型号。
7. Windows 10 最低版本（建议 21H2+）。
8. **位置承诺改为“原控件 + 当前光标”**：确认这个产品口径。
