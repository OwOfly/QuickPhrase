# QuickPhrase 最终方案（final-plan v1）

- 汇总：DeepSeek-V4.1-Flash（主席）
- 日期：2026-11-18
- 状态：**待用户确认**（确认后进入 Phase 2 模块实施）
- 输入：`plan-vote/chair-deepseek.md`、`plan-vote/glm.md`、`plan-vote/kimi.md`、`plan-vote/qwen.md`
- 目标仓库：`D:\Code\AIProject\quickphrase`
- 规格来源：`prd-v1.1.md`（PRD V1.1）

---

## 1. 背景与目标

Windows 桌面全局快捷短语输入器：热键唤起面板 → 搜索选中 → 文本写入触发前的原输入位置。单机、离线、无账号。

**第一版只验证三件事**：短语能维护、随时能唤起、文字准确进入原位置。跨应用输入不可靠时，宁可不输入。

**交付形态**：免安装单 EXE（zip），数据在 `%LocalAppData%\QuickPhrase`，卸载/覆盖 EXE 不丢数据。

## 2. 范围与非目标

**M1（MVP）范围**

- 全局热键（默认 `Ctrl+Alt+Q`，可改、冲突提示）
- 快捷面板：搜索、分类筛选、键盘导航、置顶排序、空态/无结果
- 短语 CRUD、分类、置顶、回收站（软删除 30 天）
- 设置：热键录制、输入模式、按应用规则、主题（跟随系统/浅色/深色）、开机启动、数据路径
- 持久化：JSON + 原子写 + 5 份滚动备份 + 损坏恢复入口
- 导入/导出 JSON（冲突预览、不覆盖）
- 输入：Unicode 默认、剪贴板兼容模式、长文本降级、终端/聊天按宿主策略
- 托盘常驻、单实例、第二实例唤起
- 本地日志与诊断包导出（不含短语内容）

**非目标（V1 不做）**

云同步、团队协作、AI 改写、富文本/图片、缩写自动展开、批量宏、读取输入框内容、OCR、移动端、浏览器插件、密码保险箱、自动更新。

**声明不支持**：UAC 安全桌面、锁屏、独占全屏/反作弊游戏、提权目标窗口、跨会话（RDP 另一端）、无 UIA 且无 HWND 焦点的自绘控件。

### 2.1 隐私与安全边界

- 全程不联网；没有遥测、没有崩溃上传。
- 不安装键盘钩子，不采集击键历史，不读取输入框内容，不记录剪贴板历史。
- 日志只记元数据（宿主 exe、窗口类、失败码、字符数、耗时），**不记短语正文**。
- 数据只落 `%LocalAppData%\QuickPhrase`；导出由用户显式触发，路径自选。
- 不保存密码、Token、密钥类内容的口径写进首次引导与 README；密码框（`ES_PASSWORD` / UIA `IsPassword`）检测到即拒绝。
- 不申请管理员权限；检测到提权目标直接拒绝，不尝试绕过 UIPI。

## 3. 侦察结论（含实证）

| 事实 | 证据 | 影响 |
| --- | --- | --- |
| Chromium 系 `GetGUIThreadInfo.hwndFocus` 等于顶层窗口，`hwndCaret=NULL` | 2026-11-18 本机实测 Electron 应用：`Focus=0x40A80 [Chrome_WidgetWin_1]`，与 Foreground 相同，Caret NULL | HWND 无法定位浏览器内输入元素，必须叠加 UIA 或接受窗口级校验 |
| `SendInput` 被 UIPI 阻断时**失败**，但返回值与 `GetLastError` 都不说明原因是 UIPI | MS Learn `SendInput` 文档原文 | 只能用令牌完整性预检，不能靠返回值 |
| `SendInput` 不会重置键盘状态；事件串行插入，不与用户输入交错 | 同上文档 | 注入前等物理修饰键释放；单次调用优于分块 |
| WinForms/WPF + NativeAOT 非官方支持 | SDK `NETSDK1175`；社区需 `_SuppresswinformTrimError` 逃逸，COM 路径受限 | C# 备选不能按 12–18 MB 估算 |
| 依赖版本 | crates.io：`slint 1.19.0`、`windows 0.62.2` | 方案中的旧版本号已作废 |
| 本机工具链 | rustc 1.93.0-nightly、.NET 8.0.427、Node 26.13.0 | Rust 主路线可立即开工 |

