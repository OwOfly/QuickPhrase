# QuickPhrase 优化计划（2026-11-24）

- 目标仓库：`D:\Code\AIProject\quickphrase`
- 基线：`6c6190b`（tag `v0.1.3`）；实际实施基线 `18d66f4`（另一轮冒烟/剪贴板/导入改动）
- 冻结输入：`review-input/optimization-20261124/`（全部 26 个 `src/*.rs`、脚本、清单）
- 参与：DeepSeek（主席）、glm-5.3#max、kimi-k3.1-pro#max、qwen4-max#max
- 方法：四份独立方案 → 按根因去重 → 主席裁决 → 模块化
- 状态：**实施中**；每个模块仍需三家复核

## 实施进度

| 模块 | 状态 | 提交 / 证据 |
| --- | --- | --- |
| 基线（另一轮） | 已提交 | `18d66f4` 隔离冒烟 + 剪贴板/导入 API |
| M1a 管理读路径 | 已提交 | `62f2800`；r2 三家 PASS（glm/qwen/kimi） |
| M1b 面板搜索键 | 已提交 | `afe1e00`；r2 三家 PASS |
| M2a usage 不重建 blob | 已提交 | `8f8383a`；r2 qwen/kimi PASS，glm r1 PASS（r2 上游不可用） |
| M3a D2D 缓冲/画刷复用 | 已提交 | `d8b4f06`；r2 三家 PASS |
| M2b usage 异步 FIFO | 已提交 | `7cd0c45`；r2 三家 PASS；worker FIFO 单测入库 |
| M4 注入/退出安全 | 已提交 | M4a `b1d9618`、M4b `a6e352d`；两轮均三家 PASS |
| M5 数据安全 UX | 已提交 | M5a `59874dc`、M5b `a968098`；两轮三家 PASS |
| M6 键盘/DPI/高对比度 | 已提交 | M6a `1c9dceb`、M6b `08fac62`、M6c `40ee048`；各轮三家 PASS |
| M7 测试/发布门禁 | 已提交 | M7a `c72b351`、M7b `439851d`；两轮三家 PASS |
| M8 清理/技术债 | 已提交 | `8f3d12f`；r2 三家 PASS；文件拆分/theme 缓存/UIA provider 延期 |

## 0. 结论先行

1. 产品当前最贵的不是功能，而是 **UI 线程上的整库克隆/整库写盘**：管理窗口每敲一个搜索键深克隆整库并全量 rank；每次使用统计都重建全库拼音 blob 并原子重写整库；仅复制提交还在 UI 线程同步做这件事。
2. **D2D 每帧资源 churn** 是第二个共性热点：按钮/行/卡片每次绘制新建 DIB + 内存 DC，每个填充新建画刷，面板 chip 每次鼠标移动重新测量。
3. **安全/数据路径** 有三处值得先修：注入前先抢前台再校验目标（顺序反了）；`WM_CLOSE`/关机绕过注入与脏草稿守卫；UIA 两次超时后永久降级。
4. **测试/发布门禁存在假绿**：`smoke.ps1` 把未观测到的热键跳过记为 PASS；`package.ps1 -SkipBuild` 只校验版本号，不绑定已测试的 EXE 哈希；性能门禁用 50 条小样本，测不到大库回归。
5. 不建议现在做：云同步/自动更新/遥测、全局钩子、SQLite 迁移、UI 框架重写、UIA provider 大工程（先保键盘可达与高对比度）。

## 1. 已经做得好的（不要制造工作）

- 存储原子写/备份/恢复链路完整：`ReplaceFileW`/`MoveFileExW`、5 份备份、revision 高水位、损坏恢复、失败注入测试（`storage.rs:1765-1824,2147-2187`）。
- `storage.rs` 生产代码没有裸 `unwrap/expect`（测试模块从 2361 行开始）。
- clippy `-D warnings` 干净，153 个测试通过；发布脚本校验资源版本与 zip 内容。
- v0.1.3 的自绘确认卡/下拉已补齐 `WM_CLOSE`、`WM_QUIT`、重入、焦点、Alt+Tab；剩余主要是测试与 DPI。
- 依赖树没有网络/遥测/自动更新/全局钩子。

## 2. 决策点（四份方案的分歧）

