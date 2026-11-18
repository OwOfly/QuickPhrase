# QuickPhrase 最终方案（final-plan v2）

- 汇总：DeepSeek-V4.1-Flash（主席）
- 日期：2026-11-18
- 状态：**待用户确认 v2**（v1 已被独立复核判定 REJECT，不得作为实施基线）
- 修订依据：`QuickPhrase_final-plan_v1_独立复核报告.md`（B1–B3、M1–M10）
- 目标仓库：`D:\Code\AIProject\quickphrase`
- 规格来源：`prd-v1.1.md`
- 历史版本：`final-plan-v1.md`

## 0. v2 修订对照

| 复核项 | v1 问题 | v2 处置 | 落点 |
| --- | --- | --- | --- |
| B1 | UIA 三态只判断“能否收文字”，不判断“是不是同一个控件” | 增加控件身份比对（`CompareElements` / RuntimeId）与插入点存在性检查；无法确认身份即只复制 | §4 D10/D11、§7 G6/G7 |
| B2 | 调用过 `SendInput` 后仍宣称“零写入” | 拆分 `REJECTED_BEFORE_INPUT` / `INPUT_PARTIAL_OR_UNKNOWN` / `API_EVENTS_ACCEPTED` | §7 |
| B3 | 终端“显式粘贴模式”仍会执行命令 | MVP 终端含换行一律只复制，不代替用户粘贴；高风险粘贴推迟到 M2 且默认关闭 | §4 D8、§6.1 |
| M1 | UIA 快照与“<1ms”冲突；夺焦点后取到面板自身 | HWND 快照同步、UIA 异步常驻 MTA；提交前补齐，不阻塞首帧 | §5.1、§5.4 |
| M2 | “前台代数变化即失败”未排除自身面板 | 定义允许序列：原应用 → 面板 → 原应用；其余变化作废 | §7 G3 |
| M3 | 只写“300 ms 内恢复” | 明确激活限制、失败退路、不盲目点击/输入 | §7 G4 |
| M4 | 修饰键与 Enter 释放时序未定义 | 提交前等待 Ctrl/Alt/Shift/Win/Enter 物理释放，超时拒绝 | §7 G10 |
| M5 | 剪贴板“备份纯文本”与“无损恢复”语义矛盾 | 明确不承诺无损；仅纯文本快照 + 显式恢复；默认模式不碰剪贴板 | §4 D13 |
| M6 | 成功率分母未分类，矩阵计数不一致 | 三类期望（AUTO / COPY_ONLY / UNSUPPORTED）+ 重新计数 | §9 |
| M7 | 仅 data.json 有备份协议 | 所有可变文件统一原子写/轮转/损坏恢复；补 appRules 写入与回滚 | §6 |
| M8 | 原生控件路线缺可验收的 UI 标准 | 新增 UI Spec（尺寸、token、状态、DPI、截图验收） | §6.2 |
| M9 | 工期相加错误；UTF-16 单元说明错误 | M1 = 21 人日；总数 34；2,000 单元约 2,000 个 BMP 汉字 | §8 |
| M10 | 注入过程边界未覆盖 | 状态机区分输入前后；新增 T1–T6 反例与边界测试 | §7、§9 |

**对复核报告的两处口径修正**

1. 报告 M8 写“快捷面板 480×360”，与 PRD 14.3 的 520×420 冲突。v2 以 PRD 为准。
2. 报告 M1 要求 UIA 快照“在面板夺焦点前完成”。方向正确，但不得阻塞首帧；v2 改为“HWND 同步 + UIA 异步，提交前补齐，超时降级”。

---

## 1. 背景与目标

Windows 桌面全局快捷短语输入器：热键唤起面板 → 搜索选中 → 文本写入触发前的原输入位置。单机、离线、无账号。

**第一版只验证三件事**：短语能维护、随时能唤起、文字准确进入原位置。跨应用输入不可靠时，宁可不输入。

**交付形态**：免安装单 EXE（zip），数据在 `%LocalAppData%\QuickPhrase`。

## 2. 范围与非目标

**M1（MVP）范围**

