# M0-3 验收记录（2026-11-18）

- 范围：快捷面板、搜索框、OwnerDraw 列表、键盘导航、显示定位、DPI
- 代码状态：工作区（M0-2 复核闭环后开始）
- 复核依据：`docs/_ref/final-plan.md` v4.1 §8 M0-3

## 已实现

| 项 | 实现 |
| --- | --- |
| 面板 | 520×420 逻辑像素，`WS_POPUP + WS_EX_TOPMOST + WS_EX_TOOLWINDOW`，DWM 圆角 |
| 搜索 | 原生 `EDIT` + cue banner，`EN_CHANGE` 实时过滤标题与正文 |
| 列表 | OwnerDraw `LISTBOX`，64 px 行高，标题/预览/分类三栏绘制，选中态 `#F0EDFF` |
| 键盘 | ↑↓ / Home / End 移动，Enter 提交，Esc 关闭；搜索框始终持有焦点 |
| 关闭 | Esc、Enter、再次热键、点击外部（`WM_ACTIVATE` 不抢回焦点） |
| 定位 | 优先光标矩形，回退鼠标位置；按显示器工作区夹紧；上下翻转避让屏幕边缘 |
| DPI | `GetDpiForMonitor` 取目标显示器 DPI，按比例缩放尺寸、字体、行高 |
| 视觉 | 搜索区浅底+细边框，DWM 圆角，Common-Controls v6 |
| 测试入口 | `--panel-test`、`--panel-bench`、`scripts/panel-smoke.ps1` |

## 实测

### 面板基准（release）

```
panel bench: n=100 p50=5724us p95=5724us max=...us
```

p95 **5.7 ms**，目标 ≤150 ms。首次显示包含窗口与字体创建，单独实测 109–118 ms，仍低于目标。

### 端到端（`WM_HOTKEY` 消息驱动；本机 UIPI 拦截真实 SendKeys）

```
hotkey fired
capture: identity=verified ... uia=15ms total=16ms
panel shown in 109936 us; capture total 16 ms
...
hotkey fired
capture: ... uia=8ms total=8ms
panel shown in 49481 us; capture total 8 ms
panel commit: title=项目进度回复 category=工作沟通 (M0-4 will insert)
```

- 面板显示：首次 110 ms，第二次 49 ms。
- 热键→捕获→面板整链路：首次 130 ms，第二次 60 ms。
- 面板尺寸：520×420（100% DPI）。
- Esc 关闭：通过；Enter 提交：通过并记录选择；点击外部：代码路径已实现，未自动化。

### 视觉

`docs/reviews/assets/m0-3-panel.png`（PrintWindow 渲染）：

- 搜索框可见浅底与边框，cue banner “搜索短语…”。
- 首项紫色选中，标题/预览/分类三段清晰。
- 分类标签颜色已修正（此前 `COLORREF` 字节序错误导致显示为红色）。

### 冒烟

```
panel bench exits 0          True
panel p95 <= 150 ms          True p95=5724us
panel visible after hotkey   True
panel is 520x420 at 100% DPI True
Esc hides panel              True
Enter hides panel            True
commit logged                True
app exits                    True
PASS: 8 checks
```

## 未覆盖

- 真实 `SendKeys` 热键在本机被 UIPI 拒绝，脚本用 `WM_HOTKEY` 消息驱动；真实热键在 M0-1 已手工验证。
- 125%/150%/200% DPI 与多屏边缘未实测（当前桌面 100%）。
- 中文 IME 组合态未实测（原生 EDIT，预期不吞 Enter，但未验证）。
- 点击面板外部关闭未自动化。
- 搜索过滤、Home/End、滚轮未进自动冒烟。
- 列表 3 条硬编码；存储与真实数据在 M1。

## 结论

M0-3 的功能与性能目标达到，未覆盖项明确留到 M0-5 手工矩阵。下一步 M0-4：恢复目标窗口、校验、`SendInput` 注入与三态结果。
