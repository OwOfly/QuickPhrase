# M0-4 验收记录（2026-11-18，复核修复后）

- 范围：恢复目标、G1–G12 守卫、`SendInput` Unicode、三态结果、输入策略、只复制路径
- 复核：r1 三家 CHANGES_REQUIRED → r2 闭环 + 新问题 → r3 **glm PASS、kimi PASS、qwen PASS**
- 复核依据：`docs/_ref/final-plan.md` v4.1 §7、§5.5、§8 M0-4

## 已实现

| 项 | 实现 |
| --- | --- |
| 输入策略 | 身份/能力/提权/完整性/控制字符/多行区域/长度 |
| Unicode 注入 | UTF-16 码元逐个 down/up，单次 `SendInput`；CR 换行；代理对逐单元 |
| 注入线程 | 面板提交后在工作线程运行守卫与注入，UI 线程只处理完成消息 |
| 只复制 | 策略或守卫拒绝时写 `CF_UNICODETEXT`，明确提示“原剪贴板已被替换” |
| 三态 | `Rejected` / `Accepted` / `Partial`；文案为“已发送，请确认目标已收到” |
| 注入自测 | 自建 EDIT，走完整守卫与 SendInput，全文精确比对 |
| 面板接线 | Enter **KeyUp** 才提交，确保 Enter 已释放；隐藏后恢复目标再注入 |

## 守卫

| 守卫 | 实现 |
| --- | --- |
| G1 目标存在 | `IsWindow(target)` |
| G2 进程未替换 | PID 的进程创建时间与快照比较 |
| G3 前台序列 | 监听在**捕获前**安装；按 HWND 允许清单（target + 本进程）；溢出即拒绝 |
| G4 前台恢复 | `SetForegroundWindow` + 10 ms 轮询 ≤300 ms；目标最小化未特殊处理（M0-5） |
| G5 焦点控件 | `GetGUIThreadInfo(tid).hwndFocus` 与快照比较 |
| G6 UIA 身份与能力 | `CompareElements` + 提交时重读 ControlType/IsEnabled/IsPassword/只读 |
| G7 能力 | 原生提交时复读 `ES_READONLY`；UIA 由 G6 复读 |
| G8 权限 | `target_higher` / 完整性未知只复制 |
| G9 修饰键 | 等待 Ctrl/Alt/Shift/Win/Enter 释放 ≤300 ms |
| G10 事件数 | `SendInput` 返回数；0 → `sendinput_blocked`，不足 → `sendinput_short` |
| G11 位置 | 仅同进程原生 EDIT 用 `EM_GETSEL` 比较；跨进程/UIA 记 `POSITION_UNVERIFIED` |
| G12 控制字符 | C0/C1（除 Tab/CR/LF）、U+2028/U+2029 只复制 |
| 最终复查 | 全部等待结束后、`SendInput` 前再次校验前台/焦点/UIA 身份 |

**重要安全修复**：`EM_GETSEL` 的指针参数不会被跨进程封送。`read_selection` 现在只对**本进程窗口**调用；跨进程原生控件不再发送该消息，避免目标进程写入任意地址，同时 G11 对跨进程目标降级为 `POSITION_UNVERIFIED`。

## 输入策略

| 条件 | 行为 |
| --- | --- |
| 空文本 / 身份未验证 / 能力非 text / 提权 / 完整性未知 | 只复制 |
| 禁止控制字符 | 只复制 |
| 含换行且区域 ≠ `text_editor` | 只复制 |
| 终端单行 | Unicode 注入 |
| > 2,000 UTF-16 单元 | 只复制 |
| Tab | 4 空格 |
| 换行（编辑区） | 归一为 CR |

## 实测

### 注入自测

```
inject accepted: 15 UTF-16 units, 30 events
inject selftest: outcome=已发送，请确认目标已收到 text="Injected 测试\r\n第二行"
inject-test: PASS
```

`--inject-test` 现在要求 `Outcome::Accepted` 且读回文本**完全相等**。

### 其它门禁

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --check` | PASS |
| `cargo clippy --release` | PASS（0 error / 0 warning） |
| `cargo test` | PASS **35/35**（策略边界新增 7 个） |
| `cargo build --release` | PASS |
| `scripts/inject-smoke.ps1` | PASS 4/4 |
| `scripts/panel-smoke.ps1` | PASS 10/10（测试禁真实注入） |

## 复核修复对照

| 复核发现 | 处置 |
| --- | --- |
| 跨进程 `EM_GETSEL` 指针未封送 | 仅同进程调用；跨进程位置记 `POSITION_UNVERIFIED` |
| 守卫到 `SendInput` 的 TOCTOU | 增加最终复查（前台/焦点/UIA）后才发送 |
| Enter 仍按下时面板已隐藏 | 改为 Enter **KeyUp** 提交 |
| G3 按 PID 放行、监听缺失放行、事件溢出 | 改 HWND 允许清单；监听缺失只复制；溢出拒绝 |
| G7 能力只在捕获时检查 | UIA 提交时复读 ControlType/Enabled/Password/ReadOnly；原生复读 `ES_READONLY` |
| 注入阻塞 UI | 工作线程执行，完成消息回 UI |
| COPY_ONLY 不恢复目标 | `run` 入口先恢复目标 |
| “已输入”文案过度声称 | 改为“已发送，请确认目标已收到” |
| `EmptyClipboard` 失败忽略 | 失败即返回复制失败 |
| 位置不可比无记录 | 写 `POSITION_UNVERIFIED` 日志 |
| 空文本被当成功 | 空文本只复制 |
| `SendInput` 0 事件归为部分 | 单列 `sendinput_blocked` |
| 自测只做包含匹配 | 改为全文相等 + `Accepted` |

## 未覆盖 / 延后

- **跨应用矩阵**：Chrome、VS Code、Word、微信、终端等真实宿主未执行；属 M0-5。
- **守卫负例自动化**：`target_gone`/`process_changed`/`foreground_sequence`/`focus_changed`/`modifiers_held` 等只有代码路径，无自动化反例。
- **真实中文 IME**、真实 A→B→A、目标最小化、2,001 字符真实剪贴板未实测。
- **剪贴板兼容输入模式**、序列号保护、恢复按钮属 M1-7。
- **UIA 位置端点比较**未实现；跨进程位置一律 `POSITION_UNVERIFIED`。
- 托盘气泡失败无降级反馈（`Shell_NotifyIcon` 返回值未消费）。
- `--inject-test` 为同进程自建 EDIT，不覆盖跨进程 UIPI/UIA 行为。

## 结论

M0-4 的注入核心、策略与守卫在代码层闭环，注入自测通过；真实跨应用行为仍由 M0-5 矩阵决定，当前不宣称 99% 成功率。

**复核闭环**：r3 focused 三家（glm / kimi / qwen）全部 PASS，S1–S5 全 CLOSED，无新 blocker/major。**M0-4 过门，M0-5 可开。**

残留（不阻塞）：托盘气泡失败无降级反馈；注入 worker 路径无端到端注入测试；守卫负例无自动化；跨应用矩阵待 M0-5。