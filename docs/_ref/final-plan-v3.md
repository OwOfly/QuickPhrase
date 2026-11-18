# QuickPhrase 最终方案（final-plan v3）

- 汇总：DeepSeek-V4.1-Flash（主席）
- 日期：2026-11-18
- 状态：**待 focused review**（v3 尚未经三模型定向复核，不得直接进入 M0 编码）
- 修订依据：v1 独立复核（B1–B3、M1–M10）、v2 定向增量复核（B4、B5、M11–M14）
- 目标仓库：`D:\Code\AIProject\quickphrase`
- 规格来源：`prd-v1.1.md`
- 历史版本：`final-plan-v1.md`、`final-plan-v2.md`

## 0. 修订对照

| 项 | 问题 | v3 处置 | 落点 |
| --- | --- | --- | --- |
| B1 | 只判断“能收文字”，不判断“是不是同一个控件” | 控件身份 + 可编辑 + 插入点三项分开验证 | §4 D10/D11、§5.5、§7 |
| B2 | 调用过 `SendInput` 后仍称零写入 | 三态结果 | §7 |
| B3 | 终端“显式粘贴”仍会执行命令 | 终端含换行只复制，无旁路 | §4 D8、§6.3 |
| B4 | 异步 UIA 在面板激活后取到面板自身焦点 | **捕获必须在面板激活前同步完成**；超时 UNKNOWN → 只复制；放弃“异步补齐” | §5.2、§7 |
| B5 | 按 EXE 分类无法识别嵌入式终端（VS Code 集成终端） | 引入“聚焦区域”概念：`TERMINAL` / `TEXT_EDITOR` / `UNKNOWN`；UNKNOWN 多行只复制 | §4 D21、§5.5、§6.3 |
| M11 | `GetSelection` 不足以判断插入点 | 能力矩阵：TextPattern2 光标优先；ValuePattern-only 禁止插入；Document 级不算具体控件 | §5.5 |
| M12 | 400 ms 预算不是 COM 硬中断 | 队列上限 1、超时丢弃、脏快照不复用、worker 异常重建 | §5.4 |
| M13 | API 接受 ≠ 文本写入 | `useCount` 定义为“发起插入次数”；UI 不显示“成功已验证” | §7 |
| M14 | 浏览器 UIA 覆盖率无统计口径 | 六类计数 + 逐宿主统计 | §9.2 |
| M9 | 工期与 UTF-16 说明错误 | 重算为 36 人日；2,000 单元 ≈ 2,000 个 BMP 汉字 | §8 |

**对 v2 复核报告的一处否决**

报告 B4 方案 A 提出“非激活首帧 + 捕获完成后再允许面板获得焦点”。否决：面板可见但未激活时，用户按键会进入原应用，热键后立即打字会把搜索词写进文档或终端。v3 改为“捕获先于激活同步完成”，不引入该泄漏窗口。

---

## 1. 背景与目标

Windows 桌面全局快捷短语输入器：热键唤起面板 → 搜索选中 → 文本写入触发前的原输入位置。单机、离线、无账号。

**第一版只验证三件事**：短语能维护、随时能唤起、文字准确进入原位置。跨应用输入不可靠时，宁可不输入。

**交付形态**：免安装单 EXE（zip），数据在 `%LocalAppData%\QuickPhrase`。

## 2. 范围与非目标

**M1（MVP）范围**

- 全局热键（默认 `Ctrl+Alt+Q`，可改、冲突提示）
- 快捷面板：搜索、分类筛选、键盘导航、置顶排序、空态/无结果、只读状态提示
- 短语 CRUD、分类、置顶、回收站（软删除 30 天）
- 设置：热键录制、输入模式、按应用规则、主题、开机启动、数据路径
- 持久化：JSON + 原子写 + 5 份滚动备份 + 损坏恢复
- 导入/导出 JSON（冲突预览、不覆盖）
- 输入：Unicode 单行 + 已验证区域的多行；其余只复制
- 托盘常驻、单实例、第二实例唤起
- 本地日志与诊断包导出（不含短语内容）

