# M0-5 实测发现（2026-11-18）

- 目标：用真实/半真实宿主验证 M0-4 的捕获与注入
- 证据：`scripts/host-matrix.ps1` + `scripts/host-probe.rs`
- 提交基线：M0-4 闭环 `fb02705`

## 1. 跨进程注入（通过）

独立进程的裸 Win32 `EDIT`（`scripts/host-probe.rs`）作为目标：

```
capture: identity=verified source=uia capability=text region=text_editor
         hwnd_class=QuickPhrase.HostProbe focus_class=Edit
         uia=[class=Edit type=50004 ...]
panel commit: index=0
position_unverified
inject accepted: 35 UTF-16 units, 70 events
inject outcome: 已发送，请确认目标已收到
```

- 目标读回文本与短语**完全一致**。
- 剪贴板哨兵未被改写（接受路径不碰剪贴板）。
- `position_unverified` 符合设计：跨进程不发 `EM_GETSEL`。

`host-matrix.ps1`：**10/10 PASS**。这是 M0-4 缺失的 worker 路径端到端证据。

## 2. 实测中发现的缺陷（已修）

### D1（major）原生 EDIT 被误判为 `capability=unknown`

- 现象：裸 Win32 `EDIT` 捕获为 `capability=unknown`，只能复制。
- 原因：`decide_capability` 要求 UIA 只读判定为 `Some(false)`，而普通 `EDIT` 只有
  `ValuePattern`、没有 `TextPattern`，`combine_read_only` 返回 `None`。
- 规格：`final-plan.md` §5.5 第 1 行明确「原生 Edit/RichEdit」可用，只读用
  `ES_READONLY`，身份用 `hwndFocus`，位置用 `EM_GETSEL`/`rcCaret`。
- 修复：`Gate.native_edit`（排除 `_WwG`）+ 原生分支；UIA 只用于否决显式
  `password`/`disabled`。提交时 `compare_element` 与 `final_check` 同样改用
  `context::native_edit_read_only`。
- 单测：`native_edit_uses_style_not_uia_read_only`、
  `native_edit_still_denies_explicit_password`。

### D2（major）面板激活过程中自我隐藏

- 现象：面板 `ShowWindow` 后立刻变为不可见；`present` 仍返回成功。
- 原因：`force_foreground` 附加到前一前台线程再分离时，`WM_ACTIVATE(WA_INACTIVE)`
  在“打开中”阶段同步送达，被当作“点击外部”而隐藏。
- 修复：`Panel.opening` 标志覆盖 `ShowWindow`→焦点校验→`UpdateWindow` 整段，
  期间忽略 `WA_INACTIVE`；失败路径先清标志再隐藏。
- 影响：真实桌面上若发生同样的激活抖动，面板会瞬间消失——属真实缺陷，不只测试环境。

## 3. 宿主实测结论（策略正确）

| 宿主 | 实测 | 结果 |
| --- | --- | --- |
| Win11 记事本 | `RichEditD2DPT`，UIA `type=50030` Document | `capability=document` → **只复制**（符合 §5.5） |
| WinForms `TextBox` | UIA `type=Pane`，无 TextPattern | `capability=unknown` → **只复制** |
| 裸 Win32 `EDIT` | UIA `type=50004`，`focus_class=Edit` | `capability=text` → **注入成功** |

结论：Win11 记事本已不是“原生编辑器”，V1 策略下只能复制。矩阵计划需按实测更新，
不能把记事本当作默认注入宿主。

## 4. 环境限制（不是代码缺陷，但影响自动矩阵）

- 本会话桌面拒绝把前台交给面板；自动化必须设
  `QUICKPHRASE_TEST_ALLOW_UNFOCUSED_PANEL=1`，否则面板按设计隐藏。
- `SetForegroundWindow` 对任意窗口不稳定；`host-matrix.ps1` 用
  `AttachThreadInput + BringWindowToTop + SetActiveWindow` 兜底。
- UIA 60 ms 预算对 .NET/WinForms provider 偏紧：连续两次超时会话内永久降级为
  只复制（fail-safe，但一次慢宿主会让本次运行剩余时间全变复制）。
  这是既定设计（M17），M0-5 只记录现象，不改阈值。

## 5. 复核

这两项修复经 r1 → r2 两轮独立复核（glm / kimi / qwen）：

- r1：glm PASS；kimi、qwen CHANGES_REQUIRED（原生插入点、只读冲突、样式读取 fail-open、`opening` 丢弃失活）。
- r2：**三家全部 PASS**，R1–R7 全 CLOSED，无新 blocker/major。
- 快照：`m0-5-native-edit-20261118-r2`，brief `12a47bd8…`，fix.diff `479a305d…`。

修复后门禁：`cargo test` **44/44**、clippy 干净、inject-smoke 4/4、
panel-smoke 10/10、host-matrix 10/10。

## 6. 仍需真实桌面执行

- Chrome / Edge 的 `textarea` 与 `contenteditable`
- Word（`_WwG`，Document → 预期只复制）
- VS Code 编辑器 vs 集成终端
- Windows Terminal / 微信
- 150% / 200% DPI、双屏
- 真实中文 IME 组合态

自动化矩阵已具备，需在可交互桌面上跑 `scripts/host-matrix.ps1` 与手工 T1–T26。