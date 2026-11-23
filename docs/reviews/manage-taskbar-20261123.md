# 管理窗口任务栏图标修复（2026-11-23）

- 类型：发布后 UX 缺陷修复（v0.1.1 → v0.1.2）
- 提交：`3455a7c`（WS_EX_APPWINDOW）及本次发布提交（托盘重开恢复分支）
- 范围：`src/manage.rs`
- 发现：用户点“短语管理”窗口最小化后，窗口从桌面消失、任务栏没有图标，无法恢复
- 结论：glm / kimi / qwen 复核 PASS；glm 的“最小化→关闭→重开”问题已修并 focused PASS

## 根因

管理窗口创建时 owner 是隐藏的主消息窗口（`Some(owner)`）。Windows 对 owned top-level 窗口默认不提供任务栏按钮，因此最小化后没有 Shell 入口，窗口无法从任务栏恢复。

## 修复

| 位置 | 修改 |
| --- | --- |
| `manage::create` | `CreateWindowExW` 的 ex-style 改为 `WS_EX_APPWINDOW`，owned 窗口也获得任务栏按钮 / Alt+Tab 项 |
| `manage::show` | 托盘重开时若 `IsIconic(hwnd)` 则 `ShowWindow(SW_RESTORE)`，否则维持 `SW_SHOW`；覆盖“最小化→WM_CLOSE 隐藏→重开”序列 |

`WS_EX_APPWINDOW` 只改变 Shell 枚举，不改 owner、class、title、Z 序或消息处理；窗口隐藏（WM_CLOSE → SW_HIDE）时任务栏按钮随之移除。

## 验证

- `GetWindowLongPtr(GWL_EXSTYLE)` = `0x40100`（APPWINDOW + WINDOWEDGE）
- `ShowWindow(SW_MINIMIZE)` 后 `IsIconic=true`，任务栏出现 QuickPhrase 图标
- `ShowWindow(SW_RESTORE)` 后 `IsIconic=false`
- 任务栏截图：`dist/ui-shots/taskbar-manage-minimized-012.png`
- `cargo fmt --check` / `cargo clippy --release` PASS，`cargo test` 150/150
- panel-smoke 10/10、tag-batch-smoke 10/10（本构建；panel-smoke 在 Chrome 前台时曾出现 foreground-restore flake，重跑通过）
- 复核：glm/kimi/qwen 初轮 PASS；glm 的 Q3（iconic 状态下 WM_CLOSE 后重开）以 focused 复验 PASS
- Q1/Q2：tag-batch-smoke 覆盖管理窗口，按 class/title 定位，不依赖 ex-style 精确值；设置窗口与关于窗口本来就是 unowned（有任务栏按钮），快捷面板保持 `WS_EX_TOOLWINDOW` 的临时弹窗定位，不需要同类处理

## 未覆盖 / 后续

- 任务栏按钮在管理窗口可见期间常驻，这是 `WS_EX_APPWINDOW` 的预期行为，不是只在最小化时出现
- 未做任务栏图标在 125/150/200% DPI 下的逐档截图；图标由 class icon 提供，尺寸由 Shell 缩放
- `Ctrl+Alt+Q` 快捷面板仍是临时弹窗，不进任务栏（设计如此）

## 结论

三家复核 PASS，任务栏最小化/恢复与重开分支闭环，进入 v0.1.2 发布包。