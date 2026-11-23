# QuickPhrase 快语 v0.1.0 发布说明

- 版本：`0.1.0`
- 构建：Windows x64，Rust nightly MSVC
- 产物：`QuickPhrase-0.1.0-win-x64.zip`
- EXE：`1,074,688` bytes
- ZIP：`590,171` bytes
- SHA256：`20a1ea3f2da94b20295f35cfa93fd8e0fa4de477a80368839174d4701267980d`
- 许可证：MIT
- 发布状态：自动化门禁已通过；真实桌面矩阵待人工验收

## 安装

1. 解压 `QuickPhrase-0.1.0-win-x64.zip` 到任意目录。
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
- 无安装器，采用 zip 便携分发
- 真实跨应用矩阵（Chrome / Word / VS Code / 终端 / 微信）、IME、125/150/200% DPI 仍需最终人工验收
- 标签云只显示一行；含空格标签只能用管理窗口下拉/批量标签
- 专属快捷键需手动输入组合，暂不提供录制控件
- 批量标签输入为临时草稿，切换筛选/关窗不提示

## 验证

- `cargo fmt --check` PASS
- `cargo clippy --release` 0 警告
- `cargo test` 150/150
- panel-smoke 10/10、tray-smoke 19/19、tag-batch-smoke 10/10
- host-matrix 10/10、snippet-hotkey-smoke 16/16（本构建 `20a1ea…`）
- 复核记录：`docs/reviews/` 下阶段三各模块记录

## 发布前最后检查

- [x] 解锁桌面后跑 `scripts\host-matrix.ps1 -SkipBuild`
- [x] 跑 `scripts\snippet-hotkey-smoke.ps1 -SkipBuild`
- [ ] 手工验收：记事本 / Chrome / VS Code / 终端 / 微信，IME，125/150/200% DPI
- [ ] 复测管理窗口灰框问题
- [ ] 确认 zip 哈希后创建 `v0.1.0` 标签
- [ ] 选择发布渠道（本地分发 / GitHub Releases / Gitee Releases）