| 决策 | 选项 | 裁决 | 理由 |
| --- | --- | --- | --- |
| 管理窗口读路径 | A. revision 缓存 `DataFile`；B. 借用 store，只克隆显示行 | **B 优先，A 只用于面板** | 借用无缓存失效风险；管理窗口只需要显示行。面板需要跨消息持有数据，才用 revision 快照。 |
| 使用统计写盘 | A. 每次同步原子写；B. 防抖/后台 FIFO，退出前排空 | **B** | 只合并 usage 元数据；正文保存仍同步原子写。崩溃最多丢最近几次使用计数，不丢短语。 |
| 导入/恢复线程化 | A. 先加大小上限 + busy 态；B. 解析放工作线程 | **先 A，后 B** | 数据安全优先；线程化解析放第二阶段，改动面更大。 |
| UIA provider（自绘控件无障碍） | qwen 建议 P3；glm 明确不建议 | **暂缓** | 工程量大、无自动验收路径；先做高对比度与键盘可达，UIA provider 单独立项。 |
| CI | 本地门禁 vs GitHub Actions | **加 CI（P2）** | 只跑 fmt/clippy/test/package，不改变应用的离线属性。 |

## 3. 模块与优先级

### M1（P1）读路径与搜索：去掉整库克隆、全量排序、无用高亮

**证据**
- `manage.rs:239-241` `data()` = `data_clone()`；`active_rows`（243-316）克隆整库、克隆分类表、`rank(usize::MAX)`、对每条命中做全文 `replace` 生成 preview。
- 搜索 `EN_CHANGE → reload_list`（`manage.rs:3335-3342`）；保存后连续 5 次 reload（`1046-1050`），删除/批量同样 fan-out。
- `load_editor` 连续两次 `data()`（`manage.rs:640-643`）。
- `storage.rs:1733-1747` 每次 `snippets()` 重新 sort；`panel_items`（280-302）逐字段克隆。
- `panel.rs:1948-2007` 每键构建 `Vec<Doc>`，每条 tag 重复 lowercase；`search.rs:84-150` 对 title/content 逐条 lowercase；`search.rs:191-211` 高亮再 lowercase。
- `manage.rs:1858-1885` 每次重绘读完整标题/正文并 `chars().count()`。

**改动**
1. 增加 `storage::with_data(|&DataFile| ...)` 只读借用（不克隆）；管理刷新在借用内完成，只克隆 `PhraseRow` 的标题/预览。
2. `active_rows` 增加无高亮 rank 变体；preview 先截断（约 160 字符）再替换换行。
3. `load_editor` 只取一次快照（一次借用），不再两次 `data()`。
4. 保存/删除/批量统一走一个 `refresh_library(options)`，明确是否需要刷新分类/标签/编辑器。
5. 搜索输入加 50ms 防抖（可选，先测量再定）。
6. 编辑器计数器改 UTF-16 单位（与 `storage.rs:555-568` 一致），在 `EN_CHANGE`/load 时更新，不逐帧读全文。

**验收**
- 差分夹具：同一查询/分类/标签组合，IDs、顺序、选中、预览与旧实现一致。
- 1k/10k 条库：管理搜索 p95 明显下降；一次搜索变化 0 次整库 `DataFile` 克隆。
- 153 测试 + tag-batch-smoke 10/10。

**Effort** M；**风险** 中（刷新顺序/选中漂移）。回退：保留旧 `data()` 路径。

### M2（P1）写路径：usage 不重建 blob、不阻塞 UI

**证据**
- `storage.rs:1773-1776` `persist_data` 每次保存遍历全库 `rebuild_blob()`（拼音 + lowercase）。
- `record_use`（1797-1825）→ `save_data_with(false)` → 整库序列化 + 双 sync + 回读 + ReplaceFileW；每次插入/复制都走。
- `panel.rs:934-955` copy-only 在 UI 线程同步 `record_use`；注入路径已在工作线程（998-1024）。
- `storage.rs:825-827` `import_merge` 整库克隆两次。

**改动**
1. `persist_data` 去掉 blanket `rebuild_blob`；各 mutator 已负责重建（957/1043/1126/878/644/1570）。加 debug_assert + 每类 mutation 的单测，防未来漏重建。
2. usage-only 保存走防抖/FIFO 工作线程；退出前 drain；保持计数/状态顺序。正文/设置保存仍同步。
3. copy-only 的 `record_use` 移到后台（复用注入线程模式），气球留在 UI 线程。
4. `import_merge` 的 `previous` 改为惰性克隆或 undo delta。

**验收**
- 注入 500ms 写延迟时，copy-only UI 处理 <100ms。
- 退出后计数/状态精确；blob 字节不变；写失败原数据不变。
- `usage_updates_keep_all_content_restore_points` 等测试通过。

