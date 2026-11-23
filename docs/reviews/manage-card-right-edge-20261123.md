# 列表卡片右侧遮挡修复（2026-11-23）

- 类型：发布前 UI 缺陷修复
- 范围：`src/ui.rs` 覆盖式列表滚动条（管理短语列表、左侧分类列表、主面板列表共用）
- 发现：人工验收时用户指出选中“接口开发说明”卡片右侧缺口
- 结论：glm / kimi / qwen 三家 PASS，进入 `v0.1.0` 发布包；标签仍待人工桌面矩阵

## 根因

`paint_pill_scrollbar` 在 `metrics=None`（列表不需要滚动）时，仍先用列表背景色填满右侧车道（`scaled(10)`），再返回。行绘制的右内边距在不滚动时只有 `scaled(6)`，选中卡片的右缘、1px 边框和圆角落在车道内，被刷成白色，视觉上像被一块白块遮住。

## 修复

- `paint_pill_scrollbar`：无滚动条时在填充车道前直接返回。
- `SmoothListState` 增加 `thumb_painted`：可滚动时置位；变为不可滚动时仅在屏上绘制路径读取并清除一次，并 `InvalidateRect(整客户区, false)` 重绘，清掉旧滑块残影。
- `WM_PRINTCLIENT` 截图路径不读写该标记，避免消费屏上待清理状态。

## 复核

三家独立模型（provider `es`，只读；diff 与上下文内联）：

| 模型 | 首轮 | 发现 | 复验 |
| --- | --- | --- | --- |
| glm-5.3#max | PASS | minor：客户端过小或绘制失败时 `thumb_painted` 仍置 true，最多一次冗余重绘，无视觉缺陷（接受为技术债） | — |
| kimi-k3.1-pro#max | CHANGES_REQUIRED | minor：`WM_PRINTCLIENT` 会消费 `thumb_painted`，旧滑块可能残留 | PASS |
| qwen4-max#max | PASS | 同一 minor；另问 count→0 时车道清理 | PASS |

- 处置：截图路径的读取/置位/清除全部加 `target_hdc.is_none()` 守卫。
- count→0：管理窗口 `WM_CTLCOLORLISTBOX` 返回卡片底色刷子，空列表与行外区域的重绘由该背景刷覆盖，question 关闭。
- 流程说明：单文件小修复，未生成完整 snapshot 链；使用全局 `reviewer-readonly`（fallback）并以 diff 内联方式复核。残余风险：reviewer 理论可读 live repo；本轮未读，结论仅基于内联 diff 与上下文。
- 复核记录：本会话三个 subagent 的首轮与 kimi/qwen focused 输出。

## 机械门禁（final 构建）

- `cargo fmt --check` PASS
- `cargo clippy --release -- -D warnings` PASS
- `cargo test` 150/150
- panel-smoke 10/10（14:37，本构建）
- tag-batch-smoke 10/10（14:37，本构建）
- 视觉：同一窗口 1265×809 / 125% DPI，`dist/ui-shots/manage-final-check.png`；卡片右缘 x=638 为边框色，x=639 起为空白，左右各 13px，边框与圆角完整。
- 构建：exe `1,075,712` bytes，SHA256 `1c1f89fe7da47a8b488dadd2c259d3efe43901cda309889b770827b56eefd187`；zip `590,329` bytes，SHA256 `4e89ba752a5d0ab5908af73f840443dc961db93d29e730371a15097420f8122a`。

## 未覆盖

- 未在可滚动长列表下人工滚动并截图；可滚动路径的绘制逻辑未改，只增加状态位，panel-smoke / tag-batch-smoke 覆盖滚动与选择路径。
- 未做动画与高对比度主题专项检查。

## 结论

三家复核 PASS，缺陷闭环，已进入发布包；`v0.1.0` 标签与 GitHub Release 仍等待人工桌面验收。