- 全局热键（默认 `Ctrl+Alt+Q`，可改、冲突提示）
- 快捷面板：搜索、分类筛选、键盘导航、置顶排序、空态/无结果
- 短语 CRUD、分类、置顶、回收站（软删除 30 天）
- 设置：热键录制、输入模式、按应用规则、主题（跟随系统/浅色/深色）、开机启动、数据路径
- 持久化：JSON + 原子写 + 5 份滚动备份 + 损坏恢复入口
- 导入/导出 JSON（冲突预览、不覆盖）
- 输入：Unicode 默认、剪贴板兼容模式（用户显式启用）、长文本降级、按宿主策略
- 托盘常驻、单实例、第二实例唤起
- 本地日志与诊断包导出（不含短语内容）

**非目标（V1 不做）**

云同步、团队协作、AI 改写、富文本/图片、缩写自动展开、批量宏、读取输入框内容、OCR、移动端、浏览器插件、密码保险箱、自动更新、终端高风险自动粘贴。

**声明不支持**：UAC 安全桌面、锁屏、独占全屏/反作弊游戏、提权目标窗口、跨会话（RDP 另一端）、无 UIA 且无 HWND 焦点的自绘控件。

### 2.1 隐私与安全边界

- 全程不联网；无遥测、无崩溃上传。
- 不安装键盘钩子，不采集击键历史，不读取输入框内容，不记录剪贴板历史。
- 使用 `SetWinEventHook(EVENT_SYSTEM_FOREGROUND)` 观察前台窗口切换；这是只读事件订阅，不是键盘钩子，不拦截输入。
- 日志只记元数据（宿主 exe、窗口类、失败码、字符数、耗时、UIA 状态），**不记短语正文**。
- 数据只落 `%LocalAppData%\QuickPhrase`；导出由用户显式触发。
- 密码框（`ES_PASSWORD` / UIA `IsPassword`）检测到即拒绝。
- 不申请管理员权限；检测到提权目标直接拒绝。

## 3. 侦察结论（含实证）

| 事实 | 证据 | 影响 |
| --- | --- | --- |
| Chromium 系 `GetGUIThreadInfo.hwndFocus` 等于顶层窗口，`hwndCaret=NULL` | 2026-11-18 本机实测 Electron 应用：`Focus=0x40A80 [Chrome_WidgetWin_1]`，与 Foreground 相同，Caret NULL | HWND 只能定位窗口，必须用 UIA 定位具体控件 |
| UIA 焦点元素可区分同一窗口内的不同输入框，但需要身份比对 | Windows UIA `CompareElements` / `RuntimeId` | 仅“能收文字”不够，必须证明“还是快照时那个元素” |
| UIA 元素指针/比较有线程亲和性 | UIA 线程模型文档 | 快照与复验必须在同一 MTA 工作线程 |
| `SendInput` 被 UIPI 阻断时会失败，但返回值与 `GetLastError` 不说明原因 | MS Learn `SendInput` 文档 | 只能靠令牌完整性预检 |
| `SendInput` 不重置键盘状态；事件串行插入 | 同上 | 提交前等物理键释放；单次调用 |
| WinForms/WPF + NativeAOT 非官方支持 | `NETSDK1175` | C# 备选不能按 12–18 MB 估算 |
| 依赖版本 | crates.io：`slint 1.19.0`、`windows 0.62.2` | 旧版本号作废 |
| 本机工具链 | rustc 1.93.0-nightly、.NET 8.0.427 | Rust 主路线可立即开工 |

## 4. 决策点表