**Effort** M；**风险** 中（顺序/丢失统计）。回退：同步写 + 全量重建。

### M3（P1）D2D 绘制：复用缓冲、画刷、chip 几何

**证据**
- `d2d.rs:427-523` 每次 `paint_buffered` 新建 `CreateCompatibleDC` + `CreateDIBSection`，结束删除；按钮/行/卡片/滚动条都走它（`ui.rs:501/624/1069`，`panel.rs:1775`，`manage.rs:1156/1305`）。
- `d2d.rs:554-561` 每个填充/描边新建 `ID2D1SolidColorBrush`（文本格式已缓存）。
- `panel.rs:1234-1313` chip 每次鼠标移动/NCHITTEST 重新 `measure_style`（`d2d.rs:843-874` 每次 CreateTextLayout）。
- `panel.rs:1748-1776` 每次行绘制 `template::render` + 全文 replace；`2053-2061` 每次绘制再渲染。

**改动**
1. 线程局部、按尺寸只增不减的 scratch DIB + memDC，复用到下一次 `paint_buffered`；尺寸变化或设备丢失才重建。
2. Engine 内按 `(COLORREF, alpha)` 缓存画刷，和设备一起清空。
3. chip 几何缓存（population/DPI/theme/data 变化时失效）。
4. 行预览在 `populate` 时生成并截断；commit 时仍用新鲜模板（`{date}`/`{time}` 不能缓存）。

**验收**
- 同一尺寸 100 次重绘后新增 `CreateDIBSection` 为 0；1 万次绘制 GDI 句柄稳定。
- 面板 hover/滚动视觉不变；panel/tag-batch smoke 10/10。
- 基准 p50/p95 记录前后值。

**Effort** M；**风险** 中（缓存失效/嵌套绘制污染）。回退：逐次创建。

### M4（P1）注入与退出安全

**证据**
- `inject.rs:105-109` 在 `monitor_ok`/plan/`guard()`（289-315 的 IsWindow/进程校验）之前就 `SetForegroundWindow(target)`；复用的旧 HWND 可能先激活无关窗口。
- `app.rs:378-381` `WM_CLOSE` 直接 `DestroyWindow`；`411` `WM_QUERYENDSESSION` 无条件返回 1；托盘退出（470-485）却有注入/脏草稿守卫。
- `uia.rs:160-227` 连续两次超时永久 `unavailable`，无恢复路径。

**改动**
1. 把首次 `SetForegroundWindow` 移到 `guard()` 的目标校验之后（或删除，guard 内已有一次）。校验失败不激活、不输入、只复制。
2. 统一 `request_exit(reason)`：托盘退出、`WM_CLOSE`、session end 都走同一守卫；注入中延迟 + 气泡；脏草稿先确认；关机时注入中返回 FALSE 或短暂等待。
3. UIA 改为定时隔离：cooldown 后允许一次短超时探测，成功恢复、失败继续只复制；降级时发一次托盘提示。

**验收**
- 陈旧/复用 HWND 夹具：无前台激活、无输入。
- 注入中 post `WM_CLOSE`/模拟 session end：窗口不销毁；注入完成后可正常退出。
- 假时钟单测：2 次超时 → cooldown 内不请求 → 恰好一次探测 → 成功恢复/失败再隔离。
- host matrix / inject smoke 通过。

**Effort** M；**风险** 中（焦点/关机时序）。回退：保留旧退出分支。

### M5（P2）数据安全 UX：导入/恢复上限、危险确认、长标题

**证据**
- `settings_ui.rs:2638-2652` 导入 `MB_YESNOCANCEL`，`IDNO → Replace`；`2684-2691` 恢复备份也是原生 MessageBox；Replace/restore 的 `fs::read` 无上限（`storage.rs:424-439`），merge 有 20MiB + 10k/1k 条限制（573-575）。
- `manage.rs:1081-1105` 删除确认直接拼 80 UTF-16 标题；`confirm_ui.rs:355-384` 标题/正文/提示都是固定单行矩形，长标题会裁掉关键区分部分。
- `clipboard.rs:89-92` `OpenClipboard` 失败立即返回；copy-only 的安全兜底可能因剪贴板被占用而失败。