**非目标（V1 不做）**

云同步、团队协作、AI 改写、富文本/图片、缩写自动展开、批量宏、读取输入框内容、OCR、移动端、浏览器插件、密码保险箱、自动更新、终端自动粘贴。

**声明不支持**：UAC 安全桌面、锁屏、独占全屏/反作弊游戏、提权目标窗口、跨会话（RDP）、无 UIA 且无 HWND 焦点的自绘控件。

### 2.1 隐私与安全边界

- 不联网；无遥测、无崩溃上传。
- 不安装键盘钩子，不采集击键历史，不读取输入框内容，不记录剪贴板历史。
- 使用 `SetWinEventHook(EVENT_SYSTEM_FOREGROUND / EVENT_OBJECT_FOCUS)` 做只读焦点观察；不拦截输入。
- 日志只记元数据（exe、窗口类、区域类别、失败码、字符数、耗时、UIA 状态），不记短语正文。
- 数据只落 `%LocalAppData%\QuickPhrase`。
- 密码框（`ES_PASSWORD` / UIA `IsPassword`）检测到即拒绝。
- 不申请管理员权限；提权目标直接拒绝。

## 3. 侦察结论（含实证）

| 事实 | 证据 | 影响 |
| --- | --- | --- |
| Chromium 系 `hwndFocus` 等于顶层窗口，`hwndCaret=NULL` | 2026-11-18 本机实测 Electron：`Focus=0x40A80 [Chrome_WidgetWin_1]`，Caret NULL | HWND 只能定位窗口 |
| UIA 可区分同一窗口内的控件，但需身份比对 | `CompareElements` / RuntimeId | 身份必须显式验证 |
| `GetFocusedElement` 返回系统当前焦点 | UIA 文档 | 面板激活后调用会拿到面板自身 → 捕获必须在激活前完成 |
| UIA 调用不可硬取消 | UIA 线程模型 | 超时只能丢弃结果；队列必须限长 |
| `SendInput` 被 UIPI 阻断时失败，但错误码不说明原因 | MS Learn | 令牌预检 |
| `SendInput` 不重置键盘状态；事件串行 | 同上 | 等物理键释放 |
| WinForms/WPF + NativeAOT 非官方支持 | `NETSDK1175` | C# 备选不可行 |
| 依赖版本 | crates.io：`slint 1.19.0`、`windows 0.62.2` | 版本号以此为准 |
| 本机工具链 | rustc 1.93.0-nightly、.NET 8.0.427 | Rust 可开工 |

## 4. 决策点表