| # | 决策 | 裁决 | 理由 | 残余风险 |
| --- | --- | --- | --- | --- |
| D1 | 语言/运行时 | **Rust + `windows` 0.62** | 体积/内存/无运行时依赖达标 | UI 开发量高 |
| D2 | UI 实现 | **原生控件优先**：`EDIT` 搜索框、`ListView(LVS_OWNERDRAWFIXED)` 列表、`RICHEDIT` 编辑区；Direct2D/DirectWrite 只做装饰 | 中文 IME、无障碍、键盘行为白送 | 视觉与原型有差距，需 UI Spec 约束 |
| D3 | 面板视觉 | **DWM 圆角 + 系统材质（Win11）+ 自绘阴影；Win10 纯色** | 与 ≤35 MB 兼容 | Win10 观感降级 |
| D4 | 默认注入 | **`SendInput` + `KEYEVENTF_UNICODE`，UTF-16 码元逐个 down/up，单次调用** | 不占剪贴板 | 部分控件丢弃合成事件 |
| D5 | 换行 | **统一 CR(0x0D)**；`\r\n`→`\r`，单独 `\n`→`\r`；禁止 `VK_RETURN` | EDIT 忽略 LF；VK_RETURN 会触发发送 | 个别宿主 CR 不换行 |
| D6 | Tab | **默认替换 4 空格，设置中可见可改；保留 Tab 仅用于粘贴模式** | 避免焦点跳转/补全 | 缩进语义变化 |
| D7 | 长文本 | **≤2,000 UTF-16 单元走 Unicode**（约 2,000 个 BMP 汉字；Emoji 占 2 单元）；>2,000 提示后仅允许粘贴或复制 | 单次注入过长风险高 | 用户坚持时的体验 |
| D8 | 终端 | **含任何换行符一律只复制，不代替用户粘贴**；单行允许 Unicode；M2 才评估默认关闭的高风险粘贴 | 换行=执行命令，风险不对称 | 开发者多行 SQL 场景受限 |
| D9 | 聊天软件 | **单行允许 Unicode；多行默认只复制**，M0 验证通过的宿主才开放多行 | 防误发送 | 宿主升级后行为变化 |
| D10 | 焦点校验 | **四层**：① HWND + PID + 创建时间 + 前台代数；② `hwndFocus`；③ UIA 元素身份（`CompareElements`/RuntimeId）；④ 控件可编辑 + 非密码 + 有插入点 | 逐层排除错窗口、错控件、无光标 | UIA 不可用时只能复制 |
| D11 | UIA 三态 | **TEXT_MATCH / TEXT_DIFFERENT / UNKNOWN**。只有 TEXT_MATCH 才注入；TEXT_DIFFERENT 与 UNKNOWN 只复制 | 身份匹配优先于“能收文字” | 见 D19 的覆盖率依赖 |
| D12 | 提权检测 | `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `TokenIntegrityLevel`；失败或更高即拒绝 | 返回值不可信 | 受保护进程查询失败 |
| D13 | 剪贴板 | 默认模式不碰剪贴板；兼容模式仅备份 `CF_UNICODETEXT`，**不承诺无损恢复**；序列号被外部改动则绝不覆盖；提供“恢复先前纯文本”按钮 | 无损恢复不可能 | 用户可能丢失原剪贴板 |
| D14 | 数据模型 | `categories[]` + `snippets[]` + `settings` + `appRules[]` + `schemaVersion` + `revision` | 分类可重命名/排序 | — |
| D15 | 回收站 | M1 实现软删除（30 天清理） | 原型已有入口 | 1 人日 |
| D16 | 排序/检索 | pinned → lastUsedAt(NULL 末) → updatedAt → sortOrder → id；标题精确 400 / 前缀 300 / 子串 200 / 正文 100；仅渲染前 50 | 可验收 | 拼音检索 M2 |
| D17 | 指标口径 | 空闲内存 = 静置 60 s Working Set；延迟 100 次 P95；固定基准机 | 否则无法验收 | 机器差异 |
| D18 | 工期 | **M0 9 + M1 21 + 缓冲 4 = 34 人日** | 见 §8 明细 | 排期风险 |
| D19 | UIA 覆盖率依赖 | UIA 身份校验列为 M0 核心验证；若 Chrome/Edge 的 UIA 身份命中率 <80%，产品行为改为“浏览器默认只复制”，并回写 PRD | 不在方案阶段写死产品行为 | 可用性下降 |
| D20 | UI 验收 | 以 §6.2 UI Spec 为准，截图验收；不得以“原生控件受限”交付未设计界面 | 视觉是产品的一部分 | 还原度有限 |

## 5. 架构

### 5.1 进程与线程

单进程、单实例（`CreateMutexW` + 隐藏消息窗口 + `WM_COPYDATA`）。

| 线程 | 职责 | 约束 |
| --- | --- | --- |
| UI 线程 | 消息循环、面板、管理窗口、托盘、`WM_HOTKEY` | 热键处理只做同步快照，<2 ms |
| 注入线程 | 恢复、校验、`SendInput`、剪贴板序列、超时 | 不阻塞 UI |
| UIA 线程（MTA，常驻） | `GetFocusedElement`、`CompareElements`、属性读取 | 所有 UIA 调用只在此线程；COM 对象不跨线程 |
| 存储线程 | 原子写、备份轮转、导入导出 | 不与 UI 共享文件句柄 |

**UIA 时序**：`WM_HOTKEY` → 同步快照（HWND/pid/创建时间/hwndFocus）→ 立即向 UIA 线程投递快照任务 → 显示面板（不等 UIA）→ 用户提交时若 UIA 结果未就绪，再等剩余预算（自热键起总计 400 ms）→ 超时按 UNKNOWN。面板首帧不受 UIA 影响。

### 5.2 模块与依赖

```
ui (panel, manager, settings)
  ├─ store (schema, atomic write, backup, migration, import/export)
  ├─ search (blob, scoring)
  ├─ context (snapshot, restore, verify, uia_client)
  ├─ inserter (unicode, clipboard, policy)
  ├─ hotkey
  ├─ tray
  └─ diagnostics (log, dump)
