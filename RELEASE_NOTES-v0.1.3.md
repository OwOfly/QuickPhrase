# QuickPhrase 快语 v0.1.3 发布说明

- 版本：`0.1.3`（`0.1.2` 的界面补丁）
- 构建：Windows x64，Rust nightly MSVC
- 产物：
  - `QuickPhrase-0.1.3-win-x64.zip`（EXE + README + LICENSE）
  - `QuickPhrase-0.1.3-win-x64.exe`（单文件，直接运行）
- EXE：`1,095,680` bytes
- ZIP：`600,078` bytes
- SHA256（zip）：`7a2ac86b98396c705138a8348ab16809501e300c654ddf774a98a11dffdc2942`
- SHA256（exe）：`945f2c236fe765e70b030e1cefdc96cc342e5bd7f32f71d34fa50c06de1f6972`
- 许可证：MIT
- 发布状态：补丁发布（2026-11-23）
- 发布页：https://github.com/OwOfly/QuickPhrase/releases/tag/v0.1.3

## 本次改进

“短语管理”的确认框与下拉框改为与现有 Fluent / Direct2D 风格一致：

- 原生 MessageBox 换成自绘确认卡片：警示图标、标题、说明、取消与危险按钮；初始焦点在“取消”，Esc / IDCANCEL 取消；移入回收站、彻底删除、删除分类、放弃修改共用。
- “全部标签 / 所属分类”的原生下拉换成 D2D 浮层：宽度对齐锚点，最多 8 行，选中行带勾，悬停高亮，滚轮滚动和可拖动的胶囊滚动条；外部点击、Esc、Tab、Alt+Tab 关闭。
- 键盘：F4 / Alt+Down 打开，Up / Down / Home / End / Enter 选择；关闭后焦点回到下拉框。
- 修复原生下拉抑制导致第二次键盘或鼠标打不开的问题：子类化拦截打开手势，原生 ComboBox 的“已打开”状态不再被污染。
- 修复弹层与卡片的 `WM_CLOSE`、`WM_QUIT` 重投、重复销毁、嵌套确认、焦点恢复、Alt+Tab 失活。
- 滚动或拖动后按 Down，视图会回到键盘高亮所在行，Enter 不会选中不可见项。
- 标签被导入删除时，打开下拉会同步重置筛选与中栏列表；删除确认使用已保存标题，不再引用未保存草稿。
- 模态打开期间，管理窗口收到关闭请求会被忽略，关闭弹层后再正常关闭。

## 验证

- `cargo fmt --check` PASS
- `cargo clippy --release` 0 警告
- `cargo test` 153/153（新增 3 个滚动回归测试）
- panel-smoke 10/10、tag-batch-smoke 10/10（本构建）
- 真实鼠标/键盘：连续点击开合、F4 连续三次、Alt+Down、Down+Enter 选择、滚轮/拖拽到底后 Down 回卷
- 确认卡：Esc 不删除；确认后 active 6→5、回收站 0→1
- 模态期间 manage 的 `WM_CLOSE` 被忽略；关闭弹层后窗口正常关闭、进程退出
- glm / kimi / qwen 复核 PASS（r4）

## 安装

1. 解压 `QuickPhrase-0.1.3-win-x64.zip` 到任意目录；或直接下载单文件 `QuickPhrase-0.1.3-win-x64.exe`。
2. 运行 `QuickPhrase.exe`。无需管理员权限。
3. 默认数据目录：`%LocalAppData%\QuickPhrase`。
4. 便携模式：在 EXE 同目录放 `portable.txt`（或 `portable`），数据改写至 `data\` 子目录。

替换 EXE 不会改动 `%LocalAppData%\QuickPhrase` 下的短语、设置和备份。

> 未做代码签名，Windows SmartScreen 可能提示“未知发布者”。选择“更多信息 → 仍要运行”即可。

## 功能

沿用 0.1.0 / 0.1.1 / 0.1.2：全局热键、标题/正文/标签/拼音检索、标签筛选与批量标签、短语专属快捷键、变量模板、三栏管理、回收站、本地 JSON 与 5 份备份、导入导出、便携模式、管理窗口任务栏与最小化恢复。

完整功能与安全边界见 [`RELEASE_NOTES-v0.1.0.md`](RELEASE_NOTES-v0.1.0.md)。

## 已知限制

- 未代码签名；SmartScreen 会提示未知发布者。
- 无安装器，采用 zip / 单文件 EXE 便携分发。
- 标签云只显示一行；含空格的标签需要通过管理窗口下拉或批量标签操作。
- 专属快捷键需手动输入组合，暂不提供录制控件。
- 批量标签输入是临时草稿，切换筛选或关窗不会提示保存。
- 导入/导出仅支持 QuickPhrase JSON；应用输入例外按 exe 文件名匹配，不支持通配符或窗口标题。
- 浏览器多行、终端、未知区域固定只复制；粘贴模式也受区域限制。
- 托盘气泡失败没有降级提示。
- 确认框与下拉是自绘表面，暂无像素级自动化回归。