**改动**
1. Replace/restore 复用 merge 的有界读取（宽松上限，例如 256MiB 或按 10k×10k 估算），超限明确报错不 OOM。
2. “否 = 替换全库”后再弹一次 `confirm_ui` 危险确认；其余设置页二元确认逐步换自绘卡（可选，先做 Replace）。
3. 确认卡支持 2 行换行或标题 middle-elide；80 字符标题在 100/150/200% DPI 仍可区分。
4. `OpenClipboard` 3×30ms 有限重试，仍失败才报错。

**验收**
- 超大文件返回错误且不崩溃；Replace 确认链路 Esc 不改数据。
- 80 字符、仅尾部不同的标题在三种 DPI 下可区分；按钮/提示不被裁。
- 另一进程短暂占用剪贴板时重试成功。

**Effort** S-M；**风险** 低-中。回退：逐项恢复。

### M6（P2）键盘/DPI/高对比度

**证据**
- `panel.rs:2237-2269` 编辑框只处理 Up/Down/Home/End/Enter/Esc；分类/标签 chip 仅鼠标（2470-2578）；README 写“键盘全程可操作”。
- `dropmenu.rs:208-216` 用 anchor DPI 算一次高度；`confirm_ui.rs:103` 用 owner DPI；两者都不处理 `WM_DPICHANGED`；测试中 manage/anchor 120 DPI 但 picker 284×288（96 DPI 的 8×34 布局）。
- `theme.rs` 只有 light/dark，没有 `SPI_GETHIGHCONTRAST`。

**改动**
1. 面板加有界焦点模型（F6 循环搜索/分类/标签/结果；方向键移动 chip；Enter 应用；Esc 回搜索），或至少给分类加 Ctrl+Left/Right 循环并更新 README。
2. 弹层/确认卡从 owner 取 DPI，处理 `WM_DPICHANGED`，按建议矩形重排；在 96/120/144 下高度 = pad*2 + 8*row。
3. 高对比度：检测系统 HC，使用 `GetSysColor` 调色板；`WM_SETTINGCHANGE` 刷新。

**验收**
- 纯键盘完成“开面板→切分类→选标签→回结果→提交”；IME Enter 不受影响。
- 混合 DPI 下弹层尺寸/文字/命中一致；无边缘裁切。
- 高对比度黑白主题下文本可读；截图留档。

**Effort** M；**风险** 中（焦点/IME/主题）。回退：保留鼠标路径/原 DPI 计算。

### M7（P2）测试与发布门禁：消灭假绿

**证据**
- 自绘确认/下拉无 headless 回归；真实鼠标脚本依赖前台，今天无前台时 `GetForegroundWindow()==0`，脚本假失败。
- `smoke.ps1:95-124` 把未观测到的热键跳过记为 `Ok=true`，148-160 仍 exit 0；核心激活路径可能未验证却 PASS。
- `panel-smoke.ps1:130` 固定 520×420；多处固定 sleep（68/73/136/146/159）。
- `package.ps1` 只校验版本；`-SkipBuild` 可打包同版本的旧 EXE；zip 只校验条目名，不校验条目字节。
- 性能门禁只用 50 条小样本（`panel.rs:2702-2746`），测不到大库。
- 无 `.github/` CI。

**改动**
1. 新增 `scripts/ui-kit-smoke.ps1`：OPEN_PICKER、Down/Enter、lane click、WM_CLOSE、WM_ACTIVATE、confirm Esc/602；无前台时 SKIP 而非 FAIL。
2. `smoke.ps1` 热键跳过改为退出码 3（或显式 `-AllowHotkeySkip` 豁免）；发布门禁不接受静默 PASS。
3. 固定 sleep 改条件轮询；panel-smoke 尺寸按 DPI 缩放；增加搜索输入步骤。
4. 性能门禁：`--search-bench --large`（10k 文档 × 1k 内容）、manage 过滤基准、panel 热键到可见；记录 p50/p95/max 与硬件指纹，阈值放宽松并单独跑。
5. `package.ps1`：`-SkipBuild` 必须提供已测试 EXE 的 SHA256；校验 zip 内 EXE 字节与 staging 一致；校验 `RELEASE_NOTES-v$version.md` 存在；增加 manifest 版本一致性。
6. 加 Windows CI：fmt/clippy/test/package artifact；不改变应用离线属性。

**验收**
- 新 smoke 在无交互桌面 PASS；人为破坏 picker 打开逻辑会 FAIL。
- 热键未观测到时 smoke 非 0；正常桌面 exit 0。
- 20 次带 CPU 负载运行无 timing flake；故意缺失条件按 deadline 失败。
- 同版本旧 EXE 无法通过 `-SkipBuild`；zip 字节篡改被发现。
- 大库基准能捕获 2× 性能回退。

