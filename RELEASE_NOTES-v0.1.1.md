# QuickPhrase 快语 v0.1.1 发布说明

- 版本：`0.1.1`（`0.1.0` 的补丁）
- 构建：Windows x64，Rust nightly MSVC
- 产物：
  - `QuickPhrase-0.1.1-win-x64.zip`（EXE + README + LICENSE）
  - `QuickPhrase-0.1.1-win-x64.exe`（单文件，直接运行）
- EXE：`1,075,712` bytes
- ZIP：`590,089` bytes
- SHA256（zip）：`6348cfb6586ae0b0385d343ed4eef9a2259b24f83d707d79032d296aadeb0e7b`
- SHA256（exe）：`c0cc1252fe789426a30fed98b8001524c70d7d255ebd04c6249a9ae2b8dc63c3`
- 许可证：MIT
- 发布状态：补丁发布（2026-11-23）

## 本次修复

管理窗口删除短语后，列表会残留已删除的卡片（幽灵行）：数据只剩 2 条，界面还显示 4 张旧卡片，重启后消失。

- 列表行数收缩时由列表自身带擦除地重绘，不再只重绘父窗口
- owner-draw 列表收到背景擦除时，先用主题底色铺满客户区，再画条目
- 主面板筛选导致行数收缩时同样处理

数据格式没有变化；用新版 EXE 覆盖旧版即可保留全部短语、标签、设置与回收站。

## 安装

1. 解压 `QuickPhrase-0.1.1-win-x64.zip` 到任意目录；或直接下载单文件 `QuickPhrase-0.1.1-win-x64.exe`。
2. 运行 `QuickPhrase.exe`。无需管理员权限。
3. 默认数据目录：`%LocalAppData%\QuickPhrase`。
4. 便携模式：在 EXE 同目录放 `portable.txt`（或 `portable`），数据改写到 `data\` 子目录。

> 未做代码签名，Windows SmartScreen 可能提示“未知发布者”。选择“更多信息 → 仍要运行”即可。

## 功能

沿用 0.1.0：全局热键、标题/正文/标签/拼音检索、标签筛选与批量标签、短语专属快捷键、变量模板、三栏管理、回收站、本地 JSON 与 5 份备份、导入导出、便携模式。

完整功能与安全边界见 [`RELEASE_NOTES-v0.1.0.md`](RELEASE_NOTES-v0.1.0.md)。

## 验证

- `cargo fmt --check` PASS
- `cargo clippy --release` 0 警告
- `cargo test` 150/150
- panel-smoke 10/10、tag-batch-smoke 10/10（本构建）
- 幽灵行屏幕截图复现：6→2 删除后，修复前残留 4 张卡片，修复后只剩 2 行
- glm / kimi / qwen 初轮 + focused 复核全部 PASS
- 详细记录：`docs/reviews/manage-list-ghost-rows-20261123.md`

## 已知限制

- 未代码签名；SmartScreen 会提示未知发布者
- 无安装器，采用 zip / 单文件 EXE 便携分发
- 标签云只显示一行；含空格的标签需通过管理窗口下拉或批量标签操作
- 专属快捷键需手动输入组合，暂不提供录制控件
- 批量标签输入是临时草稿，切换筛选或关窗不会提示保存
- 导入/导出仅支持 QuickPhrase JSON；应用输入例外按 exe 文件名匹配
- 浏览器多行、终端、未知区域固定只复制；粘贴模式也受区域限制
- 托盘气泡失败没有降级提示

## 发布前最后检查

- [x] 三家复核：glm / kimi / qwen PASS
- [x] 打包并确认 zip / exe 哈希
- [x] 幽灵行修复屏幕截图复验
- [x] 创建 `v0.1.1` 标签与 GitHub Release