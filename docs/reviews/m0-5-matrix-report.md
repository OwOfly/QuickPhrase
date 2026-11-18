# M0-5 矩阵报告（进行中）

- 日期：2026-11-18
- 提交：`0eb5a09`
- 环境：Windows 11 x64，100% DPI，单屏
- 说明：自动化部分已执行；真实宿主与 DPI/IME 项待可交互桌面

## 1. 自动化已执行

### 1.1 跨进程裸 Win32 EDIT（`scripts/host-matrix.ps1`）

| 检查 | 结果 |
| --- | --- |
| 宿主窗口创建 | PASS |
| 宿主获得焦点 | PASS |
| 面板打开 | PASS |
| 面板搜索框存在 | PASS |
| 捕获 `identity=verified` | PASS |
| 捕获 `capability=text` | PASS |
| `SendInput` 接受 | PASS（35 UTF-16 单元 / 70 事件） |
| 终态上报 | PASS |
| 目标文本完全一致 | PASS |
| 剪贴板哨兵未被改写 | PASS |

**10/10 PASS**。日志：`position_unverified` → `inject accepted` → 目标读回一致。

### 1.2 本进程自测（`--inject-test` / `inject-smoke.ps1`）

- 4/4 PASS。CR 换行、两行文本读回完全一致。

### 1.3 面板（`panel-smoke.ps1`）

- 10/10 PASS（测试禁注入）。

## 2. 宿主实测结论

| 宿主 | 实测类型 | 策略结果 | 状态 |
| --- | --- | --- | --- |
| 裸 Win32 `Edit`（独立进程） | UIA `Edit` 50004 | `text` → 注入成功 | 已验证 |
| Win11 记事本 | `RichEditD2DPT` / UIA Document | `document` → 只复制 | 已验证 |
| WinForms `TextBox` | UIA `Pane` | `unknown` → 只复制 | 已验证 |
| Chrome `<textarea>` | 未测 | 预期 `text` | 待真实桌面 |
| Chrome `contenteditable` | 未测 | 预期 `text`/`unknown` | 待真实桌面 |
| Word `_WwG` | 未测 | 预期 `document` → 只复制 | 待真实桌面 |
| VS Code 编辑器 | 未测 | 预期 `text` | 待真实桌面 |
| VS Code 集成终端 | 未测 | 预期 `terminal` | 待真实桌面 |
| Windows Terminal | 未测 | 预期 `terminal` | 待真实桌面 |
| 微信输入框 | 未测 | 预期 `unknown` → 只复制 | 待真实桌面 |

## 3. 反例矩阵

T1–T26 尚无逐条结论。已由代码/单测覆盖的部分：

| # | 覆盖方式 | 状态 |
| --- | --- | --- |
| T3 修饰键未释放 | G9 代码路径，无自动化 | 待实测 |
| T4 终端多行 | 策略单测 `multiline_unknown_region_is_copy_only` | 单测通过 |
| T5 部分接受 | `sendinput_short` / `sendinput_blocked` 分支 | 待故障注入 |
| T7/T8 UIA 超时 | 60 ms 超时、连续两次降级（实测 WinForms 触发） | 观察到 |
| T9 终端多行 | 策略单测 | 单测通过 |
| T11 只读/ValuePattern-only | 单测 `native_edit_still_denies_explicit_read_only` 等 | 单测通过 |
| T16 目标退出 | G1/G2 代码路径 | 待实测 |
| T17 恢复失败 | G4 + `IsIconic` 未实现 | 待实测 |
| T19 位置变化 | `position_unverified` 已实测；不一致拒绝未测 | 部分 |
| T23 控制字符 | 策略单测 | 单测通过 |
| T26 剪贴板覆盖 | 气泡文案明确“原剪贴板已被替换” | 待人工确认 |

## 4. 横切

| # | 项 | 状态 |
| --- | --- | --- |
| X1 150%/200% DPI | 未测 |
| X2 双屏/负坐标 | 未测 |
| X3 连续 20 次 | 未测 |
| X4 IME 组合中 Enter | 代码守卫 + 单测，真实 IME 未测 |
| X5 IME 组合后 Enter | 未测 |
| X6 气泡失败降级 | 未实现（残留） |

## 5. 实验 E2/E3/E4

| 实验 | 状态 |
| --- | --- |
| E2 VS Code 编辑器 vs 终端 | 未执行（无 VS Code 会话） |
| E3 捕获延迟/0-HWND 比例 | 未采集 100 次样本 |
| E4 位置对比覆盖率 | 跨进程恒 `POSITION_UNVERIFIED`；原生同进程可对比 |

## 6. 结论

- 自动化可证部分全部通过；无错误窗口/控件写入。
- 真实宿主、DPI、IME、反例矩阵仍需在可交互桌面执行。
- 未命中发布阻断项，但样本不足，**不宣称 M0-5 完成**。