# QuickPhrase 快语

Windows 桌面全局快捷短语输入器。当前版本：**0.1.0（V1）**。

完整技术方案见 `docs/_ref/final-plan.md`（v4.1），需求见 `docs/_ref/prd-v1.1.md`。

## 当前已实现

- Rust + Win32，单进程、单实例（命名互斥 + 窗口消息唤起）
- 托盘图标与右键菜单：打开面板 / 管理短语 / 新增短语 / 暂停或恢复快捷键 / 设置 / 恢复上次剪贴板内容 / 关于 / 退出；双击托盘打开管理窗口
- 全局热键 `Ctrl + Alt + Q`，注册失败有明确提示；暂停/恢复可用
- 退出时释放热键与托盘图标
- 热键触发时同步快照：前台窗口、PID、进程创建时间、焦点控件、完整性级别
- UI Automation 捕获：常驻 MTA 线程、60 ms 预算、零 HWND 上溯、`CompareElements`、只读/密码/插入点判定
- 区域分类：终端 / 编辑器 / 未验证；身份与“可输入能力”分开报告
- 前台切换监听（`EVENT_SYSTEM_FOREGROUND`，面板生命周期内）
- 快捷面板：520×420、Direct2D/DirectWrite 自绘、原生 `EDIT` 搜索、OwnerDraw 列表、↑↓/Home/End、Enter 提交、Esc/点击外部关闭
- 面板按光标定位并夹紧到显示器工作区；DWM 圆角与深色标题栏；Per-Monitor DPI 适配
- 文本注入：恢复目标、G1–G12 守卫、`SendInput` Unicode（CR 换行）、三态结果
- 输入策略：单行 Unicode；多行仅限已验证编辑器；终端/未知区域多行、超长、控制字符一律只复制到剪贴板
- Per-Monitor V2 DPI manifest（已嵌入 EXE）与 Common-Controls v6 依赖
- 本地滚动日志：`%LocalAppData%\QuickPhrase\logs\qp-YYYY-MM-DD.log`

- 本地 JSON 存储：schema、原子写、5 份备份、恢复、只读模式、revision 高水位
- 检索：D16 打分与排序、前 50、命中区间，支持标题拼音全拼与首字母（如 `tk` → 退款说明），1,000 条 P95 < 1 ms
- 变量模板：`{date}`、`{time}`、`{datetime}` 在面板预览中展开，插入时按当前本地时间渲染；`{{`/`}}` 转义
- 标签：管理窗口正文下方逗号分隔编辑，最多 10 个、单个 ≤24 字；标签进入检索 blob，可与标题/正文/拼音一起搜索
- 标签筛选：管理窗口“标签”下拉筛选；面板搜索框 `#标签` 前缀筛选，可追加关键词（如 `#support 退款`）
- 批量标签：管理列表 Ctrl/Shift 多选后批量添加或移除标签，单次原子写入
- 短语专属快捷键：管理窗口“快捷键”字段（留空不设置），保存时校验格式、重复与全局热键冲突；命中后直接捕获并注入，目标不确定时仍只复制，不自动重试
- 面板完整态：分类栏、空库/无匹配/加载失败空态、只读提示、捕获失败仅复制、关键词高亮
- 管理窗口：三栏 Fluent 布局、CRUD、分类增删、回收站与恢复、校验、未保存拦截、自定义平滑滚动条
- 设置窗口：740×560（最小 500×540）、热键录制与热回滚、Tab 策略、主题偏好、开机启动、剪贴板恢复、数据目录
- 数据闭环：JSON 导出、导入（合并/替换）、最近 5 份备份恢复；回收站 30 天自动清理
- 便携模式：可执行文件旁放 `portable.txt`（或 `portable`）文件，数据/备份/日志/截取改写到 `<exe>\data`
- 暂停状态持久化：暂停后退出，下次启动仍保持暂停，可从托盘恢复
- 面板排序：设置 → 外观与主题切换“最近使用 / 最常使用”，置顶始终优先
- 关于窗口与 Direct2D 自绘托盘菜单；托盘：新增短语、暂停/恢复、第二实例唤起面板、退出释放资源
- 注入加固：粘贴兼容模式（警示 + Ctrl+V）、剪贴板序列保护、长文本、UIA 连续超时降级、区域策略
- 应用输入例外：设置 → 应用输入例外，按可执行文件名覆盖 Unicode/粘贴模式，未命中使用全局模式
- UI Spec：统一主题令牌与浅/深/跟随系统、面板搜索图标/快捷键提示/设置与管理入口、类型图标、键盘焦点边框、淡入、管理窗口分类计数与校验红框；Direct2D/DirectWrite 矢量图标与文字
- 键盘：面板与列表全键盘操作；管理/设置使用 `IsDialogMessage` 导航原生控件；设置自绘卡片提供 Tab/方向键/空格/Enter 焦点

