# M0-3 验收记录（2026-11-18，复核修复后）

- 范围：快捷面板、搜索、OwnerDraw 列表、键盘导航、定位、DPI
- 复核：r1/r2/r3 发现全部闭环；r4 focused：glm PASS、qwen PASS、kimi K11 全 CLOSED（仅哈希项受只读工具限制）
- 复核依据：`docs/_ref/final-plan.md` v4.1 §8 M0-3

## 已实现

| 项 | 实现 |
| --- | --- |
| 面板 | 520×420 逻辑像素，`WS_POPUP + WS_EX_TOPMOST + WS_EX_TOOLWINDOW`，DWM 圆角 |
| 预创建 | 启动时创建隐藏面板，首次热键不再付窗口/字体创建成本 |
| 搜索 | 原生 `EDIT` + cue banner + `EM_LIMITTEXT 200`，`EN_CHANGE` 实时过滤 |
| 列表 | OwnerDraw `LISTBOX`，64 px 行高，标题/预览/分类三段，选中态 `#F0EDFF` |
| 键盘 | ↑↓ / Home / End 移动，Enter 提交，Esc 关闭；列表被点击后自动把焦点还给搜索框 |
| 关闭 | Esc、Enter、再次热键、WM_CLOSE 恢复目标；点击外部只隐藏不抢回焦点；重复关闭幂等 |
| 定位 | 光标矩形（已从客户区转屏幕坐标）优先，鼠标位置回退；工作区夹紧；尺寸超过工作区时按工作区封顶 |
| DPI | `GetDpiForMonitor` 取目标屏 DPI，四舍五入缩放；`WM_SIZE` 重排子控件；`WM_DPICHANGED` 采用建议矩形并刷新字体/行高 |
| 前台监听 | 面板显示期间启动 `EVENT_SYSTEM_FOREGROUND`，隐藏时停止（G3 在 M0-4 提交校验时消费） |
| 焦点安全 | 取不到前台时默认隐藏（不留死面板）；测试用 `QUICKPHRASE_TEST_ALLOW_UNFOCUSED_PANEL=1` 例外 |
| IME | Enter 前检查 `ImmGetCompositionStringW(GCS_COMPSTR)`，组合态交给 IME |
| 资源 | 面板创建失败回滚窗口/字体/画刷；退出与 `--panel-test`/`--panel-bench` 调用 `shutdown()` |
| 入口 | 热键、托盘“打开快捷面板”、双击托盘统一走 `activate_panel()` |

## 复核修复对照

| 发现 | 处置 |
| --- | --- |
| caret 客户区坐标当屏幕坐标 | `ClientToScreen(hwndCaret, …)` 转换矩形两个顶点 |
| 点击列表后 Enter/Esc 失效 | `LBN_SELCHANGE` / `LBN_SETFOCUS` 把焦点还给 EDIT |
| 再次打开时搜索文本与列表不一致 | 显示前清空 EDIT |
| 高 DPI 子控件不重排 | 增加 `WM_SIZE`，`place()` 缩放后自动 layout |
| 面板高于工作区越界 | 宽高按工作区封顶；列表高度下限 0 |
| 取不到前台留死面板 | 默认隐藏并返回失败；测试环境变量例外 |
| 重复关闭抢焦点 | `hide()` 先检查可见性，已隐藏直接返回 |
| 托盘入口仍是占位 | 统一走 `activate_panel()` |
| AttachThreadInput 挂起风险 | 先 `IsHungAppWindow` 判断 |
| 创建失败泄漏/静默 | 回滚资源；`RegisterClassW` 检查；app 记录失败并提示 |
| 日志记录短语标题 | 改为 `index + category` |
| DPI 缩放截断 | 改为四舍五入 |
| `WM_SIZE` 持借用重排导致重入 | 布局改为自由函数，先复制 HWND/DPI 再释放借用 |
| `populate` 持借用发 LB 消息 | 过滤结果与标题先算好，发消息时不持借用 |
| `shutdown` 持借用销毁窗口导致 panic | 先 `take()` 出面板，再 DestroyWindow |
| 面板退出卡住 | 前台监听线程改为 detach，不 join 阻塞退出 |
| 列表鼠标点击后键盘失效 | `LBN_SELCHANGE`/`LBN_SETFOCUS` 回焦 EDIT |
| 空结果 Enter 仍关闭 | 无匹配项时忽略提交，保持面板 |
| 恢复目标已销毁 | `IsWindow` 校验并记录失败 |
| 搜索框 Ctrl/Shift+方向键被劫持 | 有修饰键时交还原 EDIT 过程 |
| EDIT 子类化失败未检测 | 旧过程为 0 时回滚并标记创建失败 |
| 面板单测缺失 | 新增过滤与缩放 3 个单测，总计 22 |

## 实测

### 面板基准（release，计时含创建后的完整 present）

```
panel bench: n=100 p50=4776us p95=7511us max=22261us
```

p95 **7.5 ms**，目标 ≤150 ms。

### 端到端（`WM_HOTKEY` 消息驱动；本机 UIPI 拦截真实 SendKeys）

Word 前台（`OpusApp` / `_WwG`），真实 caret 命中：

```
capture: identity=verified capability=document region=text_editor
         uia=[class=_WwG type=50030 ro=None text=true sel=1 caret=Some(true)]
panel shown in 133666 us; capture total 46 ms
...
panel shown in 52812 us; capture total 7 ms
panel commit: index=0 category=工作沟通 (M0-4 will insert)
```

- 首次显示 134 ms、第二次 53 ms（均为合成 WM_HOTKEY，焦点夺取走 `AttachThreadInput`；真实热键应更快）。
- 面板尺寸 520×420 @100% DPI。
- `region=text_editor`、`caret=Some(true)` 证明 Word 光标定位链路生效。

### 冒烟（10 项）

```
panel bench exits 0          True
panel p95 <= 150 ms          True p95=8540us
panel visible after hotkey   True
panel is 520x420 at 100% DPI True
Esc hides panel              True
Esc restores foreground      True
Enter hides panel            True
commit logged first item     True
Enter restores foreground    True
app exits 0                  True
PASS: 10 checks
```

### 视觉

`docs/reviews/assets/m0-3-panel.png`：搜索框浅底+细边框+cue banner、首项紫色选中、标题/预览/分类三段、分类标签紫色。

## 未覆盖 / 延后

- **捕获失败时的“仅复制”面板**：方案 §5.2 要求面板照常打开并提示只复制；本阶段剪贴板与注入未实现，整条链路在 M0-4 一起做，当前失败只弹托盘气泡。
- 125%/150%/200% DPI、多屏边缘、负坐标：`WM_SIZE`/建议矩形已实现，但未实测。
- 真实键盘焦点路径：冒烟用 `PostMessage(WM_KEYDOWN)` 直达 EDIT，未验证 `GetFocus`。
- 点击外部关闭：代码路径有，未自动化。
- 中文 IME 组合态：守卫已加，未用真实 IME 验证。
- 搜索过滤、Home/End、滚轮、连续 20 次热键未进自动冒烟。
- bench 走 `activate=false`，不含前台切换；真实激活路径只有 2 个样本。
- 列表 3 条硬编码；存储与检索在 M1。

## 结论

M0-3 的功能、定位、焦点契约与性能达到当前阶段目标；r4 focused 通过，未覆盖项已明确归属 M0-4/M0-5。**M0-3 过门，M0-4 可开。**