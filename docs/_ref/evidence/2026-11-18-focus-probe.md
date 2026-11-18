# 证据：Chromium/Electron 焦点探测（2026-11-18）

## 目的

验证 PRD 假设“选择后可校验原焦点控件”在 Chromium 系是否成立。

## 环境

- Windows 11，2026-11-18
- 被测前台应用：OpenChamber（Electron，窗口类 `Chrome_WidgetWin_1`）
- 工具：`focus-probe.ps1`（本目录），PowerShell + P/Invoke

## 复现

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\focus-probe.ps1
```

## 原始输出

```
Process                     : OpenChamber (18588)
Foreground                  : 0x40A80 [Chrome_WidgetWin_1]
GTIOk                       : True
Active                      : 0x40A80 [Chrome_WidgetWin_1]
Focus                       : 0x40A80 [Chrome_WidgetWin_1]
Caret                       : NULL
FocusIsOwnChildOfForeground : True
```

## 结论

- `GetGUIThreadInfo().hwndFocus` 返回顶层窗口本身，不是页面内的输入控件。
- `hwndCaret` 为 NULL。
- 仅凭 HWND 无法区分同一页面的输入框 A 与 B；必须叠加 UI Automation 元素身份校验。

## 限制（必须诚实记录）

- 本次只成功观测到一个 Electron 应用；尝试拉起记事本与 Edge 时，无交互桌面的 shell 未能把目标窗口切到前台，因此没有取得原生 EDIT 与 Chrome 浏览器的对照数据。
- 该脚本只读，不注入、不修改任何状态。
- M0-2 必须在真实交互桌面上重跑，并补充：记事本、Chrome `input`/`textarea`/`contenteditable`、VS Code、Word。
- 本条证据只支持“Chromium 系 HWND 不能定位控件”，不支持任何关于注入成功率的结论。