| # | 决策 | 裁决 | 理由 | 残余风险 |
| --- | --- | --- | --- | --- |
| D1 | 语言/运行时 | **Rust + `windows` 0.62** | 体积/内存/无运行时依赖 | UI 开发量高 |
| D2 | UI 实现 | **原生控件 + D2D 装饰**：`EDIT`、`ListView(LVS_OWNERDRAWFIXED)`、`RICHEDIT` | IME/无障碍/键盘行为 | 视觉需 UI Spec 约束 |
| D3 | 视觉 | **DWM 圆角 + 系统材质（Win11）+ 自绘阴影；Win10 纯色** | ≤35 MB | Win10 观感降级 |
| D4 | 默认注入 | **`SendInput` + `KEYEVENTF_UNICODE`，UTF-16 码元逐个 down/up，单次调用** | 不占剪贴板 | 部分控件丢事件 |
| D5 | 换行 | **CR(0x0D)**；禁止 `VK_RETURN` | EDIT 忽略 LF；VK_RETURN 会发送 | 个别宿主 CR 不换行 |
| D6 | Tab | **默认 4 空格，设置可见可改；保留 Tab 仅粘贴模式** | 避免焦点跳转 | 缩进语义 |
| D7 | 长文本 | **≤2,000 UTF-16 单元**；超出仅粘贴/复制 | 单次注入过长 | 体验 |
| D8 | 终端 | **含任何换行 → 只复制**，无自动粘贴旁路；单行允许 Unicode | 换行=执行 | 开发者多行场景受限 |
| D9 | 聊天 | 单行 Unicode；**多行只复制**，M0 验证后才可能放开 | 防误发送 | 宿主升级 |
| D10 | 控件身份 | 两条路径：原生控件用 `hwndFocus` 相等；Chromium/混合宿主用 UIA `CompareElements` | HWND 不足以定位浏览器控件 | UIA 失败即只复制 |
| D11 | 三态 | **IDENTITY_VERIFIED / IDENTITY_MISMATCH / IDENTITY_UNKNOWN**，只有 VERIFIED 注入 | 身份优先于“能收文字” | 可用性下降 |
| D12 | 提权 | 令牌完整性比较；失败或更高即拒绝 | 返回值不可信 | 受保护进程 |
| D13 | 剪贴板 | 默认不碰；兼容模式仅纯文本快照；不承诺无损；外部改动不覆盖 | 无损恢复不可能 | 用户丢失原内容 |
| D14 | 数据模型 | `categories[]` + `snippets[]` + `settings` + `rules-user[]` + `schemaVersion` + `revision` | 分类可维护 | — |
| D15 | 回收站 | M1 软删除 30 天 | 原型有入口 | 1 人日 |
| D16 | 排序/检索 | pinned → lastUsedAt → updatedAt → sortOrder → id；标题 400/300/200，正文 100；渲染前 50 | 可验收 | 拼音 M2 |
| D17 | 指标 | 空闲 = 静置 60 s Working Set；延迟 100 次 P95；固定基准机 | 可复现 | 机器差异 |
| D18 | 工期 | **M0 10 + M1 22 + 缓冲 4 = 36 人日** | §8 明细 | 排期风险 |
| D19 | 浏览器 UIA 覆盖率 | M0 统计；<80% 则浏览器默认只复制并回写 PRD | 不预写产品行为 | 可用性 |
| D20 | UI 验收 | §6.4 UI Spec + 截图验收 | 视觉是产品 | 还原度 |
| D21 | 聚焦区域分类 | `TERMINAL` / `TEXT_EDITOR` / `UNKNOWN`；UNKNOWN 多行只复制；只看区域，不看 exe 白名单 | 嵌入式终端与 exe 同进程 | 区域识别率 |
| D22 | 捕获时机 | **UIA 捕获必须在面板激活前同步完成（60 ms 预算）**；超时 UNKNOWN | 异步必拿到面板自身 | 面板延迟 +≤60 ms |
| D23 | 浏览器多行 | 默认只复制；M0 实验 E1（LF 注入）通过后才放开 | Gmail/网页终端风险 | 多行场景受限 |

## 5. 架构

### 5.1 线程

| 线程 | 职责 | 约束 |
| --- | --- | --- |
| UI | 消息循环、窗口、托盘、`WM_HOTKEY` | 捕获阶段最多阻塞 60 ms |
| 注入 | 恢复、校验、`SendInput`、剪贴板 | 不阻塞 UI |
| UIA（MTA，常驻） | `GetFocusedElement`、`CompareElements`、属性、区域判定 | 队列上限 1；超时丢弃 |
| 存储 | 原子写、备份、导入导出 | 独立文件句柄 |

### 5.2 热键 → 捕获 → 面板（B4 修复）

```
t0  WM_HOTKEY
    ├─ GetForegroundWindow / pid / 线程 / 进程创建时间 / hwndFocus / 令牌
    ├─ 若前台是自己 → 中止
    ├─ 向 UIA 线程发起捕获请求，deadline = t0 + 60 ms
    │    GetFocusedElement → 校验 NativeWindowHandle 属于 target
    │    → RuntimeId / ControlType / ClassName / IsPassword
    │    → TextPattern / TextPattern2 光标信息
    │    → 区域判定（TERMINAL / TEXT_EDITOR / UNKNOWN）
    ├─ 结果：CAPTURED_VERIFIED / CAPTURED_UNSUPPORTED / CAPTURED_TIMEOUT
    └─ 之后才 SetForegroundWindow(panel) + 聚焦搜索框
```

规则：