## 4. 决策点表

| # | 决策 | 各方立场 | 裁决 | 理由 | 残余风险 |
| --- | --- | --- | --- | --- | --- |
| D1 | 语言/运行时 | 4/4 选 Rust | **Rust + `windows` 0.62** | 体积/内存/无运行时依赖全达标；C# 自包含 60 MB+ | Rust UI 开发量高 |
| D2 | UI 实现 | glm: Slint；kimi/qwen: 原生控件 + D2D；主席: Slint | **原生控件优先**：搜索框 `EDIT`、列表 `ListView(LVS_OWNERDRAWFIXED)`、编辑区 `RICHEDIT`，面板装饰 Direct2D/DirectWrite | 中文 IME、无障碍、键盘行为白送；Slint 的 IME 组合态是核心风险，不该拿产品红线赌框架 | 视觉与 HTML 原型有差距，需自绘边框/圆角/阴影 |
| D3 | 面板视觉 | 三家一致：不要真亚克力 | **DWM 圆角 + 系统材质（Win11）+ 自绘阴影；Win10 退纯色** | 与 ≤35 MB 目标兼容 | Win10 观感降级 |
| D4 | 默认注入 | 4/4 Unicode | **`SendInput` + `KEYEVENTF_UNICODE`，UTF-16 码元逐个 down/up；单次调用** | 不占剪贴板 | 部分控件丢弃合成事件 |
| D5 | 换行 | 4/4 用 CR，禁止 VK_RETURN | **统一归一为 CR(0x0D)**；`\r\n`→`\r`，单独 `\n`→`\r` | 经典 EDIT 忽略 LF；RichEdit 认 CR；VK_RETURN 会在聊天软件触发发送 | 个别宿主 CR 不换行 → 按应用改粘贴 |
| D6 | Tab | glm: 直接发；kimi: 拒绝且不得静默替换；qwen: 实测 | **默认替换为 4 空格，且在设置中显式展示该规则；可切“保留 Tab（仅粘贴模式）”** | 直接发 `0x09` 会切焦点/触发补全；静默替换不可接受，所以做成可见设置 | 代码片段缩进语义变化 |
| D7 | 长文本 | 阈值 1k/2k/2k/5k | **≤2,000 UTF-16 单元走 Unicode；>2,000 提示并可选粘贴模式；10,000 为硬上限** | 单次注入过长会长时间占用输入流；2k 约 4,000 事件，可接受 | 用户坚持 Unicode 时仍可能慢 |
| D8 | 终端 | 4/4 禁止 Unicode 换行 | **终端宿主硬黑名单：含换行即拒绝 Unicode，只允许“复制”或用户显式粘贴模式** | 换行=执行命令，风险不对称 | 终端粘贴受 bracketed paste 支持差异 |
| D9 | 聊天软件 | glm/qwen: 黑名单；kimi: 未验证只复制 | **微信/QQ/钉钉/飞书默认单行插入；多行先按宿主规则表，未验证宿主只复制** | 防止误发送消息 | 宿主升级后行为变化，需版本化规则表 |
| D10 | 焦点校验 | glm: HWND+pid；qwen: HWND+创建时间；kimi: 加 UIA RuntimeId | **三层**：① HWND + PID + 进程创建时间 + 前台代数；② `GetGUIThreadInfo.hwndFocus` 比对；③ UIA 焦点元素（50 ms 预算，三态 TEXT / NON_TEXT / UNKNOWN） | ①防窗口复用与切窗；②覆盖原生控件；③覆盖浏览器/Electron | UNKNOWN 时只能窗口级保证，必须记日志并给复制退路 |
| D11 | UIA 三态策略 | kimi: 无 UIA 身份就拒绝；其余未定 | **TEXT→注入；NON_TEXT→只复制；UNKNOWN→仅当宿主在已验证矩阵内才注入，否则只复制** | 兼顾可用率与安全，不搞一刀切 | 未验证宿主体验偏保守 |
| D12 | 提权检测 | 4/4 预检令牌 | **`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `TokenIntegrityLevel` 比较；失败或更高即拒绝** | 返回值不可信（见 §3） | 受保护进程可能查询失败 |
| D13 | 剪贴板恢复 | glm/qwen: 尽力恢复；kimi: 不自动恢复 | **只备份 `CF_UNICODETEXT`；粘贴后比对 `GetClipboardSequenceNumber`，被外部改动则绝不覆盖，提供“恢复剪贴板”按钮** | 无损恢复不可能，PRD 也已免责 | 用户可能丢原剪贴板内容 |
| D14 | 数据模型 | 三家接近 | **`categories[]` 独立实体 + `snippets[]` + `settings` + `appRules[]` + `schemaVersion` + `revision`** | 分类可重命名/排序；revision 检测外部改动 | — |
| D15 | 回收站 | PRD 入口有、优先级无 | **M1 实现软删除（`deletedAt`，30 天清理），UI 提供回收站** | 原型已有入口，不能留死链接 | 需额外 1 人日 |
| D16 | 排序/检索 | 三家打分不同 | **排序：pinned → lastUsedAt(NULL 末) → updatedAt → sortOrder → id；检索：标题精确 400 / 前缀 300 / 子串 200 / 正文 100，同分套默认排序，仅渲染前 50** | 可验收、可复现 | 拼音检索放 M2 |
| D17 | 指标口径 | kimi 提出定义缺失 | **空闲内存 = 启动后静置 60 s 的 Working Set；延迟取 100 次 P95；固定基准机（本机 i7-12700 级）记录环境** | 否则无法验收 | 用户机器差异 |
| D18 | 工期 | 23 / 27 / 31.5 / 39 人日 | **M0 8 人日 + M1 21 人日 + 4 人日缓冲 = 33 人日** | 取三家交集偏保守；AI 辅助可压缩 | 排期风险 |

## 5. 架构

### 5.1 进程与线程

单进程、单实例（`CreateMutexW` + 隐藏消息窗口 + `WM_COPYDATA`）。

| 线程 | 职责 |
| --- | --- |
| UI 线程 | 消息循环、面板、管理窗口、托盘、`WM_HOTKEY` |
| 注入线程 | 快照校验、`SendInput`、剪贴板序列、超时控制；不阻塞 UI |
| 存储线程 | 原子写、备份轮转、导入导出 |

热键回调只做快照（<1 ms）与唤起，不在 UI 线程做注入。

### 5.2 模块与依赖

```
ui (panel, manager, settings)
  ├─ store (schema, atomic write, backup, migration, import/export)
  ├─ search (blob, pinyin, scoring)
  ├─ context (snapshot, restore, verify, uia)
  ├─ inserter (unicode, clipboard, policy)
  ├─ hotkey
  ├─ tray
  └─ diagnostics (log, dump)