```

### 5.3 依赖 crate

`windows 0.62`（Win32_UI_Input_KeyboardAndMouse / UI_WindowsAndMessaging / System_Threading / System_DataExchange / UI_Accessibility / Graphics_Direct2D / Graphics_Dwrite / System_Com）、`serde`、`serde_json`、`uuid`、`time`、`thiserror`。拼音在 M2 评估后引入。

### 5.4 UIA 客户端约束

- 单例 `CUIAutomation`，创建并只在一个常驻 MTA 线程使用。
- 快照返回：元素指针 + RuntimeId（若支持）+ ControlType + `IsPassword` + `IsTextPatternAvailable` + `TextPattern.GetSelection()` 的 range 数量。
- 复验：同一线程调用 `CompareElements(snapshot, current)`；元素失效（`UIA_E_ELEMENTNOTAVAILABLE`）直接判 `TEXT_DIFFERENT`。
- 单次调用不设“硬取消”（COM 调用不可中断）；超时只放弃结果，线程继续服务下一次。
- UIA 线程崩溃/卡死：降级为 UNKNOWN，产品仍可用（只复制）。

## 6. 数据契约（schemaVersion = 1）

### 6.1 文件与备份

| 文件 | 内容 | 写入 | 备份 |
| --- | --- | --- | --- |
| `data.json` | `schemaVersion`/`revision`/`categories`/`snippets` | 原子写 | 5 份 `data-<ts>.json` |
| `settings.json` | 热键/模式/Tab/主题/启动/暂停/dataDir | 原子写 | 5 份 `settings-<ts>.json` |
| `rules-user.json` | 用户覆盖的宿主规则 | 原子写 | 5 份 |
| `logs/qp-<date>.log` | 元数据日志 | 追加 | 3 × 500 KB 轮转 |

所有可变文件统一：写同目录 `.tmp` → `FlushFileBuffers` → 重新解析校验 → `ReplaceFileW`（首写 `MoveFileExW`）。任一文件损坏 → 按时间倒序尝试备份 → 全部失败进入只读修复模式，不覆盖。

### 6.2 结构

```jsonc
// data.json
{
  "schemaVersion": 1,
  "revision": 42,
  "categories": [
    { "id": "uuid", "name": "工作沟通", "sortOrder": 0, "createdAt": "...", "updatedAt": "..." }
  ],
  "snippets": [
    {
      "id": "uuid", "title": "项目进度回复", "content": "您好……",
      "categoryId": "uuid|null", "pinned": true, "sortOrder": 0,
      "createdAt": "...", "updatedAt": "...",
      "useCount": 3, "lastUsedAt": "...|null", "lastInsertStatus": "accepted|partial|rejected|null",
      "deletedAt": "...|null",
      "searchBlob": "小写标题 + 换行 + 小写正文"
    }
  ]
}
```

```jsonc
// settings.json
{
  "schemaVersion": 1,
  "hotkey": "Ctrl+Alt+Q",
  "insertionMode": "unicode",        // unicode | clipboard
  "tabPolicy": "spaces4",            // spaces4 | keepInPasteOnly
  "theme": "system",                 // system | light | dark
  "startup": false,
  "paused": false,
  "dataDir": null
}
```

```jsonc
// rules-user.json
{ "version": 1, "rules": [
  { "exe": "Code.exe", "mode": "unicode", "newline": "cr", "tab": "spaces4", "source": "user" }
] }
```

**规则**

- 标题 1–80 UTF-16 单元；正文 1–10,000 UTF-16 单元。
- `searchBlob` v1 只含标题+正文小写；拼音首字母放 M2 的可选字段 `initials`，不改 schemaVersion。
- 导入：≤20 MB、≤10,000 条；按 `id` 匹配：一致跳过，不同要求“跳过/另存副本（新 UUID + 标题后缀）”，禁止覆盖。
- 迁移：`schemaVersion` 高于程序 → 只读模式 + 提示升级；迁移前写 `pre-migrate-v<N>-<ts>.json`。
- 回滚：旧 EXE 遇到新 schema 只读导出，不写入；README 写明“降级前先导出”。
- 软删除 30 天后物理清理；恢复保留原 `id` 与分类。
- 规则优先级：用户覆盖 > 内置宿主规则 > 全局 `insertionMode` > 默认 Unicode。安全硬规则（终端换行、密码框、提权目标、UIA 身份不匹配）**不允许用户覆盖**。

### 6.3 内置宿主规则表

| 宿主（进程名） | 模式 | 换行 | 条件 |
| --- | --- | --- | --- |
| `WindowsTerminal.exe` / `OpenConsole.exe` / `conhost.exe` / `mintty.exe` / `putty.exe` | 含换行 → `COPY_ONLY`；单行 → Unicode | **禁止注入** | 无例外，M2 前不开放 |
| `cmd.exe` / `powershell.exe` / `pwsh.exe` | 同上 | **禁止注入** | 同上 |
| `WeChat.exe` / `QQ.exe` / `DingTalk.exe` / `Feishu.exe` | 单行 Unicode；多行 `COPY_ONLY` | M0 验证后按宿主开放 | 防误发送 |
| `Code.exe`（VS Code） | Unicode，需 UIA 身份 | CR | UIA 不匹配 → `COPY_ONLY` |
| `WINWORD.EXE` / `EXCEL.EXE` | Unicode | CR | 原生控件，不强制 UIA |
| `chrome.exe` / `msedge.exe` | Unicode，需 UIA 身份 | CR | UIA 不匹配/无插入点 → `COPY_ONLY` |
| `firefox.exe` | 同上，UIA 可用性待测 | CR | 不可用则 `COPY_ONLY` |
| `explorer.exe`（重命名框） | 单行 Unicode | 不适用 | 含换行即拒绝 |
| 未登记宿主 | 单行 Unicode；多行 `COPY_ONLY` | 拒绝多行 | 首次遇到提示“该应用未验证” |

规则文件带 `version`；用户可在设置里覆盖单条规则的 Tab/模式，覆盖项标 `source: user`；安全硬规则不可覆盖。

### 6.4 UI Spec（原生控件路线）

| 项 | 值 |
| --- | --- |
| 快捷面板 | 520 × 420 逻辑像素（PRD 14.3），允许 460–540 × 320–470 |
| 管理窗口 | 1040 × 650，最小 800 × 540 |
| 栅格 | 8 px 基准；内边距 16/20；列表项高 64 |
| 圆角 | 窗口 12，卡片 12，控件 8，标签 6 |
| 字体 | `Segoe UI Variable` → `Microsoft YaHei UI` → 系统无衬线；12/13/15/17/20 |
| 色板 | 按 PRD 14.2：`#7655E9` `#5E45D7` `#F5F6FB` `#22243E` `#777D92` `#E9EBF3` `#F0EDFF` |
| 状态 | default / hover / selected / keyboard-focus / disabled / empty / no-result / error / loading |
| 深色 | 独立 token 组，不靠反色；`DWMWA_USE_IMMERSIVE_DARK_MODE` |
| DPI | 100 / 125 / 150 / 200%，Per-Monitor V2 manifest |
| 控件皮肤 | `EDIT` 去边框 + 自绘背景；`ListView` owner-draw；滚动条保持原生 |
| 验收 | 面板/管理/设置 × 浅色/深色 共 6 张 150% 截图；每个交互状态至少 1 张；对照 PRD 14.3/14.4 布局清单逐项打勾 |