- **绝不**在面板激活后再查询“当前焦点”当原始快照。
- 捕获超时或失败：面板照常打开，但本次选择只能复制；面板页脚显示“此应用仅支持复制”。
- 模态/通知抢焦点导致前台已变：捕获作废，只复制。
- UIA 首次调用在程序启动时预热（创建 `CUIAutomation` 并空转一次）。

### 5.3 提交 → 恢复 → 注入

1. 等物理键释放（Ctrl/Alt/Shift/Win/Enter），≤300 ms。
2. 关闭面板（`ShowWindow(SW_HIDE)`）。
3. `SetForegroundWindow(target)`；10 ms 轮询，≤300 ms。
4. 复核 G1–G9（见 §7）。
5. 通过后 `SendInput` 单次调用。
6. 按返回值进入 `API_EVENTS_ACCEPTED` 或 `INPUT_PARTIAL_OR_UNKNOWN`。

### 5.4 UIA 客户端约束（M12）

- 单例 `CUIAutomation`，只在一个常驻 MTA 线程使用；COM 对象不跨线程。
- 待处理队列上限 1；新请求覆盖旧请求，旧结果作废。
- 超时不取消调用，只丢弃结果；过期快照绝不复用。
- worker 连续 3 次超时或抛 `RPC_E_*`：销毁并重建 worker，期间所有请求返回 UNKNOWN。
- 所有 UIA 调用带 `UIA_E_ELEMENTNOTAVAILABLE` 处理：元素失效即判 MISMATCH。

### 5.5 能力矩阵（M11）

| 目标类型 | 身份判据 | 可编辑判据 | 插入点判据 | 允许 |
| --- | --- | --- | --- | --- |
| 原生 `Edit` / `RichEdit` | `hwndFocus` 相等 | 窗口类 + 非 `ES_PASSWORD` | `rcCaret`（可选） | 是 |
| UIA Edit + TextPattern2 | `CompareElements` | `ControlType=Edit`，`IsEnabled` | `GetCaretRange().isActive` | 是 |
| UIA Edit + TextPattern | `CompareElements` | 同上 | `GetSelection().Length ≥ 1` | 是 |
| UIA Document（整页） | `CompareElements` | `TextPattern` 可用 | 有 caret range 且矩形不覆盖整窗 | 仅单行 |
| ValuePattern-only | `CompareElements` | `ValuePattern` 可用 | 无 | **否，只复制** |
| 元素失效/无法取得 | — | — | — | **否，只复制** |

区域判定（D21）：

| 区域 | 判据 | 多行策略 |
| --- | --- | --- |
| `TERMINAL` | `ConsoleWindowClass`；已知终端 exe；UIA 祖先含已验证终端标记 | 只复制 |
| `TEXT_EDITOR` | 原生 `Edit`/`RichEdit`/`_WwG`；已验证的编辑器标记 | Unicode + CR |
| `UNKNOWN` | 其余，包括浏览器、Electron、混合宿主 | 只复制 |

**不按 exe 放行多行。** `Code.exe` 默认 UNKNOWN；只有 M0 拿到可复现的编辑器区域标记，才把该区域升级为 `TEXT_EDITOR`。

## 6. 数据契约

### 6.1 文件与备份

| 文件 | 内容 | 写入 | 备份 |
| --- | --- | --- | --- |
| `data.json` | schema/revision/categories/snippets | 原子写 | 5 份 `data-<ts>.json` |
| `settings.json` | 热键/模式/Tab/主题/启动/暂停 | 原子写 | 5 份 |
| `rules-user.json` | 用户覆盖规则 | 原子写 | 5 份 |
| `logs/qp-<date>.log` | 元数据日志 | 追加 | 3 × 500 KB |

