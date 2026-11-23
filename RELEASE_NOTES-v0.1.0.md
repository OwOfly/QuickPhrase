# QuickPhrase 快语 v0.1.0 发布说明

- 版本：`0.1.0`
- 构建：Windows x64，Rust nightly MSVC
- 产物：
  - `QuickPhrase-0.1.0-win-x64.zip`（EXE + README + LICENSE）
  - `QuickPhrase-0.1.0-win-x64.exe`（单文件，直接运行）
- EXE：`1,075,712` bytes
- ZIP：`590,329` bytes
- SHA256（zip）：`4e89ba752a5d0ab5908af73f840443dc961db93d29e730371a15097420f8122a`
- SHA256（exe）：`1c1f89fe7da47a8b488dadd2c259d3efe43901cda309889b770827b56eefd187`
- 许可证：MIT
- 发布状态：人工桌面验收已通过（2026-11-23）；正式 `v0.1.0` 发布
- 发布页：https://github.com/OwOfly/QuickPhrase/releases/tag/v0.1.0

## 安装

1. 解压 `QuickPhrase-0.1.0-win-x64.zip` 到任意目录；或直接下载单文件 `QuickPhrase-0.1.0-win-x64.exe`。
2. 运行 `QuickPhrase.exe`。
3. 默认数据目录：`%LocalAppData%\QuickPhrase`。
4. 便携模式：在 EXE 同目录放 `portable.txt`（或 `portable`），数据改写到 `data\` 子目录。

## 功能

- 全局热键 `Ctrl+Alt+Q` 唤起快捷面板；可录制更换
- 标题/正文/标签/拼音检索；最近使用/最常使用排序；置顶
- 变量模板 `{date}`、`{time}`、`{datetime}`
- 管理窗口 CRUD、分类、回收站、标签编辑
- 标签筛选、面板标签云、多标签 AND、批量添加/移除标签、全选当前筛选
- 短语专属快捷键，一键直出；目标不确定时只复制，不自动重试
- 按应用输入例外（Unicode / 粘贴模式）
- 本地 JSON 存储、原子写、5 份备份、导入导出、30 天回收站
- 托盘、暂停/恢复、剪贴板恢复、只读模式

## 安全边界

- 全离线，无网络、无遥测、无键盘钩子
- 注入只针对已验证的可编辑目标；权限更高、身份不明、终端多行、超长或控制字符一律只复制
- `SendInput` 失败或目标切换不会自动重试

## 已知限制

- 未代码签名，Windows SmartScreen 可能提示未知发布者
- 无安装器，采用 zip / 单文件 EXE 便携分发
- 标签云只显示一行；含空格标签只能用管理窗口下拉/批量标签
- 专属快捷键需手动输入组合，暂不提供录制控件
- 批量标签输入为临时草稿，切换筛选/关窗不提示

## 验证

- `cargo fmt --check` PASS
- `cargo clippy --release` 0 警告
- `cargo test` 150/150
- panel-smoke 10/10、tag-batch-smoke 10/10（本构建 `4e89ba…`）
- tray-smoke 19/19、host-matrix 10/10、snippet-hotkey-smoke 16/16（`a908f8e` 构建；本次仅列表绘制改动，未触及托盘/注入路径）
- 复核记录：`docs/reviews/` 下阶段三各模块记录

## 本次修复（2026-11-23）

- 修复列表选中卡片右侧被滚动条车道背景刷掉的问题：列表不可滚动时不再绘制右侧车道，卡片右边框与圆角完整。
- 影响范围：管理窗口短语列表、左侧分类列表、主面板列表共用同一滚动条绘制逻辑。
- 复核：glm / kimi / qwen 三家 PASS（kimi/qwen 提出的截图路径状态边界已修复并复验）；细节见 `docs/reviews/manage-card-right-edge-20261123.md`。

## 发布前最后检查

- [x] 解锁桌面后跑 `scripts\host-matrix.ps1 -SkipBuild`
- [x] 跑 `scripts\snippet-hotkey-smoke.ps1 -SkipBuild`
- [x] 手工验收：记事本 / Chrome / VS Code / 终端 / 微信，IME，125/150/200% DPI（2026-11-23 通过）
- [x] 复测管理窗口灰框问题（同次验收通过）
- [x] 确认 zip 哈希并创建 `v0.1.0` 标签
- [x] 发布渠道：GitHub Releases（`OwOfly/QuickPhrase`）