## 7. 状态机（v2）

```
IDLE
 └─ WM_HOTKEY → HOTKEY_RECEIVED
      └─ CAPTURING_TARGET        # HWND 同步快照；UIA 异步
           └─ PANEL_READY
                ├─ CANCELLED                    # Esc / 点击外部 / 再次热键 / 第三方切窗
                └─ USER_COMMITTED               # Enter/点击；等待物理键释放
                     └─ RESTORE_AND_VERIFY
                          ├─ REJECTED_BEFORE_INPUT   # 零写入，可复制
                          └─ READY_TO_INSERT
                               └─ INPUT_STARTED
                                    ├─ API_EVENTS_ACCEPTED
                                    └─ INPUT_PARTIAL_OR_UNKNOWN
```

| 状态 | 含义 | 对用户的说法 |
| --- | --- | --- |
| `REJECTED_BEFORE_INPUT` | 未调用 `SendInput` | “未输入，已复制” |
| `API_EVENTS_ACCEPTED` | 事件数匹配，事件进入输入流 | 不弹窗；不宣称宿主已提交文本 |
| `INPUT_PARTIAL_OR_UNKNOWN` | 调用过 `SendInput`，返回数不符或前台变化 | “可能已部分输入，请检查目标内容”；不自动重试 |

**注入前守卫（G1–G9 全部满足才进入 `READY_TO_INSERT`）；G10 为调用后判定**