**尚未实现 / 已知限制**：托盘气泡失败降级、代码签名；真实跨应用输入矩阵与 125/150/200% 视觉矩阵待交互桌面验证。

## 环境

- Windows 10 21H2+ / Windows 11 x64
- Rust nightly MSVC（当前 `nightly-2025-12-04`）
- Windows SDK（`rc.exe`，build.rs 会自动在 `Windows Kits\10\bin\*\x64` 下查找）

## 构建

```powershell
cargo build --release
```

产物：`target\release\quickphrase.exe`（当前约 1 MB，含图标、版本资源、Direct2D 渲染与拼音数据）。

## 打包

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\package.ps1
```

产物在 `dist\`：

- `QuickPhrase-<version>-win-x64.zip`（EXE + README + LICENSE）
- `SHA256SUMS.txt`

脚本会校验 zip ≤15 MB，并打印版本资源与打标签命令（`git tag v<version>`）。
图标由 `scripts\make-icon.ps1` 生成，不需要外部素材。

## 安装 / 卸载

- 安装：解压 zip 到任意目录，运行 `QuickPhrase.exe`。无需管理员权限。
- 开机启动：托盘「设置」→ 勾选「开机启动」；卸载前取消勾选或删除
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\QuickPhrase`。
- 卸载：退出托盘程序，删除解压目录；如需清除数据，删除
  `%LocalAppData%\QuickPhrase`（含短语、设置、备份与日志）。
- 版本：EXE 内含文件版本 0.1.0、产品名与图标。

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
  uia.rs           UI Automation 常驻线程、捕获、身份比对、降级
  capture.rs       捕获编排、能力与区域判定、JSON 报告
  panel.rs         快捷面板、搜索、分类、高亮、定位与 DPI
  inject.rs        输入策略、守卫、SendInput/粘贴、三态结果、注入自测
  clipboard.rs     剪贴板快照、序列保护、恢复
  storage.rs       数据/设置/规则持久化、CRUD、备份与恢复
  search.rs        D16 打分、排序与命中区间
  manage.rs        三栏管理窗口、CRUD、回收站、校验
  settings_ui.rs   设置窗口、热键录制、启动项、剪贴板恢复
  theme.rs         浅/深/跟随系统主题令牌与对比度校验
  foreground.rs    前台切换监听
  hotkey.rs        RegisterHotKey 封装
  single_instance.rs  命名互斥 + 第二实例唤起
  tray.rs          Shell_NotifyIcon 封装
  logging.rs       本地滚动日志
  platform.rs      DPI、消息框、数据目录、启动项
scripts/smoke.ps1          M0-1 冒烟测试
scripts/capture-smoke.ps1  M0-2 捕获冒烟测试
scripts/panel-smoke.ps1    M0-3 面板冒烟测试
scripts/inject-smoke.ps1   M0-4 注入自测
scripts/tray-smoke.ps1     M1-6 托盘冒烟
scripts/make-icon.ps1      生成 assets/quickphrase.ico
scripts/package.ps1        打包 zip 与 SHA256
docs/_ref/         方案、PRD
docs/reviews/      各里程碑验收与复核记录
```

## 已知限制（V1）

- 导入导出目前仅支持 QuickPhrase JSON；应用例外按 exe 文件名匹配，不支持通配符或窗口标题。
- 托盘气泡失败无降级提示。
- 代码签名未做，Windows 可能显示未知发布者。
- 真实跨应用输入矩阵（Chrome/Word/VS Code/WeChat/终端）与 125/150/200% 视觉矩阵待交互桌面验证。
- 浏览器多行、终端、未知区域固定只复制；粘贴模式也受同样区域限制。
- 日志只记元数据，不记短语正文；捕获 JSON 含 UIA 元素名称，仅本机诊断使用。
