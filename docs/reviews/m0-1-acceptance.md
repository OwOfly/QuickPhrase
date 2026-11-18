# M0-1 验收记录（2026-11-18）

- 范围：工程骨架、DPI manifest、单实例、托盘、`RegisterHotKey`、滚动日志
- 代码状态：工作区，未提交
- 环境：Windows 11 x64；rustc 1.93.0-nightly（nightly-2025-12-04，MSVC）；Windows SDK 10.0.26100.0
- 复核依据：`docs/_ref/final-plan.md` v4.1 §8 M0-1

## 构建产物

| 配置 | 大小 |
| --- | ---: |
| debug | 304,640 B（≈298 KB） |
| release（`opt-level="z"` + LTO + strip） | 159,232 B（≈155 KB） |

## 验收结果

| 项 | 判定 | 实测 |
| --- | --- | --- |
| 冷启动至可响应热键 ≤1.5 s | PASS | 5 次 `--bench`：47/43/43/41/40 ms，中位 43 ms（release） |
| 退出释放全局热键 | PASS | 关闭后立即跑 `--selftest`，同一热键注册成功，exit 0 |
| 重复启动不产生第二进程 | PASS | 第二实例 exit 0；首实例日志出现 `second instance activation requested`；首实例存活 |
| 托盘常驻 | PASS | `Shell_NotifyIconW(NIM_ADD)` 返回真；日志 `tray icon added`；`TaskbarCreated` 重新添加已接线 |
| 优雅退出 | PASS | `WM_CLOSE` 后进程退出，日志 `message loop ended` |
| 热键真实触发 | PASS | 注入 `Ctrl+Alt+Q` 后日志 `hotkey fired`，占位对话框出现 |
| DPI manifest 嵌入 | PASS | `mt.exe -inputresource:...;#1` 成功导出，含 `PerMonitorV2` |
| 运行时空闲内存 | PASS（早期样本） | release 启动约 1 s 后 Working Set 9.71 MB |
| 日志滚动 | 已实现，未压测 | 500 KB/文件，保留 3 份 |

## 证据

`scripts/smoke.ps1` 输出：

```
cold start median <= 1500 ms             True runs=47,43,43,41,40 ms; median=43
first instance alive                     True pid=55316
idle working set <= 35 MB (early sample) True 9.71 MB at ~1 s
second instance exits 0                  True exit=0
first instance survives                  True pid=55316
WM_CLOSE closes main window              True closed=True; alive=False
hotkey released after exit               True selftest exit=0
hotkey registered in log                 True
second instance signalled                True
PASS: 9 checks
```

日志片段：

```
2026-11-18 15:45:35.751 INFO  QuickPhrase 0.1.0 starting (selftest=false, bench=false)
2026-11-18 15:45:35.761 INFO  tray icon added
2026-11-18 15:45:35.761 INFO  hotkey registered: Ctrl+Alt+Q
2026-11-18 15:45:35.761 INFO  ready in 10 ms (pid 52132)
2026-11-18 15:45:36.689 WARN  another instance is running; signalling it
2026-11-18 15:45:36.693 INFO  second instance activation requested
2026-11-18 15:45:38.144 INFO  message loop ended
```

DPI 运行期调用返回 `2147942405`（`0x80070005`，访问被拒），与“manifest 已先设置 PerMonitorV2”一致；`mt.exe` 导出内容确认 manifest 存在。

## 未覆盖 / 保留项

- 托盘图标为系统默认图标；自定义图标在 M1-9。
- 热键写死 `Ctrl+Alt+Q`；录制与冲突重设在 M1-5。
- 内存只测了启动 1 s 的样本，正式口径（静置 60 s）在 M0-5。
- 未做 100 次连续唤起压测（面板尚不存在，属 M0-3）。
- 托盘图标在 Explorer 重启后的复原逻辑已接线，但未做杀 Explorer 的实测。

## 结论

M0-1 通过，可进入 M0-2。M0-2 的实现契约（B6、M15、M16、M17、M20）已在 v4.1 冻结。