- G1 `IsWindow(target)` 为真。
- G2 PID 与进程创建时间与快照一致。
- G3 前台代数只允许“原应用 → 面板 → 原应用”；其它变化作废。
- G4 隐藏面板后 300 ms 内 `GetForegroundWindow() == target`；不做盲目点击或输入强推。
- G5 `GetGUIThreadInfo(tid).hwndFocus` 非空且为 `target` 后代；与快照不一致时只尝试一次 `AttachThreadInput` + `SetFocus`，仍不一致即拒绝。
- G6 UIA：`CompareElements(snapshot, current)` 为真，或 RuntimeId 相同；元素失效即拒绝。
- G7 UIA：`ControlType` ∈ {Edit, Document, Text}；`IsTextPatternAvailable` 为真；`GetSelection()` 至少 1 个 range；`IsPassword` 为假。
- G8 目标完整性级别 ≤ 本进程。
- G9 物理键已释放：Ctrl/Alt/Shift/Win/Enter 全部 up（`GetAsyncKeyState` 轮询 ≤300 ms）；超时拒绝。
- G10 `SendInput` 返回值 == 请求事件数（仅在调用后判定）。

**失败处理**：G1–G9 任一失败 → `REJECTED_BEFORE_INPUT`，零写入；G10 失败 → `INPUT_PARTIAL_OR_UNKNOWN`，不重试、不补发粘贴。

**计数语义**：`useCount` 仅在 `API_EVENTS_ACCEPTED` 时 +1；`lastInsertStatus` 记录三种终态之一，便于诊断，但不作为“宿主已收到”的证据。

## 8. 实施计划

### M0 可行性（9 人日）

