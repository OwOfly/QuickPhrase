# M0-5 真实宿主矩阵计划

- 目标：用真实宿主验证 M0-4 的注入策略与守卫，产出可复现报告
- 依据：`docs/_ref/final-plan.md` §8 M0-5、§9.3 反例表 T1–T26
- 前置：M0-4 复核闭环（r3：glm/kimi PASS，qwen 待回）
- 性质：手工矩阵 + 日志取证；不新增自动注入路径

## 1. 环境登记（每份报告必填）

| 项 | 记录 |
| --- | --- |
| Windows 版本 / 内部版本 | `winver` |
| 显示器数量、分辨率、DPI | 100% / 150% / 200% |
| QuickPhrase 提交 | `git rev-parse --short HEAD` |
| 测试日期与操作者 | — |
| 日志路径 | `%LocalAppData%\QuickPhrase\logs\qp-YYYY-MM-DD.log` |

## 2. 宿主清单

| # | 宿主 | 版本来源 | 目标控件 | 预期区域 | 预期结果 |
| --- | --- | --- | --- | --- | --- |
| H1 | 记事本（Win11） | `Notepad` / `RichEditD2DPT` | 编辑区 | `text_editor`（UIA Document） | **只复制**（实测） |
| H2 | Word | `OpusApp` / `_WwG` | 文档正文 | `text_editor`（UIA Document） | 仅单行注入；多行只复制 |
| H3 | VS Code | `Code.exe` / Monaco | 编辑器 | `text_editor`（UIA Text） | 单行注入 |
| H4 | VS Code | 集成终端 | 终端 | `terminal` | 单行注入；多行只复制 |
| H5 | Chrome | `Chrome_WidgetWin_1` | `<textarea>` | 待定 | 单行注入；多行只复制 |
| H6 | Chrome | 同上 | `contenteditable` | `text_editor`/`unknown` | 单行按实际区域判定 |
| H7 | Edge | `Chrome_WidgetWin_1` | 输入框 | 同 Chrome | 同 Chrome |
| H8 | Windows Terminal | `CASCADIA_HOSTING_WINDOW_CLASS` | 终端 | `terminal` | 单行注入；多行只复制 |
| H9 | 微信 | `WeChatMainWndForPC` | 聊天输入框 | `unknown` | 只复制（预期） |
| H10 | 本程序自测 | `--inject-test` | 自建 EDIT | `text_editor` | 全文读回一致 |
| H11 | 独立进程探针 | `scripts/host-probe.rs` | 裸 Win32 `Edit` | `text_editor` | 跨进程注入成功（已实测 10/10） |

> 区域判定由 `capture::region_from` 用 exe/class/UIA hint 推导。**报告必须记录实际判定的 region**，不以预期替代实测。

## 3. 核心场景（8 项，每项 5 次）

| # | 场景 | 输入 | 通过判据 |
| --- | --- | --- | --- |
| S1 | 记事本单行 | `你好 QuickPhrase` | 文本落点正确；无多余按键；日志 `inject accepted` |
| S2 | 记事本多行 | `第一行\n第二行` | 目标内为两行；发送 CR 而非 LF；无自动执行 |
| S3 | Word 单行 | 同上单行 | 落点正确；无格式破坏 |
| S4 | Word 多行 | 两行文本 | **只复制**（Document 非 `text_editor` 原生编辑器策略）；气泡明确提示 |
| S5 | VS Code 编辑器单行 | 中文 + Emoji | Emoji 正常；无乱码 |
| S6 | VS Code 集成终端单行 | `Get-Date` | **不自动回车**；命令未被自动执行 |
| S7 | Chrome `<textarea>` 单行 | 中文 | 落点正确 |
| S8 | Windows Terminal 单行 | `echo hi` | 不自动执行 |

每项记录：capture 日志行、inject 日志行、气泡文案、截图。

## 4. 反例矩阵（T1–T26）

按 `final-plan.md` §9.3 逐条执行，重点：

