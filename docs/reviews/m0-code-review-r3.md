# M0 代码复核 r3（2026-11-18）

- 快照：`m0-code-20261118-r3`（`ce96320..d1a4fa1`）
- 输入：`review-input/m0-code-20261118-r3/`（brief SHA256 `4d4ece47…9e34f`）
- 类型：focused，只核 K1–K4
- 复核员：glm、kimi、qwen（per-run 只读）

## 结论

| 模型 | K1 | K2 | K3 | K4 | 总裁决 |
| --- | --- | --- | --- | --- | --- |
| glm | CLOSED | CLOSED | CLOSED | PARTIAL | CHANGES_REQUIRED |
| kimi | CLOSED | CLOSED | CLOSED | PARTIAL | CHANGES_REQUIRED |
| qwen | CLOSED | CLOSED | CLOSED | PARTIAL | CHANGES_REQUIRED |

三家的唯一未闭环项完全相同：K4 要求 K2 关键分支具备离线单测，而快照内只有 K1 的测试。

## K4 闭环（快照后）

提交 `d8d24a9` 增加 3 个测试，`cargo test` 19/19：

| 测试 | 覆盖 |
| --- | --- |
| `mailbox_replaces_pending_request_and_drops_old_reply` | 新请求覆盖旧请求；旧 reply 立刻 `Disconnected` |
| `stale_predicate_rejects_newer_id_and_expired_deadline` | 更旧 ID 与过期 deadline 均拒绝 |
| `expired_capture_is_not_committed` | 过期 Capture 不进入 `last` 写回（同一判定函数） |

`is_stale` 拆出 `is_stale_at(now)` 纯函数，测试不依赖系统时钟。

## 无新 blocker/major

三家均未在本轮 diff 中发现新的错误位置输入、崩溃或安全边界绕过。

## 技术债（不阻塞）

1. `scripts/capture-smoke.ps1`：日志有 `capture written:` 但无新鲜报告时，现已改为登记失败后 exit 1（快照后修复）。
2. `Mailbox::submit` 在互斥量中毒时静默丢请求；release 使用 `panic=abort`，中毒实际不可达，记为已知残留。
3. `scripts/smoke.ps1` 在 SendKeys 未抛异常但未观察到热键时记 SKIP，无法区分 UIPI 与真实失效；已有手工证据，记为已知限制。

## 最终判定

**PASS**（chair closure）：

- K1–K3：三家一致 CLOSED。
- K4：快照内 PARTIAL，快照后以 `d8d24a9` 的 3 个离线测试闭环，19/19 通过。
- 无未闭环 blocker/major。

M0-3 解除阻塞。下一步开始面板：原生 `EDIT` 搜索框、OwnerDraw 列表、键盘导航、多屏定位。