| 模块 | 内容 | 验收 | 人日 |
| --- | --- | --- | ---: |
| M0-1 骨架 | cargo、DPI manifest、单实例、托盘、`RegisterHotKey`、日志 | 冷启动 ≤1.5 s；退出释放热键；第二实例唤起 | 1.5 |
| M0-2 快照 + UIA | HWND/pid/创建时间/hwndFocus；MTA 线程；`CompareElements`；前台代数 | 同页 A/B 输入框不串写；元素重建后拒绝 | 2 |
| M0-3 面板 | 原生窗口 + `EDIT` + `ListView`，3 条硬编码，键盘导航，多屏定位 | 100 次 P95 ≤150 ms；125/150/200% 不越界 | 1.5 |
| M0-4 注入与状态机 | Unicode/CR/Tab/代理对、提权预检、修饰键等待、三态结果 | 反例 T1–T6 全部通过 | 3 |
| M0-5 矩阵与报告 | 140 项检查、体积/内存/P95、失败归因 | 报告可复现，含每格期望与实际 | 1 |

**M0 出口**：见 §9 发布门。任一不满足 → 停，回方案层。

### M1 MVP（21 人日）

| 模块 | 内容 | 依赖 | 验收 | 人日 |
| --- | --- | --- | --- | ---: |
| M1-1 存储 | schema、原子写、备份、迁移、损坏恢复 | M0 | 100 次故障注入无覆盖 | 3.5 |
| M1-2 检索 | searchBlob、打分、排序、高亮 | M1-1 | 1,000 条 P95 ≤50 ms | 2 |
| M1-3 面板完整 | 分类栏、空态/无结果、失败态、动效开关 | M0-3 | PRD 14.3 状态全覆盖 | 3 |
| M1-4 管理窗口 | 三栏、CRUD、分类、回收站、校验、未保存拦截 | M1-1/2 | 全流程通过 | 5 |
| M1-5 设置 | 热键录制、模式、Tab、主题、启动、路径 | M1-1 | 重启一致 | 2 |
| M1-6 托盘与生命周期 | 菜单、暂停/恢复、第二实例、退出清理 | M0-1 | 无残留 | 1.5 |
| M1-7 注入加固 | 宿主规则表、长文本、UIA 降级、剪贴板保护 | M0-4 | 矩阵回归 | 2 |
| M1-8 UI Spec 落地 | 皮肤、深色、DPI、状态截图 | M1-3/4/5 | §6.4 验收清单 | 1 |
| M1-9 打包 | 图标、版本、manifest、README、zip、签名脚本 | 全部 | 新机解压即用 | 1 |
| 缓冲 | 集成、回归、性能基线 | — | — | 4 |

**合计 34 人日**（9 + 21 + 4）。

### 关键路径

`M0-2 → M0-4 → M0-5`。M0-3 可并行。M0 未过不做 M1。

## 9. 测试矩阵与发布门

### 9.1 计数

- 14 个宿主格 × 8 个场景 = **112 格**
- Windows 10 追加 Chrome、VS Code 两格 × 8 = **16 格**
- 专项：横切 6 项 + 反例 T1–T6 = **12 项**
- 合计 **140 项检查**

14 个宿主格：记事本（Win11）、记事本（Win10）、Chrome `input`、Chrome `textarea`、Chrome `contenteditable`、Edge `textarea`、VS Code、Word、微信、Windows Terminal、cmd（conhost）、管理员记事本、资源管理器重命名、钉钉。

8 场景：中文单行、英文+全角符号、CR 多行、Tab 缩进、Emoji、选区替换、光标中间插入、2,000 字长文本。

### 9.2 每格三类期望

| 类别 | 含义 | 判定 |
| --- | --- | --- |
| `AUTO_INSERT` | 声明支持自动输入 | 成功写入 ≥99/100；错误控件/窗口 = 0 |
| `COPY_ONLY` | 只允许复制，禁止注入 | 注入次数 = 0 |
| `UNSUPPORTED` | 明确拒绝 | 安全失败 = 100% |

每格在测试报告中必须写明类别、期望、实际、证据路径。自动写入成功率只统计 `AUTO_INSERT` 格。

### 9.3 横切与反例

横切：IME 组合态、面板期间切窗、Alt+Tab、150% 双屏边缘、热键冲突、连按 20 次。

