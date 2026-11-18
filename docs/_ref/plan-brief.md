# QuickPhrase 方案议会 · 简报 v1

- 日期：2026-11-18
- 主席：DeepSeek-V4.1-Flash
- 议员：glm-5.3#max、kimi-k3.1-pro#max、qwen4-max#max
- 仓库：`D:\Code\AIProject\quickphrase`（当前为空，greenfield，无历史代码）

## 0. 你的任务

为一个尚未开工的 Windows 桌面小工具产出**可执行的技术方案**。没有既有代码可读，判断只能来自工程经验与一手事实。

禁止：修改任何文件、执行 shell、git 操作、输出"视情况而定"式结论。
必读：同目录 `prd-v1.1.md`（PRD 全文）。
可选：`prototype-v1.1.html`（交互原型）、`design.png`（视觉稿）。

## 1. 产品定义（摘自 PRD）

全局快捷短语输入器。热键唤起搜索面板 → 选中短语 → 文本写入**触发前的原输入位置**。单机、离线、无账号。
不是输入法、不是剪贴板历史、不做全局文本替换/缩写扩展。
目标用户：客服、开发、办公，重复话术、代码片段、SQL、邮件模板。

## 2. 硬约束（PRD 自定；可质疑，但必须给数据与替代值）

| 项 | 目标 |
| --- | --- |
| EXE 体积 | ≤ 15 MB |
| 空闲内存 | ≤ 35 MB |
| 面板显示延迟 | P95 ≤ 150 ms |
| 冷启动至可响应热键 | ≤ 1.5 s |
| 1,000 条本地检索 | P95 ≤ 50 ms |
| 单条正文上限 | 10,000 字符 |
| 平台 | Windows 10/11 x64 |
| 依赖 | 不依赖 WebView2；不要求管理员权限；默认不开机自启 |
| 热键 | `RegisterHotKey`，不使用全局键盘钩子 |
| 注入 | 默认 `SendInput` + `KEYEVENTF_UNICODE`；兼容模式才用剪贴板 |
| 数据 | `%LocalAppData%\QuickPhrase`，JSON + 原子写 + 最近 5 份滚动备份 |
| 安全红线 | 目标窗口不确定就不注入；错误窗口写入 = 发布阻断项 |

## 3. 必须回答的问题

1. **技术栈**：Rust + `windows` crate（UI 可选 Slint / egui / Direct2D 自绘）、C# WPF/WinForms、C++ Win32，选哪个。给体积、内存、开发量、中文 IME、高 DPI、多屏的证据。若 ≤15 MB 不现实，直接说并给替代数字。
2. **注入设计**：`KEYEVENTF_UNICODE` 的真实边界——换行发 CR 还是 LF、Emoji 代理对、Tab 字符、>2,000 字长文本、注入期间用户继续按键、终端宿主（WindowsTerminal/conhost/mintty）把换行当命令执行。给降级策略与具体阈值。
3. **焦点校验**：弹窗前抓什么（`GetForegroundWindow` / `GetGUIThreadInfo` / `hwndFocus` / `rcCaret`），关闭后如何验证回到原控件，失败判据与 API 调用序列。
4. **风险定级**：UIPI/管理员窗口、杀软误报、浏览器与 Electron 的 `contenteditable` 焦点恢复、IME 组合态、游戏/受保护进程。哪些能工程解决，哪些只能声明不支持。
5. **数据模型**：分类实体、回收站、排序规则、导入冲突、`schemaVersion` 迁移。给字段与规则。
6. **模块划分**：M0（可行性验证）与 M1（MVP），逐模块给职责、依赖、验收标准、预估人日。
7. **M0 测试矩阵**：宿主 × 场景，最少覆盖多少组、哪些是阻断项。
8. **最可能翻车的 3 个假设**：PRD 里哪三条最经不起实机验证。

## 4. 输出格式

Markdown，≤ 2,500 中文字。分节对应问题 1–8，末尾追加一节「我不同意 PRD 的地方」。

要求具体：库名、API 名、阈值、数字、失败判据。不要贴大段代码。