**Effort** M；**风险** 低（测试/脚本）。回退：逐脚本恢复。

### M8（P3）清理与技术债

- `scaled`/`point_in`/`force_foreground`/读文本 helper 去重（qwen）。
- `manage`/`settings_ui` 纯函数补单测（parse_tags、error_text、active_rows、sort_label、data_error_text）。
- 3k 行文件拆分（机械 move，单独提交）。
- 清理 `backups/{stem}.tmp` 遗留（glm）。
- `theme::current()` 缓存（glm）。
- 文本 layout 缓存（chair）。
- 发布 checklist 脚本（chair）。
- UIA provider（qwen P3）暂缓，单独立项。

**Effort** S-L；**风险** 低-中。每项单独验收。

## 4. 建议顺序

1. **先测量**：M7 的 large-corpus bench（1-2 天）→ 确认 M1/M2/M3 的真实收益。
2. **M1 + M2**（读/写热路径）：用户感知最强，风险可控。
3. **M3**（D2D 复用）：与 M1 一起做视觉回归。
4. **M4**（注入/退出安全）：安全优先，独立小 diff。
5. **M7**（测试/发布门禁）：在动数据路径前先把护栏补上。
6. **M5 + M6**（数据 UX/键盘/DPI）。
7. **M8** 清理。

## 5. 快速收益（S，低风险）

1. `inject.rs:105-109` 删除/后移首次 `SetForegroundWindow`（先校验目标）。
2. `storage.rs:1773-1776` 去掉每次保存的全库 blob 重建 + 补单测。
3. `panel.rs:940` copy-only `record_use` 移后台线程。
4. `manage.rs:640-643` `load_editor` 只取一次快照。
5. `panel.rs:2162` 去掉每键同步日志（或聚合）。
6. `confirm_ui` 长标题 elide/wrap。
7. `app.manifest` 版本从 0.1.0.0 同步到 0.1.3。
8. `d2d.rs:554-561` 画刷缓存。
9. `clipboard.rs:89-92` OpenClipboard 3×30ms 重试。
10. `smoke.ps1` 热键跳过不再记 PASS。

## 6. 不做

- 云同步、自动更新、遥测、全局键盘钩子、注入 DLL。
- SQLite/新存储格式迁移；现有原子写 + 备份链路是核心保证。
- UI 框架重写（Electron/WebView/egui）或 async runtime。
- 把正文保存改成后台队列（只允许 usage 统计合并）。
- 无 revision/失效条件的行缓存（会显示已删除短语，像数据丢失）。
- 为自绘表面做像素比对作为主要回归（跨 DPI/主题脆弱）。
- 放宽密码/只读/UIA/前台校验，或 SendInput 失败自动重试。
- 现在做 UIA provider 大工程（先保键盘/高对比度）。

## 7. 待确认

- 目标库规模：典型 100/1k/10k 条？决定 M1/M2 的优先级与基准规模。
- 是否接受 usage 统计“崩溃丢最近几次计数”的防抖取舍？
- 是否要现在加 GitHub Actions（需要仓库 Actions 权限/分钟预算）？
- 分类/标签 chip 的键盘模型：F6 焦点环 vs 仅加分类快捷键（先小改）？
- 高对比度是 P2 还是 P3？
- UIA provider 是否单独立项（本次不做）？

## 8. 复核来源

- `optimization-chair-proposal.md`（主席）
- glm：整库克隆、blob 重建、lowercase 逐条、D2D/画刷、chip 测量、导入线程、编辑器全窗重绘、性能门禁小样本、import 双克隆、backups tmp、theme 缓存。
- kimi：退出一致守卫、usage 写盘、整库克隆、D2D 缓冲、UIA 恢复、注入前目标校验、设置大操作、重复过滤/日志、top-k、chip/预览缓存、UTF-16 计数、chip 键盘、长标题、性能门禁、smoke 假绿、固定 sleep、package 溯源。
- qwen：自绘弹层回归、管理搜索克隆/全文 replace/无用高亮、面板每键/每行重算、WM_CLOSE 守卫、UIA 降级、导入大小上限、设置 No=Replace、D2D/chip、blob 重建/双解析、copy-only UI 写、WM_CTLCOLOREDIT、OpenClipboard、panel-smoke DPI、package 门禁、分类键盘、helper 去重、纯函数测试、manifest 版本。