| 编号 | 反例 | 合格结果 |
| --- | --- | --- |
| T1 | Chrome 同页输入框 A → B | 不写入 B；不确定时只复制 |
| T2 | 面板出现后目标 DOM/控件重建 | 元素不匹配即拒绝 |
| T3 | 提交时 Enter/Ctrl/Alt 仍按下 | 不自动发送消息 |
| T4 | 终端含换行文本 | 只复制，不执行命令 |
| T5 | `SendInput` 部分事件被接受 | 报“可能部分输入”，不重试 |
| T6 | 剪贴板含图片/HTML 或被他进程修改 | 不宣称无损恢复，不覆盖 |

### 9.4 发布阻断项

1. `AUTO_INSERT` 格出现错误顶层窗口或错误控件写入。
2. `COPY_ONLY`/`UNSUPPORTED` 格发生注入。
3. 聊天软件因工具操作自动发送消息。
4. 终端执行了短语中的命令。
5. Esc / 点击外部后原输入框内容改变。
6. 提权目标出现“看似成功、实际未写入”。
7. 面板 P95 >150 ms 或静置 60 s Working Set >35 MB。

“错误窗口写入 = 0”是**测试阻断门**，不是对外的形式化保证；输入开始后的不确定性由 §7 的三态表达。

## 10. 风险登记

| 风险 | 概率 | 影响 | 处置 |
| --- | --- | --- | --- |
| UIA 身份对 Chrome/Electron 命中率低 | 中 | 高 | D19：M0 实测；<80% 则浏览器默认只复制并回写 PRD |
| UIA 调用卡顿/挂起 | 中 | 中 | 常驻 MTA + 400 ms 总预算 + UNKNOWN 降级；不阻塞首帧 |
| 浏览器内错字段写入 | 低–中 | 高 | G6/G7 身份与插入点双重校验 |
| 杀软误报 | 高 | 高 | 签名（预算为估算，需询价）、无钩子、提交厂商 |
| 中文 IME 异常 | 低 | 高 | 原生 EDIT；M0 必测组合态 |
| 终端多行被误执行 | 低 | 高 | 终端只复制；单行也加二次校验 |
| 输入部分成功 | 中 | 中 | 三态提示；不重试；日志留证 |
| 工期超支 | 中 | 中 | M0 门禁；M1 可砍动效/主题 |

**回滚**：迁移前备份；升级只换 EXE；旧 EXE 遇新 schema 只读导出。

## 11. 与 PRD 的差异（回写 PRD）

1. 默认 Unicode 改为**带长度/宿主/控制字符约束的 Unicode**。
2. 终端含换行**只复制**，不提供自动粘贴；单行允许 Unicode。
3. 聊天软件多行默认只复制，M0 验证后才开放。
4. Unicode 硬上限 2,000 UTF-16 单元（≈2,000 个 BMP 汉字）。
5. Tab 默认 4 空格，设置可见可改。
6. 剪贴板不承诺无损恢复；仅纯文本快照 + 显式恢复。
7. 回收站 P1，进 M1，`deletedAt` 进 schema v1。
8. 10,000 上限按 UTF-16 单元。
9. ≥99% 的分母 = `AUTO_INSERT` 格；`COPY_ONLY`/`UNSUPPORTED` 另计。
10. 新增用例：A→B 控件切换、DOM 重建、修饰键未释放、终端的换行、SendInput 部分成功、剪贴板多格式。
11. 主题三态写入设置与验收。
12. 指标口径：静置 60 s / 100 次 P95。
13. 输入结果三态（零写入 / 事件已接受 / 可能部分输入），不再宣称全局零写入。
14. UIA 控件身份校验列为 P0；浏览器无身份即只复制。
15. 终端高风险自动粘贴不在 V1。

## 12. 待确认问题

1. 代码签名：正式公开分发前是否采购证书。v1 的 ¥1,500–4,000/年只是估算，需实际询价。
2. 开源还是私有：不影响 M0，发布前决定。
3. 基准机：性能门禁跑在哪台机器（当前开发机型号）。
4. Windows 10 最低版本：建议 21H2+，且仅对受测主机承诺。
5. 是否接受终端 MVP“含换行只复制”：这会限制把多行 SQL 直接送进终端的场景。
6. 界面语言：默认简中；文案资源化预留。
