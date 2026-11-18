# M0 代码复核 r2（2026-11-18）

- 快照：`m0-code-20261118-r2`（`fix.diff` = `08f9879..ce96320`）
- 输入：`review-input/m0-code-20261118-r2/`（brief SHA256 `c2533b8e…3ad3`）
- 类型：focused，只核 r1 的 R1–R8
- 复核员：glm、kimi、qwen（per-run 只读）

## 结论

| 模型 | 结论 | 关键判断 |
| --- | --- | --- |
| glm | PASS | R1–R8 全 CLOSED；未发现新 blocker/major |
| kimi | CHANGES_REQUIRED | R1/R5/R8 PARTIAL；N1 原生 `ES_PASSWORD` 未进门闩（major） |
| qwen | PASS | R1–R8 全 CLOSED；N1/N2 为冒烟 SKIP 记账 minor |

## 分歧裁决

- **原生密码/只读门闩**：glm 把 R7 判 CLOSED，同时承认原生位“只用于报告”；kimi 明确指出这就是绕过点。按更严证据采纳 kimi，判 R1 PARTIAL。
- **队列上限**：glm/qwen 接受“互斥串行 + id 门闩 + 两次熔断”为等效有界；kimi 要求真正的单槽覆盖。采纳 kimi，改成 `Mutex<Option<Request>> + Condvar` 单槽邮箱。
- **冒烟 SKIP**：三家都指出 SKIP 被计入通过数；qwen 另指出“无新鲜报告”可能掩盖已登记失败。两者都修。

## r2 残留修复（提交 `5c21d24` 及其后）

| 项 | 修复 |
| --- | --- |
| 原生密码/只读 | `Gate` 增加 `native_password` / `native_read_only`，先于 UIA 拒绝 |
| UIA 密码未知 | `is_password` 改为 `Option<bool>`；`None` → `Unknown`（只复制） |
| 队列 | 真单槽邮箱，新请求覆盖旧请求；旧 reply 被 drop；Warmup 也带 id/deadline |
| 冒烟 SKIP | M0-1 输出 `PASS: 9 checks (1 skipped)`；capture-smoke 跳过时 exit 3，且先判已登记失败 |
| 单测 | 16 个（新增原生密码/只读、UIA 密码未知） |

## 修复后门禁

- `cargo fmt --check` / `clippy --release` / `build --release`：PASS
- `cargo test`：16/16
- M0-1：PASS 9 + 1 SKIP
- M0-2：PASS 15

## 状态

r2 残留修复尚未经过 r3 focused 复核；通过后才解除“不得进入 M0-3”。