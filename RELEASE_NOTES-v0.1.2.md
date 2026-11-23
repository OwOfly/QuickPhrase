# QuickPhrase 快语 v0.1.2 发布说明

- 版本：`0.1.2`（`0.1.1` 的补丁）
- 构建：Windows x64，Rust nightly MSVC
- 产物：
  - `QuickPhrase-0.1.2-win-x64.zip`（EXE + README + LICENSE）
  - `QuickPhrase-0.1.2-win-x64.exe`（单文件，直接运行）
- EXE：`1,075,712` bytes
- ZIP：`590,050` bytes
- SHA256（zip）：`667c2a5986a566c0b71a204de4ecaf003612767d3fba22e18122587c4d2d146c`
- SHA256（exe）：`70b9b133fe9053ee6d37198ba3b425a05cf80f4a9de3e2f86db0e5c80d62edb0`
- 许可证：MIT
- 发布状态：补丁发布（2026-11-23）

## 本次修复

“短语管理”窗口点最小化后从桌面消失，任务栏没有图标，无法恢复。

- 管理窗口的 owner 是隐藏主窗口，Windows 默认不给它任务栏按钮；现在显式加 `WS_EX_APPWINDOW`
- 最小化后任务栏保留 QuickPhrase 图标，点击图标恢复
- 补上“最小化 → 关闭 → 从托盘重新打开”的恢复分支：窗口仍处于最小化时用 `SW_RESTORE`，不再只亮任务栏按钮

0.1.1 的“删除短语后列表残留旧卡片（幽灵行）”修复也包含在本版本中。

## 安装

1. 解压 `QuickPhrase-0.1.2-win-x64.zip` 到任意目录；或直接下载单文件 `QuickPhrase-0.1.2-win-x64.exe`。
2. 运行 `QuickPhrase.exe`。无需管理员权限。
3. 默认数据目录：`%LocalAppData%\QuickPhrase`。
4. 便携模式：在 EXE 同目录放 `portable.txt`（或 `portable`），数据改写到 `data\` 子目录。

> 未做代码签名，Windows SmartScreen 可能提示“未知发布者”。选择“更多信息 → 仍要运行”即可。

## 功能

沿用 0.1.0 / 0.1.1：全局热键、标题/正文/标签/拼音检索、标签筛选与批量标签、短语专属快捷键、变量模板、三栏管理、回收站、本地 JSON 与 5 份备份、导入导出、便携模式。

完整功能与安全边界见 [`RELEASE_NOTES-v0.1.0.md`](RELEASE_NOTES-v0.1.0.md)。

## 验证

- `cargo fmt --check` PASS
- `cargo clippy --release` 0 警告
- `cargo test` 150/150
- panel-smoke 10/10、tag-batch-smoke 10/10（本构建）
- 任务栏验证：exstyle `0x40100`；最小化后 `IsIconic=true` 且任务栏出现 QuickPhrase 图标；恢复后 `IsIconic=false`
- 任务栏截图：`dist/ui-shots/taskbar-manage-minimized-012.png`
- glm / kimi / qwen 复核 PASS；glm 提出的“最小化→关闭→托盘重开”问题已修并 focused PASS
- 详细记录：`docs/reviews/manage-taskbar-20261123.md`

## 已知限制

- 未代码签名；SmartScreen 会提示未知发布者
- 无安装器，采用 zip / 单文件 EXE 便携分发
- 标签云只显示一行；含空格的标签需通过管理窗口下拉或批量标签操作
- 专属快捷键需手动输入组合，暂不提供录制控件
- 批量标签输入是临时草稿，切换筛选或关窗不会提示保存
- 导入/导出仅支持 QuickPhrase JSON；应用输入例外按 exe 文件名匹配
- 浏览器多行、终端、未知区域固定只复制；粘贴模式也受区域限制
- 托盘气泡失败没有降级提示
- `Ctrl+Alt+Q` 快捷面板仍是临时弹窗，不进任务栏（设计如此）

## 发布前最后检查

- [x] 三家复核：glm / kimi / qwen PASS
- [x] 打包并确认 zip / exe 哈希
- [x] 任务栏最小化/恢复与截图验证
- [x] 创建 `v0.1.2` 标签与 GitHub Release