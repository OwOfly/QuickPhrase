# QuickPhrase 快语 v0.1.4 发布说明

- 版本：`0.1.4`（`0.1.3` 的性能、数据安全与可访问性更新）
- 平台：Windows 10 21H2+ / Windows 11 x64
- 资产：`QuickPhrase-0.1.4-win-x64.zip`、`QuickPhrase-0.1.4-win-x64.exe`、`SHA256SUMS.txt`

## 主要变化

### 性能

- 管理窗口搜索不再深克隆整库、不再对全部短语重算高亮；面板预先计算搜索键并按需缓存。
- usage 统计不再重建整库搜索 blob，改为单后台 FIFO 写盘；退出、关机与面板测试前 flush。
- Direct2D 按尺寸复用内存 DC/DIB（32 个 LRU）并复用画刷，减少每帧资源创建。

### 注入与退出安全

- 注入先校验目标（窗口/进程/线程身份）再激活，回收的 HWND 不会被抢先激活。
- 托盘退出、`WM_CLOSE`、`WM_QUERYENDSESSION` 统一经过退出/会话门禁：确认卡打开到决议期间阻断新注入，会话结束待决期间保持 fail-closed。
- 接受放弃草稿后清理 dirty 状态；面板恢复前台前校验目标进程身份。
- UIA 连续两次超时/断链后隔离 60 秒，到期允许一次探测；成功恢复，失败继续隔离；worker 死亡后下个请求自动重建。

### 数据安全

- 整库替换/恢复改为有界读取（640 MiB、100k 短语/10k 分类），解析前先做零分配计数探针。
- “替换全部数据”在原生确认后再弹危险确认；取消不改数据，成功后才刷新编辑器/热键，现有数据先备份。
- 替换后失效分类视图回落“全部短语”；导入/恢复错误文案统一。
- 确认卡支持多行消息，80 字删除标题不再被裁；`OpenClipboard` 增加 3×30 ms 有限重试。

### 键盘、DPI 与高对比度

- 面板搜索框 `Ctrl+←/→` 循环分类筛选；纯 Ctrl 才触发，不影响 Ctrl+Shift 选词与 IME。
- 下拉选择器与确认卡处理 `WM_DPICHANGED`，按目标显示器工作区 clamp；拖拽跨 DPI 不跳变。
- 检测 Windows 高对比度并使用系统色配对；`WM_SETTINGCHANGE`/`WM_SYSCOLORCHANGE`/`WM_THEMECHANGED` 即时刷新管理、设置、面板与关于窗口。

### 测试与发布

- `cargo test` 197/197；smoke 不再把未观测到的热键记为 PASS。
- 面板冒烟按 DPI 断言尺寸并覆盖搜索/清空；`WM_SETTEXT`/列表计数使用有界消息。
- `perf-gate.ps1` 覆盖 10k×1k 搜索与面板基准；报告记录 EXE 哈希/版本/commit/CPU/OS。
- `package.ps1` 绑定已测 EXE 哈希、校验 zip 内字节、release notes 与 manifest 版本；默认拒绝脏树/未打 tag。
- 新增 Windows CI（fmt/clippy/test/package）。

## 已知限制

- `panel-smoke`、`inject-smoke`、`host-matrix`、`snippet-hotkey-smoke`、`tray-smoke` 需要交互式桌面，发布前在隔离桌面复跑。
- 高对比度与混合 DPI 需要真实桌面截图/人工验收。
- 自定义高对比度方案若分离 `BTNFACE`/`WINDOWTEXT`，个别强调色图标可能对比不足。
- 超大库导入仍有数倍文件大小的内存峰值；>10k 短语的极限库需拆分。
- 同进程同线程 HWND 复用仍可能被激活（文本输入仍受焦点/选区复核保护）；UIA provider 暂缓。
- 3k 行文件拆分、主题/文本布局缓存、UIA provider 为后续独立工作。

## 升级

替换 EXE 不会改动数据；便携模式继续使用 `<exe>\data`。升级前可保留一份导出 JSON 或备份目录。