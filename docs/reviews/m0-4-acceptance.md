# M0-4 验收记录（2026-11-18）

- 范围：恢复目标、G1–G12 守卫、`SendInput` Unicode、三态结果、输入策略、只复制路径
- 复核：待独立复核（M0-3 已过门）
- 复核依据：`docs/_ref/final-plan.md` v4.1 §8 M0-4、§7、§5.5

## 已实现

| 项 | 实现 |
| --- | --- |
| 输入策略 | `inject::plan_text`：身份/能力/提权/完整性/控制字符/多行区域/长度 |
| Unicode 注入 | UTF-16 码元逐个 down/up，单次 `SendInput`；CR 换行；Emoji 代理对逐单元 |
| 只复制 | 策略或守卫拒绝时写 `CF_UNICODETEXT` 并提示；不重试 |
| 三态 | `Rejected` / `Accepted` / `Partial`；`Accepted` 只代表事件进入输入流 |
| 注入自测 | `--inject-test` 自建 EDIT，走完整守卫与 SendInput，再读回比对 |
| 面板接线 | Enter 隐藏面板 → 运行守卫与注入 → 托盘气泡报告终态 |

## 守卫

| 守卫 | 实现 |
| --- | --- |
| G1 目标存在 | `IsWindow(target)` |
| G2 进程未替换 | 重新读取 PID 的进程创建时间，与快照比较 |
| G3 前台序列 | 面板生命周期内 `EVENT_SYSTEM_FOREGROUND` 事件，出现第三方 PID 即拒绝 |
| G4 前台恢复 | `SetForegroundWindow` + 10 ms 轮询 ≤300 ms |
| G5 焦点控件 | `GetGUIThreadInfo(tid).hwndFocus` 与快照 `focused_hwnd` 比较 |
| G6 UIA 身份 | `identity_source=uia` 时 `CompareElements` 必须为真 |
| G7 能力 | `capability` 必须为 `text` |
| G8 权限 | `target_higher` / 完整性未知一律只复制 |
| G9 修饰键 | 等待 Ctrl/Alt/Shift/Win/Enter 释放 ≤300 ms |
| G10 事件数 | `SendInput` 返回数必须等于请求数 |
| G11 位置 | 原生 EDIT 用 `EM_GETSEL` 比较快照与提交时选区；不一致拒绝 |
| G12 控制字符 | C0/C1（除 Tab/CR/LF）、U+2028/U+2029 一律只复制 |

## 输入策略

| 条件 | 行为 |
| --- | --- |
| 身份未验证 / 能力非 text / 提权 / 完整性未知 | 只复制 |
| 含禁止控制字符 | 只复制 |
| 含换行且区域 ≠ `text_editor` | 只复制 |
| 终端单行 | Unicode 注入 |
| 文本 > 2,000 UTF-16 单元 | 只复制 |
| Tab | 转 4 空格 |
| 换行（编辑区） | 归一为 CR |
| Emoji / 非 BMP | 逐 UTF-16 单元发送代理对 |

## 实测

### 注入自测

```
inject accepted: 15 UTF-16 units, 30 events
inject selftest: outcome=已输入 text="Injected 测试\r\n第二行"
inject-test: PASS
```

`scripts/inject-smoke.ps1`：**4/4 PASS**。

### 其它门禁

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --check` | PASS |
| `cargo clippy --release` | PASS（0 error / 0 warning） |
| `cargo test` | PASS 28/28（新增 6 个策略单测） |
| `cargo build --release` | PASS |
| M0-3 面板冒烟 | PASS 10/10（测试用 `QUICKPHRASE_TEST_NO_INJECT=1` 禁止真实注入） |

## 未覆盖 / 延后

- **跨应用矩阵**：Chrome、VS Code、Word、微信、终端等真实宿主未执行；属 M0-5。
- **剪贴板兼容输入模式**：`Ctrl+V` 路径、序列号保护、恢复按钮属 M1-7。
- **UIA 位置对比**：仅原生 EDIT 的 `EM_GETSEL` 已实现；UIA 控件记 `POSITION_UNVERIFIED`。
- **G3 事件序列**：代码已接线，未做真实 A→B→A 注入阻断测试。
- **真实中文 IME**：组合态守卫已加，未实测。
- **只复制反馈**：托盘气泡；面板内 copy-only 页脚状态留 M1-3。
- 注入矩阵中“长文本 >2,000 单元”只做了策略单测，未做真实 2,001 字符剪贴板验证。

## 结论

M0-4 的注入核心路径通过自测，策略与守卫具备离线单测；真实跨应用行为必须由 M0-5 矩阵决定，当前不宣称 99% 成功率。