统一：`.tmp` → `FlushFileBuffers` → 重解析校验 → `ReplaceFileW`。损坏按时间倒序恢复；全失败进入只读模式。

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
// rules-user.json
{ "version": 1, "rules": [{ "exe": "Code.exe", "region": "TEXT_EDITOR", "source": "user" }] }
```

规则：

- 标题 1–80 UTF-16 单元；正文 1–10,000 UTF-16 单元。
- 导入 ≤20 MB / ≤10,000 条；同 `id` 一致跳过、不同要求“跳过/另存副本”，禁止覆盖。
- `schemaVersion` 高于程序 → 只读导出，不写入。
- 迁移前写 `pre-migrate-v<N>-<ts>.json`。
- 软删除 30 天；恢复保留原 `id` 与分类。
- 规则优先级：用户覆盖 > 内置 > 全局模式；安全硬规则（终端换行、密码框、提权、身份不匹配）不可覆盖。

### 6.3 内置宿主规则

| 宿主 | 单行 | 多行 | 备注 |
| --- | --- | --- | --- |
| `WindowsTerminal` / `OpenConsole` / `conhost` / `mintty` / `putty` / `cmd` / `powershell` / `pwsh` | Unicode | **只复制** | `ConsoleWindowClass` 直接判 TERMINAL |
| `Code.exe`（含集成终端） | Unicode | **只复制**（除非区域已验证为编辑器） | 混合宿主 |
| `idea64.exe` / `pycharm64.exe` / `devenv.exe` | Unicode | 只复制（除非区域已验证） | 可能内嵌终端 |
| `WeChat` / `QQ` / `DingTalk` / `Feishu` | Unicode | 只复制 | 防误发送 |
| `WINWORD` / `EXCEL` / `OUTLOOK`（Word 编辑器） | Unicode | Unicode + CR | `_WwG` 判 TEXT_EDITOR |
| 记事本 / 写字板 | Unicode | Unicode + CR | 原生控件 |
| Chrome / Edge / Firefox | Unicode | 只复制（E1 通过后按浏览器 LF 模式放开） | 网页可能发送/执行 |
| 资源管理器重命名 | Unicode | 不适用 | 单行 |
| 未登记 | Unicode | 只复制 | 首次提示“未验证” |

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
| 控件皮肤 | `EDIT` 去边框自绘背景；`ListView` owner-draw；滚动条原生 |
| 验收 | 面板/管理/设置 × 浅色/深色 6 张 150% 截图；每状态至少 1 张 |

## 7. 状态机

```
IDLE
 └─ WM_HOTKEY → HOTKEY_RECEIVED
      └─ CAPTURING_ORIGINAL          # 同步，deadline 60 ms
           ├─ CAPTURED_VERIFIED      → PANEL_READY
           ├─ CAPTURED_UNSUPPORTED   → PANEL_READY（只复制）
           └─ CAPTURED_TIMEOUT       → PANEL_READY（只复制）
                ├─ CANCELLED                     # 零写入
                └─ USER_COMMITTED
                     └─ RESTORE_AND_VERIFY
                          ├─ REJECTED_BEFORE_INPUT      # 未调用 SendInput
                          └─ READY_TO_INSERT
                               └─ INPUT_STARTED
                                    ├─ API_EVENTS_ACCEPTED
                                    └─ INPUT_PARTIAL_OR_UNKNOWN