```

### 5.3 依赖 crate

`windows 0.62`（Win32_UI_Input_KeyboardAndMouse / UI_WindowsAndMessaging / System_Threading / System_DataExchange / UI_Accessibility / Graphics_Direct2D / Graphics_Dwrite）、`serde`、`serde_json`、`uuid`、`time`、`thiserror`。拼音在 M2 评估体积后引入。

## 6. 数据契约（schemaVersion = 1）

```jsonc
// data.json
{
  "schemaVersion": 1,
  "revision": 42,
  "categories": [
    { "id": "uuid", "name": "工作沟通", "sortOrder": 0, "createdAt": "2026-11-18T02:00:00Z", "updatedAt": "..." }
  ],
  "snippets": [
    {
      "id": "uuid", "title": "项目进度回复", "content": "您好……",
      "categoryId": "uuid|null", "pinned": true, "sortOrder": 0,
      "createdAt": "...", "updatedAt": "...",
      "useCount": 3, "lastUsedAt": "...|null",
      "deletedAt": "...|null",
      "searchBlob": "标题 正文 小写 首字母"
    }
  ]
}
```

```jsonc
// settings.json
{
  "schemaVersion": 1,
  "hotkey": "Ctrl+Alt+Q",
  "insertionMode": "unicode",        // unicode | clipboard | perApp
  "tabPolicy": "spaces4",            // spaces4 | keep
  "theme": "system",                 // system | light | dark
  "startup": false,
  "paused": false,
  "dataDir": null
}
```

```jsonc
// app-rules.json —— 内置只读基线 + 用户覆盖
[{ "exe": "WindowsTerminal.exe", "mode": "clipboard", "newline": "block", "source": "builtin" }]
```

**规则**

- 标题 1–80 UTF-16 单元；正文 1–10,000 UTF-16 单元（非字符数）。
- 保存：写同目录 `.tmp` → `FlushFileBuffers` → 重新解析校验 → `ReplaceFileW`（首写用 `MoveFileExW`）→ 轮转 5 份 `data-<ts>.json`。写失败不动原文件。
- 启动：主文件损坏 → 依次尝试最近 5 份备份 → 全部失败则进入只读修复模式，不覆盖。
- 迁移：`schemaVersion` 高于程序 → 拒载并提示；低 → 先备份，再逐版迁移。
- 导入：≤20 MB、≤10,000 条；按 `id` 匹配：内容一致跳过，内容不同要求“跳过/另存副本（新 UUID + 标题后缀）”，禁止覆盖。
- 软删除 30 天后物理清理；恢复保留原 `id` 与分类。

### 6.1 内置宿主规则表（V1 基线）

| 宿主（进程名） | 输入模式 | 换行 | Tab | 备注 |
| --- | --- | --- | --- | --- |
| `WindowsTerminal.exe` / `OpenConsole.exe` / `conhost.exe` / `mintty.exe` / `putty.exe` | 仅粘贴/复制 | **禁止** | 转空格 | Unicode 换行 = 执行命令 |
| `cmd.exe` / `powershell.exe` / `pwsh.exe` | 仅粘贴/复制 | **禁止** | 转空格 | 同上 |
| `WeChat.exe` / `QQ.exe` / `DingTalk.exe` / `Feishu.exe` | Unicode | 默认单行；多行先按实测开关 | 转空格 | 防误发送 |
| `Code.exe`（VS Code） | Unicode | CR | 转空格 | 编辑器内 Tab 会触发补全 |
| `WINWORD.EXE` / `EXCEL.EXE` | Unicode | CR | 转空格 | — |
| `chrome.exe` / `msedge.exe` / `firefox.exe` | Unicode（需 UIA=TEXT） | CR | 转空格 | UIA 不可用则只复制 |
| `explorer.exe`（重命名框） | Unicode | 不适用 | 不适用 | 单行，含换行即拒绝 |
| 其它未登记宿主 | Unicode（单行）/ 复制 | 拒绝多行 | 转空格 | 首次遇到时提示"该应用未验证" |

规则文件带 `version` 字段；用户可在设置里覆盖单条规则，覆盖项标记 `source: user`。宿主升级导致行为变化时，只更新规则包，不改代码。

## 7. 状态机

```
IDLE
 └─ WM_HOTKEY → CAPTURED（快照：hwnd/pid/创建时间/hwndFocus/hwndCaret/前台代数/UIA 元素）
      └─ 面板可见 → PANEL_VISIBLE
           ├─ Esc / 点击外部 / 再次热键 → CANCELLED（零写入）
           └─ Enter/点击 → RESTORING
                ├─ 校验失败 → FAILED_SAFE（不注入，提示 + 复制）
                └─ 校验通过 → INSERTING
                     ├─ SendInput 返回数不符 → FAILED_SAFE
                     └─ 完成 → IDLE（useCount+1, lastUsedAt=now）
