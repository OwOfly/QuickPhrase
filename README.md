# QuickPhrase 快语

Windows 桌面全局快捷短语输入器。当前进度：**M1-7 注入加固**。

完整技术方案见 `docs/_ref/final-plan.md`（v4.1），需求见 `docs/_ref/prd-v1.1.md`。

## 当前已实现

- Rust + Win32，单进程、单实例（命名互斥 + 窗口消息唤起）
- 托盘图标与右键菜单：打开面板 / 管理短语 / 暂停或恢复快捷键 / 设置 / 恢复上次剪贴板内容 / 关于 / 退出；双击托盘打开管理窗口
- 全局热键 `Ctrl + Alt + Q`，注册失败有明确提示；暂停/恢复可用
- 退出时释放热键与托盘图标
- 热键触发时同步快照：前台窗口、PID、进程创建时间、焦点控件、完整性级别
- UI Automation 捕获：常驻 MTA 线程、60 ms 预算、零 HWND 上溯、`CompareElements`、只读/密码/插入点判定
- 区域分类：终端 / 编辑器 / 未验证；身份与“可输入能力”分开报告
- 前台切换监听（`EVENT_SYSTEM_FOREGROUND`，面板生命周期内）
- 快捷面板：520×420、原生 `EDIT` 搜索、OwnerDraw 列表、↑↓/Home/End、Enter 提交、Esc/点击外部关闭
- 面板按光标定位并夹紧到显示器工作区；DWM 圆角；Per-Monitor DPI 适配
- 文本注入：恢复目标、G1–G12 守卫、`SendInput` Unicode（CR 换行）、三态结果
- 输入策略：单行 Unicode；多行仅限已验证编辑器；终端/未知区域多行、超长、控制字符一律只复制到剪贴板
- Per-Monitor V2 DPI manifest（已嵌入 EXE）与 Common-Controls v6 依赖
- 本地滚动日志：`%LocalAppData%\QuickPhrase\logs\qp-YYYY-MM-DD.log`

- 本地 JSON 存储：schema、原子写、5 份备份、恢复、只读模式、revision 高水位
- 检索：D16 打分与排序、前 50、命中区间，1,000 条 P95 < 1 ms
- 面板完整态：分类栏、空库/无匹配/加载失败空态、只读提示、捕获失败仅复制、关键词高亮
- 管理窗口：三栏、CRUD、分类增删、回收站与恢复、校验、未保存拦截
- 设置窗口：热键录制与热回滚、Tab 策略、主题偏好、开机启动、剪贴板恢复
- 托盘：暂停/恢复、第二实例唤起面板、退出释放资源
- 注入加固：粘贴兼容模式（警示 + Ctrl+V）、剪贴板序列保护、长文本、UIA 连续超时降级、区域策略

**尚未实现**：深色主题渲染与动画（M1-8）、导入/导出、托盘气泡失败降级、暂停状态持久化、回收站 30 天清理。

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

- 托盘出现图标后，按 `Ctrl + Alt + Q` 打开快捷面板
- 重复启动不会产生第二个进程，会唤起已有实例
- 托盘菜单「暂停快捷键」可注销热键，再次点击恢复

## 测试

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\smoke.ps1
```

覆盖：冷启动、首实例存活、第二实例退出、托盘回调、优雅关闭、热键释放、日志断言。

捕获链路：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\capture-smoke.ps1
```

手工诊断：先把焦点放到目标窗口，再运行

```powershell
.\target\release\quickphrase.exe --capture-test --delay 2000
```

`--delay` 毫秒后抓取当前前台，结果写入日志，并落盘到
`%LocalAppData%\QuickPhrase\captures\capture-<ts>.json`，包含 UIA 元素属性、只读判定、插入点、耗时与前台事件序列。

面板：

```powershell
# 100 次显示/隐藏基准
.\target\release\quickphrase.exe --panel-bench

# 手动打开面板（Esc 退出）
.\target\release\quickphrase.exe --panel-test

# 自动化冒烟
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\panel-smoke.ps1

# 注入自测（自建 EDIT，走完整守卫与 SendInput）
.\target\release\quickphrase.exe --inject-test
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\inject-smoke.ps1
```

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
  app.rs           隐藏主窗口、消息循环、托盘菜单
  context.rs       原生窗口/进程/完整性/焦点快照
  uia.rs           UI Automation 常驻线程、捕获、身份比对
  capture.rs       捕获编排、能力与区域判定、JSON 报告
  panel.rs         快捷面板、搜索框、OwnerDraw 列表、定位与 DPI
  inject.rs        输入策略、守卫、SendInput、三态结果、注入自测
  clipboard.rs     纯文本剪贴板写入（只复制路径）
  foreground.rs    前台切换监听
  hotkey.rs        RegisterHotKey 封装
  single_instance.rs  命名互斥 + 第二实例唤起
  tray.rs          Shell_NotifyIcon 封装
  logging.rs       本地滚动日志
  platform.rs      DPI、消息框、数据目录
scripts/smoke.ps1          M0-1 冒烟测试
scripts/capture-smoke.ps1  M0-2 捕获冒烟测试
scripts/panel-smoke.ps1    M0-3 面板冒烟测试
scripts/inject-smoke.ps1   M0-4 注入自测
docs/_ref/         方案、PRD、评审记录
```

## 已知限制（M0-3 阶段）

- 托盘使用系统默认应用图标，正式图标在 M1-9 打包阶段替换。
- 热键目前是写死的 `Ctrl+Alt+Q`，设置界面在 M1-5。
- 日志只记元数据，不记短语正文；捕获 JSON 会含 UIA 元素名称，仅本机诊断使用。
- 焦点切换类反例（A→B→A、UIA 挂起）尚未在真实交互桌面验证，M0-5 补。
- 面板列表是 3 条硬编码数据；存储与检索在 M1。
- 本机桌面受 UIPI 限制，脚本用 `WM_HOTKEY` 消息驱动面板；真实热键已在手工测试中验证。
- 跨应用注入矩阵尚未执行；M0-5 覆盖 Chrome、VS Code、Word、微信、终端等。
- 剪贴板兼容输入模式与序列号保护在 M1-7。
