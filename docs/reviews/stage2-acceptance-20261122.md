# 阶段二验收收尾（2026-11-22）

- 基线：`a3394d6`（按应用输入例外）
- 目标：确认阶段二功能与发布包可交付，列出仍需交互桌面完成的门禁
- 发布包：`dist/QuickPhrase-0.1.0-win-x64.zip`，SHA256 `c5e7864afccf5b36abc1e35c84a184c0182b0a4e71eaa5ba78f0589b24db3045`

## 自动化门禁

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --check` | PASS |
| `cargo clippy --release` | PASS（0 警告） |
| `cargo test` | PASS 134/134 |
| `panel-smoke` | PASS 10/10（偶发 Start 菜单抢焦点，重跑通过） |
| `tray-smoke` | PASS 19/19 |
| `package.ps1` | PASS：exe 1,021,952 bytes；zip 563,881 bytes |

## 跨进程注入实测

`scripts/host-matrix.ps1` 使用独立 `host-probe.exe` 的纯 Win32 EDIT：

```
set LOCALAPPDATA=<临时目录>; scripts/host-matrix.ps1 -SkipBuild
PASS: 10 checks
inserted text matches exactly
clipboard untouched on accepted path
```

- 使用临时 `LOCALAPPDATA` 隔离数据目录，未读写用户真实数据。
- 验证：面板打开、UIA 捕获 `text_editor`、Enter 提交、注入文本完全一致、剪贴板未被修改。
- 说明：先前用真实测试数据跑同一脚本时，最近使用排序使首条不是硬编码期望文本，脚本报文本不一致；这是测试断言依赖数据顺序，不是注入错误。隔离数据后通过。

## 编辑框灰显修复复测

- 已提交 `64f70c4`：移除 `ES_NOHIDESEL`、`WM_CTLCOLOREDIT` 改 OPAQUE、方向键/焦点后重绘、激活/载入恢复启用。
- 自动化尝试：加载 21 字正文、光标置尾、发送左方向键并截图；修复前后均未能复现用户截图的整框灰显，文字保持可见。
- 结论：防御修复已落地，但**根因与修复效果仍待用户交互桌面复测**；若复现，需要操作顺序/录屏。

## 按应用例外

- 设置页新增“应用输入例外”，规则写 `rules-user.json`，按 exe basename 匹配。
- 已用脚本向 exe 编辑框发送键盘字符 `notepad.exe` 并点击添加，日志确认写入；测试规则已清理。
- 规则模式已进入 `inject::Options`，与面板提示/copy-only 同源。
- **未验证**：真实应用规则覆盖后的 Unicode/粘贴注入矩阵；需用 Chrome/记事本等实测。

## 仍需交互桌面验收（阻断发布标签）

- Chrome 普通输入框/textarea/contenteditable、Word、VS Code 编辑器、Windows Terminal、微信输入框：单行/多行/选区/IME。
- 全局 Unicode vs 粘贴模式 vs 应用规则两方向；终端/未知区域 copy-only；剪贴板恢复。
- 125/150/200% DPI 与多屏：面板/管理/设置/托盘菜单位置与文字裁切。
- 深色主题下设置应用例外页：原生 EDIT/COMBO 下拉列表、规则滚动条。
- 管理窗口正文编辑后按←/Home/End/失焦/切回：确认灰显不再出现。
- 杀软/SmartScreen/干净虚拟机首次运行；代码签名决策。

## 发布结论

- 代码、文档、发布包与哈希一致；`dist/SHA256SUMS.txt` 匹配。
- 暂不创建 `v0.1.0` 标签：上述交互桌面验收未完成，且仓库无远程、未做签名。完成矩阵后再执行 `scripts/package.ps1` 生成最终哈希并打标签。