```

| 状态 | 含义 | 用户可见 |
| --- | --- | --- |
| `REJECTED_BEFORE_INPUT` | 未调用 `SendInput` | “未输入，已复制” |
| `API_EVENTS_ACCEPTED` | 事件数匹配 | 不弹窗；不显示“输入成功” |
| `INPUT_PARTIAL_OR_UNKNOWN` | 调用过 `SendInput`，结果不确定 | “可能已部分输入，请检查目标” |

**注入前守卫 G1–G9（全部满足才进入 `READY_TO_INSERT`）**

- G1 `IsWindow(target)`。
- G2 PID + 进程创建时间一致。
- G3 前台序列只允许“原应用 → 面板 → 原应用”。
- G4 300 ms 内 `GetForegroundWindow() == target`；不盲目强推。
- G5 `hwndFocus` 非空且属 `target`；原生控件路径要求与快照相等。
- G6 身份：原生路径 `hwndFocus` 相等；UIA 路径 `CompareElements(snapshot, current)` 为真或 RuntimeId 相同。
- G7 能力：按 §5.5；`ValuePattern`-only、Document 整页、密码框、无插入点 → 拒绝。
- G8 完整性 ≤ 本进程。
- G9 物理键全部释放（≤300 ms）。
- G10（调用后）`SendInput` 返回数 == 请求数。

失败：G1–G9 → `REJECTED_BEFORE_INPUT`；G10 → `INPUT_PARTIAL_OR_UNKNOWN`，不重试、不补发粘贴。

**计数**：`useCount` = 到达 `API_EVENTS_ACCEPTED` 的发起次数；UI 文案为“使用次数”，不宣称“成功插入”。

## 8. 实施计划

### M0（10 人日）

| 模块 | 内容 | 验收 | 人日 |
| --- | --- | --- | ---: |
| M0-1 骨架 | cargo、DPI manifest、单实例、托盘、热键、日志 | 冷启动 ≤1.5 s；退出释放热键 | 1.5 |
| M0-2 捕获 | 同步快照、UIA 60 ms、身份/能力/区域、线程约束 | 捕获永远先于面板激活；超时只复制 | 3 |
| M0-3 面板 | 原生窗口 + EDIT + ListView，3 条硬编码，键盘导航，多屏 | 100 次 P95 ≤150 ms；DPI 不越界 | 1.5 |
| M0-4 注入与状态机 | Unicode/CR/Tab/代理对、提权、修饰键、三态 | T1–T12 全部通过 | 3 |
| M0-5 矩阵与实验 | 146 项检查；E1/E2/E3 | 报告可复现 | 1 |

**实验**

- **E1**：浏览器多行用 LF(U+000A) 注入，是否产生换行且不触发 keydown 发送/执行。通过 → 允许浏览器 LF 多行；不通过 → 只复制。
- **E2**：VS Code 编辑器 vs 集成终端的 UIA 区域标记是否可复现。
- **E3**：UIA 捕获延迟分布（每宿主 100 次）。

### M1（22 人日）

| 模块 | 内容 | 依赖 | 验收 | 人日 |
| --- | --- | --- | --- | ---: |
| M1-1 存储 | schema、原子写、备份、迁移、恢复 | M0 | 100 次故障注入 | 3.5 |
| M1-2 检索 | blob、打分、排序、高亮 | M1-1 | 1,000 条 P95 ≤50 ms | 2 |
| M1-3 面板完整 | 分类、空态、失败态、只读态 | M0-3 | PRD 14.3 全覆盖 | 3 |
| M1-4 管理窗口 | 三栏、CRUD、分类、回收站、校验 | M1-1/2 | 全流程 | 5 |
| M1-5 设置 | 热键、模式、Tab、主题、启动、路径 | M1-1 | 重启一致 | 2.5 |
| M1-6 托盘 | 菜单、暂停/恢复、第二实例、退出 | M0-1 | 无残留 | 1.5 |
| M1-7 注入加固 | 区域规则、长文本、UIA 降级、剪贴板 | M0-4 | 回归通过 | 2.5 |
| M1-8 UI Spec | 皮肤、深色、DPI、截图 | M1-3/4/5 | §6.4 清单 | 1 |
| M1-9 打包 | 图标、版本、README、zip、签名脚本 | 全部 | 新机即用 | 1 |
| 缓冲 | 集成、回归、基线 | — | — | 4 |

**合计 36 人日**（10 + 22 + 4）。

## 9. 测试矩阵与发布门

### 9.1 计数

- 14 宿主格 × 8 场景 = 112
- Win10 追加 Chrome / VS Code × 8 = 16
- 专项 18 项：横切 6 + T1–T12
- **合计 146 项**

14 宿主：记事本（Win11）、记事本（Win10）、Chrome `input`、Chrome `textarea`、Chrome `contenteditable`、Edge `textarea`、VS Code **编辑器**、VS Code **集成终端**、Word、微信、Windows Terminal、cmd（conhost）、管理员记事本、资源管理器重命名。
（钉钉/飞书作为第 15/16 格在 M1 回归补测。）

### 9.2 分类与计数（M14）

每格必须标 `AUTO_INSERT` / `COPY_ONLY` / `UNSUPPORTED`，并记录：

`VERIFIED`、`IDENTITY_UNKNOWN`、`IDENTITY_MISMATCH`、`NON_EDITABLE`、`TIMEOUT`、`REGION_UNKNOWN`、`AUTO_INSERT`、`COPY_ONLY`。

按宿主 × 控件类型统计 `VERIFIED` 命中率；D19 用此数据决定浏览器策略。

### 9.3 反例 T1–T12

| # | 反例 | 合格结果 |
| --- | --- | --- |
| T1 | Chrome 同页 A → B | 不写 B；不确定只复制 |
| T2 | 面板出现后 DOM/控件重建 | 拒绝注入 |
| T3 | 提交时 Enter/Ctrl/Alt 仍按下 | 不自动发送 |
| T4 | 终端含换行 | 只复制，不执行 |
| T5 | `SendInput` 部分接受 | 报“可能部分输入”，不重试 |
| T6 | 剪贴板含图片/HTML 或被改 | 不宣称无损，不覆盖 |
| T7 | 热键后 UIA 延迟 300–500 ms | 捕获超时 → 只复制；不把面板焦点当原控件 |
| T8 | UIA 永久阻塞 >2 s | UI 不卡；worker 重建；旧快照不复用 |
| T9 | 同一 `Code.exe`：编辑器 vs 集成终端 | 编辑器按规则，终端多行只复制 |
| T10 | 浏览器网页终端（xterm） | 多行不注入 |
| T11 | 原生 EDIT + ValuePattern-only 控件 | 后者只复制 |
| T12 | 焦点缓存/快照过期（若实现缓存） | 不复用，重新捕获或只复制 |

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
| 捕获 60 ms 超时导致大量只复制 | 中 | 中 | 启动预热；E3 测量；必要时 M2 加焦点事件缓存 |
| 区域识别不到编辑器，VS Code 只能复制 | 高 | 中 | E2 实测；失败则接受限制并写进 README |
| 浏览器多行长期只复制 | 中 | 中 | E1 决定；不行就保持 |
| UIA 卡顿/挂起 | 中 | 中 | 队列 1、超时丢弃、worker 重建 |
| 杀软误报 | 高 | 高 | 签名（需询价）、无钩子、提交厂商 |
| IME 异常 | 低 | 高 | 原生 EDIT；M0 必测 |
| 部分输入 | 中 | 中 | 三态提示；不重试 |
| 工期超支 | 中 | 中 | M0 门禁；M1 可砍动效 |

**回滚**：迁移前备份；升级只换 EXE；旧 EXE 遇新 schema 只读导出。

## 11. 与 PRD 的差异（回写 PRD）

1. 默认 Unicode 受长度/区域/控制字符约束。
2. 终端与未知区域的多行只复制。
3. 聊天多行只复制。
4. Unicode 上限 2,000 UTF-16 单元（≈2,000 个 BMP 汉字）。
5. Tab 默认 4 空格，设置可见。
6. 剪贴板不承诺无损恢复。
7. 回收站 P1，进 M1。
8. 10,000 上限按 UTF-16 单元。
9. ≥99% 分母 = `AUTO_INSERT` 格。
10. 新增反例 T1–T12。
11. 主题三态。
12. 指标口径固定。
13. 输入结果三态。
14. 控件身份校验为 P0。
15. 终端自动粘贴不在 V1。
16. 多行不再按 exe 放行，按聚焦区域。
17. 浏览器多行默认只复制，待 E1。
18. 捕获必须在面板激活前完成（新增时序约束）。

## 12. 待确认问题

1. **浏览器多行**：如果 E1（LF 注入）不通过，V1 浏览器只能单行自动输入、多行靠复制粘贴。接受吗？
2. **VS Code**：若 E2 识别不出编辑器区域，VS Code 多行也只能复制。接受吗？
3. **终端多行只复制**：确认无旁路。
4. 签名：公开分发前是否采购证书（需询价，v1 的金额只是估算）。
5. 开源还是私有。
6. 基准机型号。
7. Windows 10 最低版本（建议 21H2+）。
