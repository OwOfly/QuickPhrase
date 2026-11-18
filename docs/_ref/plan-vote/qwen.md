# 议员方案：qwen4-max#max

## 1 技术栈
**主选：Rust + `windows` crate，UI 混合**——输入控件用原生 EDIT/RICHEDIT（IME 免费），面板列表/视觉用 Direct2D+DirectWrite 自绘。release 配 `opt-level="z", lto="fat", codegen-units=1, panic="abort", strip` → EXE 约 2–4 MB；空闲工作集 10–20 MB（D2D 设备懒创建）；冷启动注册热键 <300 ms；面板预创建隐藏窗口+预热字体，热键→可见 30–80 ms；1000 条子串检索 <10 ms。硬约束全达标且留余量。
**备选：C++ Win32（MSVC）同架构。** 取舍：Rust 内存安全+`serde_json`+cargo 迭代快；C++ Win32 资料最熟、CRT 静态略小，但人日 +30–50%、手动安全。决策门：M0 若 `windows` crate D2D 互操作受阻 >2 人日，切 C++。
排除 C#：WPF 自包含单文件 ~70–100 MB、空闲 60–120 MB 双超；WinForms 裁剪后仍 ~25–40 MB，NativeAOT WinForms 未生产就绪。排除 egui（eframe+wgpu 工作集 80–150 MB 超 35 MB）、Slint（+3–6 MB、30–60 MB，中文 IME 组合不成熟威胁 UI-04）。
IME：搜索框=原生 EDIT，跟踪 `WM_IME_STARTCOMPOSITION/ENDCOMPOSITION`，组合态内 `VK_RETURN` 不触发选择。DPI：`SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`+`WM_DPICHANGED`+`GetDpiForWindow`；多屏 `MonitorFromWindow`+`GetMonitorInfo` 夹紧工作区。

## 2 注入设计
`SendInput`+`KEYEVENTF_UNICODE`（VK_PACKET 0xE7），每 UTF-16 码元一对 down/up。
- **换行**：默认 `\n`→`\r`(0x0D)，`\r\n`→单 `\r` 防双换行。终端宿主（WindowsTerminal/conhost/mintty/PuTTY）`\r`=执行 → 含换行即拒注转复制。聊天宿主（WeChat/QQ/DingTalk/Slack 进程名黑名单）`\r`=发送 → 多行拒注转粘贴。
- **Emoji**：非 BMP 拆代理对高/低各一发；Win10+ EDIT/Chrome/VSCode 可重组，旧 Qt 可能丢 → 按应用降级粘贴。
- **Tab(0x09)**：可能被 `IsDialogMessage` 当焦点导航吞；实测丢失则建议粘贴。
- **长文本**：>2000 字提示推荐粘贴；仍走 Unicode 则 ≤50 字/批（100 INPUT）、批间 2 ms、每批查 `GetForegroundWindow`，变即中止+报已注入字数+复制剩余。10k 字约 0.5–1.5 s。
- **注入期用户按键**：每批查前台+`GetAsyncKeyState` 修饰键；注入前等 Ctrl/Alt/Shift 释放 ≤500 ms，超时 FAILED_SAFE。
- **管理员目标**：`SendInput` 对提权窗口静默丢弃（返回值仍成功！）→ 注入前 `OpenProcess`+`OpenProcessToken`+`GetTokenInformation(TokenElevation)`，目标提权且自身非提权 → 拒注。
- **粘贴模式**：`OpenClipboard` 重试 5×50 ms；仅存 CF_UNICODETEXT；记 `GetClipboardSequenceNumber`；Ctrl+V 后 300 ms 恢复，序列号被改则跳过+告警；不承诺其他格式。
- `SendInput` 返回数 < 期望 → FAILED_SAFE。

## 3 焦点校验
触发即快照（弹窗前）：`GetForegroundWindow`→hwnd；`GetWindowThreadProcessId`→pid/tid；`GetGUIThreadInfo(tid)`→hwndFocus/hwndCaret/rcCaret；`QueryFullProcessImageName`→进程名；`GetProcessTimes`→创建时间（防 hwnd 复用）；`ClientToScreen` 光标→定位；`QueryPerformanceCounter` 时间戳。
恢复序列：隐藏面板→`SetForegroundWindow(hwnd)`，失败则 `AttachThreadInput`+`BringWindowToTop` 兜底并立即解绑→10 ms 轮询 `GetForegroundWindow==hwnd` ≤300 ms→校验 pid+创建时间→若快照 hwndFocus 非空，`GetGUIThreadInfo` 比对，不符则 `AttachThreadInput`+`SetFocus` 重试一次。
**失败判据**（任一→FAILED_SAFE，不注入，提示+复制）：`IsWindow` 假；pid/创建时间不符；300 ms 前台未回；已知焦点比对失败；目标提权。浏览器/Electron hwndFocus 常 NULL → 仅校验顶层+进程（应用自恢复内部焦点）。