| # | 操作 | 合格结果 |
| --- | --- | --- |
| T1 | Chrome 同页两个输入框，焦点移走后提交 | 不写入第二个框；不确定只复制 |
| T2 | 面板期间刷新页面 / 控件重建 | 拒绝注入 |
| T3 | 按住 Enter 或 Ctrl 时提交 | 不发送（G9） |
| T4 | 终端多行 | 只复制 |
| T5 | `SendInput` 部分接受 | 报可能部分输入，不重试 |
| T6 | 剪贴板含图片/HTML | 明确告知原内容不可恢复 |
| T7 | 人为让 UIA 延迟 300–500 ms | 捕获超时 → 只复制 |
| T8 | UIA 永久阻塞 > 2 s | UI 不卡；只复制 |
| T9 | VS Code 编辑器 vs 终端 | 终端多行只复制 |
| T10 | 网页终端（xterm） | 多行不注入 |
| T11 | 只读控件 / ValuePattern-only | 只复制 |
| T12 | 捕获超时后继续操作 | 不复用旧快照 |
| T13 | 通知/弹窗抢焦点 | 捕获作废或提交拒绝 |
| T14 | 模态对话框 | 不注入对话框 |
| T15 | 同应用切标签/文档 | 身份不匹配即拒绝 |
| T16 | 注入中目标退出 | `部分/未知`，不重试 |
| T17 | 目标最小化 / 激活受限 | 拒绝并只复制 |
| T18 | Chrome 内层 `NativeWindowHandle=0` | 上溯成功 → verified；失败 → 只复制 |
| T19 | 面板期间光标/选区被改 | 可对比不一致 → 拒绝；不可对比 → `position_unverified` 继续 |
| T20 | TextPattern 可用、ValuePattern 缺失 | 不当可编辑；只复制 |
| T21 | UIA 元素重建、RuntimeId 相同 | `CompareElements` 失败即拒绝 |
| T22 | UIA 挂起时连按热键 | 线程有上限；UI 可用；结果不串 |
| T23 | 终端文本含 ESC/U+2028 | 只复制或明确拒绝 |
| T24 | 首次运行、无 JSON | 正确落盘，重启后仍在 |
| T25 | 面板期间前台 A→B→A | 不因最终 HWND 相同而放行 |
| T26 | COPY_ONLY 覆盖剪贴板 | 有明确提示；纯文本可恢复；非文本告知不可恢复 |

## 5. 横切检查

| # | 项 | 判据 |
| --- | --- | --- |
| X1 | 150% / 200% DPI | 面板不越界；注入落点正确 |
| X2 | 双屏、负坐标 | 面板出现在光标所在屏 |
| X3 | 连续 20 次热键 + 提交 | 无泄漏、无错窗、无卡顿 |
| X4 | 中文 IME：组合中按 Enter | 只确认候选，不提交短语 |
| X5 | 中文 IME：组合后按 Enter | 正常提交短语 |
| X6 | 托盘气泡失败降级 | 至少日志可见（当前残留） |

## 6. 实验（E2/E3/E4）

| 实验 | 方法 | 产出 |
| --- | --- | --- |
| E2 | VS Code 编辑器与集成终端各 50 次捕获 | region 标记是否稳定可复现 |
| E3 | 每宿主 100 次捕获 | UIA 延迟分布、`NativeWindowHandle==0` 比例、上溯成功率 |
| E4 | 原生 + UIA 目标各 50 次提交 | 位置对比覆盖率，决定 M1 是否收紧“严格原位” |

## 7. 报告格式

```markdown
# M0-5 矩阵报告 <日期> <commit>

## 环境
## 宿主版本
## 核心场景
| # | 宿主 | region 实测 | 结果 | 证据 |
## 反例 T1–T26
| # | 结果 | 证据 |
## 横切 X1–X6
## 实验 E2/E3/E4
## 缺陷
| ID | 严重度 | 场景 | 现象 | 日志 |
## 结论
发布阻断项是否命中：
```

## 8. 通过标准

1. `AUTO_INSERT` 格零错窗、零错控件写入。
2. `COPY_ONLY` / `UNSUPPORTED` 格零注入。
3. 聊天软件（微信）不自动发送。
4. 终端不自动执行。
5. T1–T26 全部有结论（通过 / 明确失败 / 环境不可复现并注明）。
6. E2/E3/E4 数据可复现。

任一条不满足 → 不进 M1，先修。