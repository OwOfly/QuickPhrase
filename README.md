# QuickPhrase 快语

Windows 桌面全局快捷短语输入器。当前进度：**M0-1 骨架**。

完整技术方案见 `docs/_ref/final-plan.md`（v4.1），需求见 `docs/_ref/prd-v1.1.md`。

## 当前已实现

- Rust + Win32，单进程、单实例（命名互斥 + 窗口消息唤起）
- 托盘图标与右键菜单：打开面板（占位）/ 管理短语（占位）/ 暂停快捷键 / 设置（占位）/ 关于 / 退出
- 全局热键 `Ctrl + Alt + Q`，注册失败有明确提示；暂停/恢复可用
- 退出时释放热键与托盘图标
- Per-Monitor V2 DPI manifest（已嵌入 EXE）
- 本地滚动日志：`%LocalAppData%\QuickPhrase\logs\qp-YYYY-MM-DD.log`

**尚未实现**：快捷面板、短语存储、文本注入、设置、管理窗口。热键目前只弹占位提示。

## 环境

- Windows 10 21H2+ / Windows 11 x64
- Rust nightly MSVC（当前 `nightly-2025-12-04`）
- Windows SDK（`rc.exe`，build.rs 会自动在 `Windows Kits\10\bin\*\x64` 下查找）

## 构建

```powershell
cargo build --release
```

产物：`target\release\quickphrase.exe`（当前约 155 KB）。

## 运行

```powershell
.\target\release\quickphrase.exe
```

- 托盘出现图标后，按 `Ctrl + Alt + Q` 触发占位提示
- 重复启动不会产生第二个进程，会唤起已有实例
- 托盘菜单「暂停快捷键」可注销热键，再次点击恢复

## 测试

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke.ps1
```

覆盖：冷启动、首实例存活、第二实例退出、托盘回调、优雅关闭、热键释放、日志断言。

单独测启动耗时（不含 PowerShell 调度开销）：

```powershell
$psi = [Diagnostics.ProcessStartInfo]::new()
$psi.FileName = ".\target\release\quickphrase.exe"
$psi.Arguments = "--bench"
$psi.UseShellExecute = $false
$sw = [Diagnostics.Stopwatch]::StartNew()
[Diagnostics.Process]::Start($psi).WaitForExit()
$sw.ElapsedMilliseconds
```

## 目录

```
src/
  main.rs          入口、CLI 参数
  app.rs           隐藏主窗口、消息循环、托盘菜单、状态机占位
  hotkey.rs        RegisterHotKey 封装
  single_instance.rs  命名互斥 + 第二实例唤起
  tray.rs          Shell_NotifyIcon 封装
  logging.rs       本地滚动日志
  platform.rs      DPI、消息框、数据目录
scripts/smoke.ps1  M0-1 冒烟测试
docs/_ref/         方案、PRD、评审记录
```

## 已知限制（M0-1 阶段）

- 托盘使用系统默认应用图标，正式图标在 M1-9 打包阶段替换。
- 热键目前是写死的 `Ctrl+Alt+Q`，设置界面在 M1-5。
- 日志只记元数据，不记短语正文。
