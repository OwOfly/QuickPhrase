# M1-5 设置验收记录（2026-11-19）

- 范围：热键录制与应用、Tab 策略、主题偏好、开机启动、数据目录展示、剪贴板恢复
- 依据：`docs/_ref/prd-v1.1.md` §14.5、`docs/_ref/final-plan.md` §8 M1-5
- 复核：待独立复核

## 已实现

| 项 | 实现 |
| --- | --- |
| 窗口 | `QuickPhrase.SettingsWindow`，560×430，Per-Monitor DPI |
| 热键 | 显示当前组合；「录制」后捕获下一组修饰键 + 键；保存时在主窗口重新注册，失败回滚并提示 |
| 热键解析 | `Ctrl/Alt/Shift/Win` + `A–Z/0–9/F1–F24`，大小写不敏感 |
| Tab 策略 | `spaces4` / `tab`，落盘 `settings.json` |
| 主题 | `system` / `light` / `dark` 偏好落盘（深色渲染属 M1-8） |
| 开机启动 | 写/删 `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\QuickPhrase` |
| 数据目录 | 只读展示 `%LocalAppData%\QuickPhrase` |
| 剪贴板 | 「恢复上次复制的剪贴板内容」调用内存快照 |
| 持久化 | `storage::save_settings`，写入失败回滚内存 |
| 应用接线 | 启动、暂停恢复、设置保存都从 `settings.hotkey` 注册 |

## 实测

- `--settings-test` 窗口创建/关闭退出码 0。
- 真实应用打开设置并保存：`settings.json` 变为 `tabPolicy=tab`、`theme=dark`，热键仍 `Ctrl+Alt+Q`。
- `cargo test` 99/99（新增热键解析 3 个）。

## 已知偏差 / 未覆盖

- 「兼容粘贴」输入方式未暴露，属 M1-7（避免出现不可用选项）。
- 主题仅存偏好，深色渲染属 M1-8。
- 数据目录不可修改，只展示；迁移属 M1-9/后续。
- 剪贴板快照仅本次进程内存，重启后不可恢复。
- 热键录制未自动化（需要真实修饰键状态），仅代码路径与解析单测。
- 开机启动在受限环境可能失败，失败时提示「已保存；开机启动未生效」。

## 结论

M1-5 的设置窗口、热键、Tab、启动与剪贴板恢复已接入；待复核后进入 M1-6 托盘。