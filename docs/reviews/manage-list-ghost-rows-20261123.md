# 删除短语后的列表幽灵行修复（2026-11-23）

- 类型：发布后 UI 缺陷修复（v0.1.0 → 待定 v0.1.1）
- 提交：`f3b7104`
- 范围：`src/manage.rs`、`src/panel.rs`、`src/ui.rs`
- 发现：用户删除短语后截图，列表标题显示 2 项，但界面残留 4 张已删除卡片
- 结论：glm / kimi / qwen 初轮与 focused 复验均 PASS；修复进入 master，补丁版本待发布

## 现象

管理窗口把 6 条短语删到 2 条后，列表标题与左侧计数都显示 2，但列表仍显示 4 张已删除的“退款说明”卡片。数据本身正常（2 条活动 + 3 条回收站）；重启或强制重绘后残影消失。

## 根因

`reload_list` 在 `LB_RESETCONTENT` + 重新添加行后只调用 `InvalidateRect(manage.hwnd, ...)`，没有让 listbox 自己带擦除地重绘。owner-draw listbox 在行数减少时不会可靠地擦除最后一行以下的旧像素，于是已删除卡片留在屏幕上。

复现必须用屏幕截图（`CopyFromScreen`）；`PrintWindow` 会触发一次完整重绘，反而看不到残影。

## 修复

| 位置 | 修改 |
| --- | --- |
| `manage::reload_list` | 重填后 `InvalidateRect(manage.list, None, true)`，并保留父窗口失效 |
| `manage::reload_left` | 分类变化后对左侧列表带擦除失效 |
| `panel.rs` 筛选重填 | 过滤导致行数收缩后对面板列表带擦除失效 |
| `ui::smooth_listbox_proc` | 新增 `WM_ERASEBKGND`：用 `ListSurface` 对应底色（`palette.card` / `palette.sidebar`）填满客户区并返回 1 |
| 失败回退 | `GetClientRect` 失败、空客户区或 `CreateSolidBrush` 失败时不返回 1，落到原 proc 默认擦除，避免假报“已擦除” |

`WM_ERASEBKGND` 的填充在 `WM_PAINT` / `WM_DRAWITEM` 之前执行，行卡片、圆角和覆盖式滚动条随后重画；填充受 update region 裁剪。

## 验证

- 屏幕截图复现（6→2）：
  - 修复前 `dist/ui-shots/delete-ghost-screen-before.png`：残留 4 张“退款说明”
  - 修复后 `dist/ui-shots/delete-ghost-screen-after2.png`：只剩 2 行，下方干净
- 最终构建再次执行 6→2 删除，屏幕抓取因桌面锁定只拍到锁屏壁纸，未能形成新的可视证据；成功路径代码与 `after2` 验证时一致（后续只加失败回退与面板失效）。需在解锁桌面后由用户目检一次。
- `cargo fmt --check` PASS
- `cargo clippy --release -- -D warnings` PASS
- `cargo test` 150/150
- panel-smoke 10/10（最终构建）
- tag-batch-smoke 10/10（最终构建）

## 复核

| 模型 | 初轮 | 要点 | focused |
| --- | --- | --- | --- |
| glm-5.3#max | PASS | question：主面板筛选收缩是否覆盖 | PASS |
| kimi-k3.1-pro#max | PASS | 无 defect；确认无重入/闪烁回归 | PASS |
| qwen4-max#max | PASS | minor：刷子创建失败仍返回 1，会静默保留幽灵行 | PASS |

- 处置：失败路径回退默认擦除；主面板筛选路径补列表自身失效；`reload_left` 的失效在 unsafe 块内无条件执行。
- 流程说明：沿用轻量 defect 复核，diff 与证据内联；未生成完整 snapshot 链，使用全局 `reviewer-readonly`（fallback）。残余风险：reviewer 理论可读 live repo。
- 复核原始输出：本会话 subagent 记录（三家初轮 + 三家 focused）。

## 未覆盖 / 后续

- 没有自动化像素级回归测试；该缺陷依赖屏幕像素，本轮用屏幕截图人工复现与验证。
- 最终构建的可视复验被锁屏阻断，发布前需解锁桌面再删一次确认。
- 是否发布 `v0.1.1` 补丁版待用户确认；`v0.1.0` 的 Release 资产保持不动。