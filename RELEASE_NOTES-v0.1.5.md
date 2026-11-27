# QuickPhrase 快语 v0.1.5 发布说明

- 版本：`0.1.5`（`0.1.4` 的两处界面重绘修复）
- 平台：Windows 10 21H2+ / Windows 11 x64
- 资产：`QuickPhrase-0.1.5-win-x64.zip`、`QuickPhrase-0.1.5-win-x64.exe`、`SHA256SUMS.txt`

## 修复

### 标签筛选 / 所属分类下拉

- 打开选择器时不再每次 `CB_RESETCONTENT` 重建原生组合框：内容未变只同步选中项；确需重建时用 `WM_SETREDRAW` 静默完成并只失效一次。
- 修复点击瞬间下拉框被画成空白、弹层随后才出现的闪动。

### 主题切换

- 设置窗口实时预览主题时只失效了顶层窗口，owner-draw 子按钮要等鼠标悬停触发 `WM_DRAWITEM` 才用新配色重画；切换后「取消」等按钮会短暂保留上一主题的底色。
- 改为 `RedrawWindow(..., RDW_INVALIDATE | RDW_ALLCHILDREN)`，父窗口与全部子控件在同一次重绘中更新。

## 验证

- `cargo fmt --check`、`cargo clippy --release -- -D warnings`、`cargo test`：197/197
- `scripts/smoke.ps1 -SkipBuild`：10/10
- 组合框修复：修复前逐帧抓图可复现「点击后先空白」，修复后无空白帧；带标签数据时弹层正常显示两项
- 托盘、标签批量、面板、跨进程注入、专属快捷键、host-matrix、perf 未在本补丁重复执行；0.1.4 已验证，本版只改两条绘制/刷新路径

## 已知限制

- 未做代码签名，Windows SmartScreen 可能提示“未知发布者”。
- 高对比度与混合 DPI 场景仍需真实桌面人工验收。
- 超大库导入仍有数倍文件大小的内存峰值；>10k 短语的极限库需拆分。

## 升级

- 替换 EXE 不会改动数据；便携模式继续使用 `<exe>\data`。
- 从 0.1.4 升级可直接覆盖 `QuickPhrase.exe`，`settings.json` 与短语库保持原样。