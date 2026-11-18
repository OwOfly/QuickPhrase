# 议员方案：glm-5.3#max

## 1. 技术栈

**主选：Rust 1.83+ + `windows` crate（≥0.58）+ Slint 1.x**（桌面免版税许可，可商用）。Slint 编译配置 `default-features=false, features=["renderer-software","backend-winit","accessibility"]`，release 用 `opt-level="z"`、`LTO="fat"`、`panic="abort"`、`strip=true`。预计 EXE 3–5 MB、冷启动 <500 ms、空闲 private working set 25–40 MB。M0 门禁：Task Manager 私工作集 ≤35 MB、面板 P95 ≤150 ms，不过门禁则 UI 降级 Direct2D/DirectWrite 自绘（+4 人日）。舍 egui：需 wgpu/softbuffer，常驻 40 MB+ 超标，IME preedit 定位弱。舍 C++ Win32：内存最优但 UI/IME/高 DPI 工期 ×2。
**备选：C# .NET 9 WinForms + NativeAOT**。工期省约 30%（M0+M1 约 21 人日 vs 31.5），WinForms 的 IME/DPI 成熟；代价：EXE 12–18 MB、空闲 45–70 MB，必超 35 MB 红线——选它等于接受内存目标改为 ≤70 MB，其余指标不变。取舍：保指标选 Rust，保交付速度且愿意放宽内存选 C#。

## 2. 注入设计

- 换行统一发 **CR(0x0D)**：Edit/RichEdit（记事本）CR→CRLF；Chromium/Electron（Chrome/Edge/VS Code）归一为 \n。M0 失败判据：某宿主 CR 不换行或显示 □ → 该宿主进 per-app 表改发 LF。
- Emoji：按 UTF-16 code unit 逐个 `SendInput`，高代理在前（😂=D83D+DE02）。宿主显示两半字符 → 该宿主仅粘贴模式。
- Tab(0x09) 逐位发；Chromium textarea 若焦点被 tab 导航抢走 → 该宿主改粘贴。
- 长文本：每批 ≤256 个 INPUT，批间 `Sleep(8ms)`；>2,000 units 时每批前复核 `GetForegroundWindow` 与快照一致，不一致中止剩余并提示复制。Unicode 模式硬上限 5,000 units，超出只允许粘贴模式；全程 >3 s 强制中止。
- 用户注入期间继续按键：PRD 禁钩子，只能缓解——快照时 `GetLastInputInfo` 距上次输入 <300 ms 则先 `Sleep(150ms)`，批间前台校验兜底，残余风险写入 README。
- 注入后补发 8 个 KEYUP（Ctrl/Alt/Shift/Win ×左右）清修饰态。
- **终端（Windows Terminal/conhost/mintty）直接列 Unicode 黑名单、仅粘贴模式**：CR 会被当 Enter 执行命令，风险不对称。粘贴后用 `GetClipboardSequenceNumber` 前后对比判断剪贴板被外部抢占，被抢占即放弃恢复并提示（与 PRD 6.4 一致）。

## 3. 焦点校验

触发（WM_HOTKEY）：`GetForegroundWindow`→`GetWindowThreadProcessId`→`QueryFullProcessImageNameW`（OpenProcess 仅 PROCESS_QUERY_LIMITED_INFORMATION）→`GetGUIThreadInfo(tid)` 取 hwndFocus/hwndCaret/rcCaret（ClientToScreen 后用于面板定位）→记 tick。
选择后：①隐藏面板后若前台 ≠ 面板 HWND → 用户切过窗 → FAILED_SAFE；②`SetForegroundWindow(hwndTop)`，5 ms 轮询 ≤50 ms 验前台==hwndTop，失败重试一次（合计 ≤100 ms）；③再 `GetGUIThreadInfo` 验 hwndFocus==快照，或同 image + `RealGetWindowClass` 同类；④`IsWindow`+pid 一致才注入。可编辑判据：hwndCaret ≠ NULL，或 `IUIAutomation::GetFocusedElement` 的 `UIA_IsTextPatternAvailable`（预算 30 ms，超时按未知）；两者皆无/未知 → 只读+复制。任一步失败 → FAILED_SAFE：不发字符、toast 原因、给复制按钮。

## 4. 风险定级

- **UIPI/管理员窗口**：`OpenProcessToken` 失败即判提升 → copy-only。可检测、不可注入，声明不支持（不提权，无工程解）。
- **杀软误报**：无签名 Rust+SendInput 高风险。对策：OV 代码签名（≈¥1,800/年）+ 微软误报提交 + 发布 SHA-256。残余中，无法归零。
- **Electron/contenteditable**：焦点恢复通常 `SetForegroundWindow` 即可，低-中危，M0 实证。
- **IME 组合态**：无跨进程检测 API；M0 实测首字符被吞则提示“先上屏候选词”，残余中。
- **游戏/反作弊**：检测不可靠且可能封号 → 声明不支持 + FAQ + 托盘“暂停热键”。高危，不可工程解决。
- **RDP**：M0 顺带验证，不做承诺。

