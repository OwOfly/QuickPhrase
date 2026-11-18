# M0-2 验收记录（2026-11-18）

- 范围：热键前快照、UIA 捕获（含零 HWND 上溯）、只读分支、CompareElements、前台监听、UIA 超时降级
- 代码状态：工作区，未提交（M0-1 已提交 `c963057`）
- 环境：Windows 11 x64；Chrome 前台页面；无交互桌面（agent 会话）
- 复核依据：`docs/_ref/final-plan.md` v4.1 §8 M0-2、§5.2、§5.4、§5.5

## 已实现

| 项 | 实现 |
| --- | --- |
| 原生快照 | 前台 HWND、PID、线程、进程创建时间、exe、窗口类/标题、`hwndFocus`、caret、完整性级别 |
| UIA 捕获 | 常驻 MTA 线程 + `CUIAutomation`；`GetFocusedElement`；60 ms 预算；启动预热 |
| 零 HWND | `NativeWindowHandle==0` 时用 `RawViewWalker` 最多上溯 64 层（实测 Chromium 需要 24 层），命中顶层窗口后校验归属 |
| 身份 | `CompareElements` 为唯一权威；RuntimeId 未使用 |
| 只读 | 原生 `ES_READONLY`；`ValuePattern.CurrentIsReadOnly`；TextRange `UIA_IsReadOnlyAttributeId` |
| 插入点 | `TextPattern2.GetCaretRange().isActive`；`TextPattern.GetSelection().Length` |
| 区域 | 终端（`ConsoleWindowClass` / 终端 exe / UIA class 含 xterm）；编辑器（`Edit`/`RichEdit`/`_WwG`）；其余 UNKNOWN |
| 超时 | 请求 ID + deadline；过期请求不执行、结果不回写 `last`；连续 2 次超时后本会话 `UIA_UNAVAILABLE`，注入降级只复制 |
| 决策 | 身份与能力分离；UIA 超时/否定证据不回退原生；只读未知、Document、无插入点、禁用、密码、提权、完整性未知一律只复制 |
| 隐私 | 不采集 UIA `CurrentName`（可能包含控件正文），报告只保留类名、控件类型、属性标志 |
| 前台监听 | 面板生命周期内 `EVENT_SYSTEM_FOREGROUND` + 独立消息循环，记录 seq/hwnd/pid |

## 实测证据

### 1. 可编辑控件（关键路径）

`--capture-test --delay 500`，前台为 OpenChamber（Electron）的编辑器：

```json
{
  "identity": "verified",
  "identity_source": "uia",
  "capability": "text",
  "region": "unknown",
  "timings_ms": { "native": 0, "uia": 25, "total": 25 },
  "uia": {
    "control_type": 50004,
    "class_name": "cm-content cm-composer-empty cm-lineWrapping",
    "is_read_only": false,
    "has_text_pattern": true,
    "has_value_pattern": true,
    "selection_count": 1,
    "native_hwnd_zero": true,
    "ancestor_verified": true,
    "ancestor_depth": 24,
    "elapsed_ms": 25
  }
}
```

**深度发现**：上溯上限原设 20 层时，同一元素 `ancestor_verified=false`、身份降级为 UNKNOWN；改成 64 层后在**第 24 层**命中窗口，身份恢复 VERIFIED。Chromium/Electron 的零 HWND 元素可以超过 20 层，20 不够。

**复核修复**：三家代码复核后，能力判定改为“未知即拒绝”；UIA 超时/否定不再回退原生；只读需两源都明确为 false；Document、无插入点、禁用、提权、完整性未知均只复制；`CurrentName` 已从报告移除。

### 2. 零 HWND 的非编辑元素

前台为 Chrome 页面的列表项，`capability` 正确判为 `none`，不会因为“身份已验证”误报可输入：

```json
{
  "identity": "verified",
  "identity_source": "uia",
  "capability": "none",
  "region": "unknown",
  "timings_ms": { "native": 0, "uia": 51, "total": 51 },
  "uia": {
    "control_type": 50007,
    "native_hwnd_zero": true,
    "ancestor_verified": true,
    "has_text_pattern": false,
    "is_read_only": null,
    "elapsed_ms": 51
  }
}
```

### 3. 只读分支

前台为页面正文（Document）时，`TextPattern` 的 `UIA_IsReadOnlyAttributeId` 命中：

```
capability=readonly region=unknown uia=[class= type=50030 ro=Some(true) text=true sel=1] uia=19ms
compare_last=Some(true)
```

| 验收点 | 结果 |
| --- | --- |
| 零 HWND 上溯 | PASS：`native_hwnd_zero=true`、`ancestor_verified=true`、`ancestor_depth=24` |
| 可编辑控件识别 | PASS：`control_type=50004`、`has_text_pattern=true`、`is_read_only=false` → `capability=text` |
| 只读识别（TextRange） | PASS：`is_read_only=Some(true)` → `capability=readonly` |
| 非编辑控件不误报 | PASS：列表项 `identity=verified`、`capability=none` |
| `CompareElements` | PASS：`compare_last=Some(true)` |
| 60 ms 预算 | PASS：25 ms（预热后）；冷启动 51 ms |
| 预热 | PASS：`uia warmup: true` |
| 日志与 JSON 落盘 | PASS：`captures/capture-*.json` |

## 未覆盖（必须在 M0-5 手工矩阵补）

- 原生 `EDIT`（记事本/Word）样本：当前桌面无法把焦点切过去；Chromium contenteditable 已覆盖。
- 前台 A → B → A 序列（无交互桌面，`SetForegroundWindow` 被系统拒绝，前台监听未收到事件）。
- UIA 永久挂起与连续超时降级（需要故障注入或卡死的 provider）。
- 原生 `EDIT` 的 `hwndFocus` 身份路径（与上一条同因）。
- T7/T8/T12–T15、T25 的运行时验证。

## 结论

M0-2 的代码路径与可自动验证的部分通过；焦点相关的反例尚未实测，不能据此宣称 M0-2 全量通过。
建议：先做 M0-3 面板，使“面板期间切窗 / 提交校验”可被真实操作覆盖，再在 M0-5 一并跑 T7–T25。
