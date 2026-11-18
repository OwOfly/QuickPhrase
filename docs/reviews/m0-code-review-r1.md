# M0 代码复核 r1（2026-11-18）

- 快照：`m0-code-20261118-r1`（base `84bf30a` → 工作区）
- 输入：`review-input/m0-code-20261118-r1/`（brief SHA256 `673f1f11…78bda1`）
- 复核员：glm-5.3#max、kimi-k3.1-pro#max、qwen4-max#max（per-run 只读 agent，仅读输入目录）
- 结论：三家全部 `CHANGES_REQUIRED`

## 根因归并

| # | 根因 | 命中 | 处置 |
| --- | --- | --- | --- |
| R1 | 只读未知/双源冲突被当成可写 | 3/3 | `combine_read_only`：任一 true 即只读；false+unknown 仍为 unknown；未知一律只复制 |
| R2 | Document、无插入点、禁用未参与放行 | 3/3 | `decide_capability`：Document、`is_enabled=false`、无 caret/选区 → 只复制 |
| R3 | UIA 超时/否定证据回退成原生 `native_edit` | qwen/kimi/glm | 删除原生回退；UIA 缺失 → `uia_unavailable`，否定 → `unknown` |
| R4 | 提权/完整性未知未进入拒绝分支 | kimi | `Gate.target_higher` / `integrity_unknown` 优先于一切能力判定 |
| R5 | UIA 无请求 ID/deadline，过期结果写回 `last` | 3/3 | 请求带 id + deadline；`latest` 原子门闩；过期不执行、不回写 |
| R6 | COM 初始化/反初始化不配对 | 3/3 | 检查 `CoInitializeEx`；失败直接返回；`CoCreateInstance` 失败路径补 `CoUninitialize` |
| R7 | `ES_PASSWORD`/`ES_READONLY` 未按窗口类过滤 | qwen | 仅对 `Edit`/`RichEdit`/`_WwG` 生效，避免 `BS_LEFTTEXT` 误判 |
| R8 | 单实例失败 fail-open、前台是自己仍捕获、日志跨午夜/轮转失败、托盘提示不认暂停、按 exe 缺省允许多行的旧逻辑 | glm/qwen/kimi | 逐项修复；详见 `git show` |

## 额外修复（复核未列但同源）

- 隐私：UIA `CurrentName` 可能含控件正文，已从快照和 JSON 移除（此前 capture JSON 曾包含整段聊天文本）。
- 冒烟门禁：`compare_last` 必须为 `Some(true)`；UIA 必须存在且 `ancestor_verified=true`、`foreground_stable=true`；预算阈值 60 ms；M0-1 日志断言改用本轮增量。
- `--selftest`/`--bench` 在热键注册失败时返回 3。
- 前台监控安装失败不再返回假监听器。
- 日志轮转失败不再 truncate 当前文件。

## 修复后机械门禁

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --check` | PASS |
| `cargo clippy --release` | PASS（0 error / 0 warning） |
| `cargo test` | PASS 14/14（新增能力判定与只读组合单测） |
| `cargo build --release` | PASS |
| M0-1 冒烟 | PASS 9 项 + 1 SKIP（热键注入被 UIPI 拦截，SKIP 不计通过） |
| M0-2 冒烟 | PASS 15 项 |

## 残余

- 焦点切换、UIA 挂起、原生 EDIT、提权目标仍无运行时实测，留 M0-5。
- 本轮修复尚未经过 focused 复核。
- 无存储/注入代码，M0-3 尚未开始。