## 5. 数据模型

`data.json`：`{schemaVersion:1, categories:[{id,name,sortOrder}], phrases:[{id(UUIDv4), title(1–80 units), content(1–10,000 units), categoryId, pinned, sortOrder, createdAt, updatedAt, useCount, lastUsedAt, deletedAt|null}]}`；`settings.json` 独立存热键/输入模式/暂停热键/自启/主题。
回收站 = deletedAt 非空，启动清 30 天前条目，恢复保留原 id。列表排序：pinned → lastUsedAt → updatedAt。搜索打分：标题前缀 100 > 标题子串 60 > 正文前缀 40 > 正文子串 20，同分看 lastUsedAt，仅渲染前 50 条（保 1,000 条 P95 ≤50 ms）。导入按 id 匹配，冲突逐项选跳过/覆盖/副本（新 id，title+" (2)"）；>5 MB 或 >20,000 条拒载。迁移：单步函数链；高版本拒载+提示升级+回滚最新有效备份。原子写：.tmp → `FlushFileBuffers` → `MoveFileEx(REPLACE_EXISTING)`；写前滚动备份 5 份，启动发现损坏自动恢复。

## 6. 模块与工期

**M0（8 人日）**：hotkey（RegisterHotKey + ERROR_HOTKEY_ALREADY_REGISTERED 报错）1；capture 快照 1；最小 Slint 面板（固定 3 条+键盘导航+**预建隐藏窗口**）2；恢复+注入（含降级路径）2；剪贴板兼容模式 1；矩阵+日志（qp.log 500 KB×3 滚动）1。验收 = 第 7 节矩阵 + 兼容性表。
**M1（23.5 人日）**：snippet_store 4；搜索 2；面板完整（14.3 全状态+多屏 DPI 定位）5；管理窗三区 6；托盘+设置+单实例（`CreateMutexW Local\QuickPhrase.SingleInstance` + `Shell_NotifyIconW` + HKCU Run）3；热键配置 UI 1.5；失败 UX 接线 1；打包 1。**M0+M1 合计 31.5 人日**。

## 7. M0 测试矩阵

8 宿主（记事本、VS Code、Chrome、Edge、Windows Terminal、conhost、微信 4.x、钉钉）× 6 场景（中文单行、CR 多行、Tab+缩进 SQL、Emoji、3,000 字+注入中打 10 个键、IME 组合态触发）= 48 组；横切 4 组：选区替换、面板期切窗后选择、管理员记事本、150% DPI 多屏边缘。**阻断项**：错误窗口写入 = 0；中文多行在记事本/Chrome/VS Code/微信 4/4 通过；Esc 零写入；面板 P95 ≤150 ms（100 次 hotkey→首帧，i5-8250U 基准机）。

## 8. 最可能翻车的 3 个假设

1. “Unicode 模式对普通输入框 ≥99%”：终端 CR 执行、老 Win32 控件、IME 首字符污染都可能拉低；per-app 模式表（FR-14，现 P1）大概率被顶成 P0。
2. “P95 ≤150 ms / 空闲 ≤35 MB”：Slint 冷面板 80–150 ms 属常态，不预建隐藏窗口必超；35 MB 需实测。两门禁可能迫使 Direct2D 降级或修订指标。
3. “GetGUIThreadInfo 校验够用”：Chromium 系 hwndFocus 恒等于顶层句柄，校验空转，安全全押前台+pid+切换检测；“错误写入=0”无法证明，只能被 48 组矩阵逼近。

## 我不同意 PRD 的地方

1. §6.5 把终端列为“需实机验证”太软：应直接写死 Unicode 黑名单、仅粘贴（理由见第 2 节）。
2. §8 的 35 MB/150 ms 过紧：建议改 ≤45 MB + 冷启动 ≤1 s；否则被迫自绘 UI，+60% UI 工期换 15 MB 不值。
3. §4.1“标题匹配优先于正文匹配”不可实现：必须落打分表（100/60/40/20），本方案已给。
4. 回收站在 14.4 有入口承诺、FR 清单却无优先级：应定 P1，且 deletedAt 字段进 schema v1，免 v2 迁移。
5. “暂停热键/输入模式”须持久化到 settings.json（PRD 未写，否则重启状态漂移）。
6. 10,000 上限应写明按 UTF-16 code units 计（≠字符数），否则验收用例 7 对 Emoji 有歧义。
7. §12 缺“注入期间用户继续打字”用例，本方案已入矩阵，建议 PRD 补为用例 13。