```

**失败判据（任一成立即 FAILED_SAFE）**

`IsWindow(target)` 假；PID 或进程创建时间不符；前台代数在面板期间变化；300 ms 内 `GetForegroundWindow() != target`；`hwndFocus` 非 target 后代；UIA=NON_TEXT；令牌完整性高于自身；`SendInput` 返回数 ≠ 请求数；注入中前台变化。

**FAILED_SAFE 行为**：不发送任何字符；轻提示失败原因；提供“复制文本”和“返回面板重选”；写日志（只记元数据，不记短语内容）。

## 8. 实施计划

### M0 可行性（8 人日）

| 模块 | 内容 | 验收 | 人日 |
| --- | --- | --- | ---: |
| M0-1 骨架 | cargo 工程、DPI manifest、单实例、托盘、`RegisterHotKey`、滚动日志 | 冷启动 ≤1.5 s；退出释放热键；重复启动唤起首个实例 | 1.5 |
| M0-2 快照 | HWND/PID/创建时间/`GetGUIThreadInfo`/前台代数 | Alt+Tab、关窗、切屏后选择均不误投 | 1.5 |
| M0-3 面板 | 原生窗口 + `EDIT` + `ListView`，3 条硬编码，↑↓/Enter/Esc，多屏定位 | 100 次唤起 P95 ≤150 ms；125/150/200% 不越界 | 1.5 |
| M0-4 注入 | Unicode、CR、Tab、代理对、提权预检、单次调用、返回数校验 | 12 宿主 × 8 场景矩阵通过；错误目标 0 | 2.5 |
| M0-5 兼容矩阵 | 手工执行 + 记录 + 失败归因 | 报告含每格结论与降级建议；同时记录 EXE 体积、静置 60 s Working Set、100 次唤起 P95 | 1 |

**M0 出口**：核心宿主成功写入 ≥99/100；错误窗口/控件写入 = 0；聊天无自动发送；终端无命令执行；Esc 零修改。任一不满足 → 停，回方案层。

### M1 MVP（21 人日）

| 模块 | 内容 | 依赖 | 验收 | 人日 |
| --- | --- | --- | --- | ---: |
| M1-1 存储 | schema、原子写、备份、迁移、损坏恢复 | M0 | 故障注入 100 次无空库覆盖 | 3.5 |
| M1-2 检索 | searchBlob、打分、置顶排序、高亮 | M1-1 | 1,000 条 P95 ≤50 ms | 2 |
| M1-3 面板完整 | 分类栏、无结果/空库、设置入口、失败态、动效开关 | M0-3 | PRD 14.3 全部状态可走通 | 3 |
| M1-4 管理窗口 | 三栏、CRUD、分类、回收站、校验、未保存拦截 | M1-1/2 | 新建/编辑/删除/恢复/导入导出全通 | 5 |
| M1-5 设置 | 热键录制与冲突、模式、Tab 策略、主题、启动、路径 | M1-1 | 重启后状态一致 | 2 |
| M1-6 托盘与生命周期 | 菜单、暂停/恢复、第二实例唤起、退出清理 | M0-1 | 暂停状态持久化；退出无残留 | 1.5 |
| M1-7 注入加固 | 宿主规则表、长文本降级、UIA 三态、剪贴板序列保护 | M0-4 | 矩阵回归通过 | 2 |
| M1-8 打包 | 图标、版本信息、manifest、README、免安装 zip、签名脚本 | 全部 | 新机解压即用；体积/内存达标 | 1 |
| 缓冲 | 集成、回归、性能基线 | — | — | 4 |

### 关键路径

`M0-2 → M0-4 → M0-5` 是唯一关键路径。M0-3 面板可以并行。M0 未过不做 M1。

## 9. 测试矩阵（M0 阻断口径）

12 宿主格：记事本（Win11）、记事本（Win10）、Chrome `input`、Chrome `textarea`、Chrome `contenteditable`、Edge `textarea`、VS Code、Word、微信、Windows Terminal、cmd（conhost）、管理员记事本。
另加 2 组：资源管理器重命名、钉钉/飞书（任选其一）。

8 场景：中文单行、英文+全角符号、CR 多行、Tab 缩进、Emoji、选区替换、光标中间插入、2,000 字长文本。
横切 6 项：IME 组合态、面板期间切窗、Alt+Tab、150% DPI 双屏边缘、热键冲突、连按 20 次。

合计约 112 格（12×8 为主，另加横切）。Windows 10 只跑记事本/Chrome/VS Code 三格。

**发布阻断项（任一即停）**

1. 文本进入与目标不同的顶层窗口或不同输入框。
2. 聊天软件因注入自动发送消息。
3. 终端执行了注入内容中的命令。
4. Esc / 点击外部后原输入框内容改变。
5. 提权目标窗口出现“看似成功、实际未写入”。
6. 面板 P95 >150 ms 或空闲内存 >35 MB（按 §4 D17 口径）。

## 10. 风险登记

| 风险 | 概率 | 影响 | 处置 |
| --- | --- | --- | --- |
| 浏览器内错字段写入 | 中 | 高 | UIA 三态 + 失败退路；UNKNOWN 仅限已验证宿主 |
| 杀软误报 | 高 | 高 | Authenticode 签名、无钩子、提交微软/360/火绒；无签名版仅内测 |
| 中文 IME 在自绘控件上异常 | 低（原生 EDIT） | 高 | 原生控件规避；M0 必测组合态 |
| 终端粘贴模式差异 | 中 | 中 | 按宿主规则表；不支持则只复制 |
| 长文本注入慢/丢字 | 中 | 中 | 2,000 上限 + 返回数校验 + 不自动重试 |
| 规则表随宿主升级失效 | 中 | 中 | 规则表带版本号，M2 做社区可更新（离线包） |
| Win10 视觉降级 | 高 | 低 | 纯色 + 阴影，功能一致 |
| 工期超支 | 中 | 中 | M0 出口门禁；M1 按模块验收，可砍主题/动效 |

**回滚**：数据 schema 迁移前强制备份；版本升级只替换 EXE；回滚 = 换回旧 EXE + 用备份恢复数据。不做自动更新。

## 11. 与 PRD 的差异（需回写 PRD）

1. 默认 Unicode 改为**带长度/宿主/控制字符约束的 Unicode**，不再无条件默认。
2. 终端多行改为**自动拒绝并转复制**，不等用户手动切换模式。
3. 聊天软件多行改为**按宿主规则表**，未验证宿主只复制。
4. 长文本硬上限 2,000 UTF-16 单元走 Unicode，2,001–10,000 只允许粘贴或复制。
5. Tab 默认替换 4 空格，但必须在设置中可见、可改。
6. 剪贴板兼容模式**不承诺自动恢复**，改为“检测到外部改动不覆盖 + 显式恢复按钮”。
7. 回收站定级 P1，进入 M1，`deletedAt` 进 schema v1。
8. 10,000 上限明确为 UTF-16 code units。
9. ≥99% 成功率的分母 = 声明支持的矩阵格，排除声明不支持项。
10. 新增用例：注入期间用户继续按键；UIA 返回 NON_TEXT；目标进程被替换（创建时间不符）。
11. 主题三态（跟随系统/浅色/深色）写入设置与验收。
12. 空闲内存、P95 延迟给出测量口径（静置 60 s / 100 次）。

## 12. 待确认问题

1. 代码签名预算：OV/EV 证书约 ¥1,500–4,000/年。不签名，国内杀软误报概率高。
2. 开源还是私有：影响 license、README、issue 模板与发布方式。
3. UIA 依赖是否接受：拒绝则浏览器/Electron 场景更保守（多数只能复制）。
4. 基准机：性能门禁跑在哪台机器上（当前开发机型号）。
5. Windows 10 最低版本：建议 21H2+；更低版本需额外验证。
6. 是否需要中文/英文双语界面：默认只做简中。