## 4 风险定级
- **UIPI/管理员**：可解=检测+拒注；注入本身不可解（不提权）→声明不支持。
- **杀软误报**：部分可解=代码签名（EV ~$300–600/年）+免壳+向 Defender/厂商提交误报，预算 1–2 人日；无签名 Rust+SendInput 易被启发式标记，SmartScreen 信誉需时间。
- **浏览器/Electron contenteditable**：大体可解（顶层恢复+按应用粘贴 FR-14），残留按应用 bug。
- **IME 组合态**：面板侧可解（原生 EDIT）；触发侧跨进程组合不可检测（禁钩子）→声明限制+矩阵覆盖。
- **游戏/受保护/UAC 安全桌面/锁屏**：不可解→声明不支持，不绕过。
- **密码框**：经典 EDIT 查 `GWL_STYLE & ES_PASSWORD` 拒注；浏览器密码框跨进程不可识别→声明+提示勿存敏感文本。

## 5 数据模型
`store.json`={schemaVersion:1, categories[], phrases[]}；`settings.json` 分离。
Phrase：id UUIDv4、title 1–80 字符、content 1–10000 字符（按 char 非字节）、categoryId Option、pinned bool、sortOrder i32、createdAt/updatedAt ISO8601 UTC、useCount u32、lastUsedAt Option、deletedAt Option（软删）。
回收站：MVP 软删+10 s 撤销 toast，回收站 UI 放 M2。排序 pinned↓→lastUsedAt↓(NULL 末)→updatedAt↓；检索评分标题前缀 150/标题含 100/正文含 50。
导入：按 id 匹配，冲突预览选跳过/另存副本（新 UUID+标题后缀），绝不覆盖；校验文件 ≤10 MB、版本、字段、长度。
迁移：schemaVersion 整数，vN→vN+1 纯函数链；高于支持版本拒载+备份。原子写：写 tmp→`sync_all`→重解析校验→`ReplaceFileW`（或 `MoveFileExW` REPLACE_EXISTING|WRITE_THROUGH）；替换前滚备份留 5 份；损坏按新到旧试备份。

## 6 模块与人日
**M0≈7 人日（6–8）**：hotkey 注册+1409 冲突 0.5；context 快照/恢复/校验 2；inserter Unicode+粘贴+提权预检+分批 2；最小面板 1；矩阵执行 1.5。门：矩阵不过→停。
**M1≈20 人日（18–22）**：snippet_store 3；popup（D2D 列表/搜索/导航/IME 守卫/定位/DPI）4；admin 三区+校验+未保存守卫 4；tray_settings（`Shell_NotifyIcon`/`CreateMutex` 单实例/HKCU Run 自启）2；inserter 加固（按应用模式/序列号守卫/中止）2；search+基准 1；集成+12 验收+性能基线 3；打包签名 README 1。**合计 ≈27 人日。**

## 7 M0 测试矩阵
8 宿主：Win10 记事本、Win11 记事本、Chrome(input/textarea/contenteditable)、VS Code、WeChat、WindowsTerminal、cmd/conhost、提权记事本。×8 场景：中文插入、英文符号、多行换行、Emoji、选区替换、IME 组合态触发、切换窗口中止、长文本 2k/10k = **64 格**。
**阻断红线**（任一违反=发布阻断）：错误窗口写入、聊天误发送、终端误执行、管理员静默失败、Esc 改动内容。通过门：受支持格 ≥63/64，红线 0。

## 8 最可能翻车的 3 个假设
① **"Unicode 默认全兼容"**：聊天/终端换行语义、部分 Electron/Qt 丢 VK_PACKET、旧 EDIT 代理对 → 无按应用兜底实测成功率恐 80–90% 而非 99%。
② **"焦点校验=正确输入位置"**：浏览器/Electron hwndFocus 恒 NULL，顶层校验只证窗口不证输入元素；hwnd 复用、`AttachThreadInput` 副作用、300 ms 超时慢机偏紧 → 残留"对窗口错字段"。
③ **"兼容模式剪贴板可恢复"**：延迟渲染+序列号竞态使无损恢复不可能，PRD 已免责但用户预期落空。

## 我不同意 PRD 的地方
1. **9.1"原生控件起步" vs 14.2 Fluent 毛玻璃**：原生控件做不出 Fluent；主张 D2D 混合+放弃真亚克力模糊（纯色+轻阴影）保 ≤35 MB（14.7 已允许，应明说）。
2. **默认 Unicode 含多行聊天/终端**：安全红线应自动按宿主拒注多行并转粘贴/复制，而非等用户手动切（PRD 6.4 手动）。
3. **10k 字默认 Unicode**：>2000 字应默认提示粘贴（仍可选），降中止/丢字风险。
4. **≥99% 成功率分母未定义**：应明确=声明支持的矩阵格，排除声明不支持项。
