# M1-6 托盘验收记录（2026-11-19）

- 范围：托盘菜单、暂停/恢复、第二实例、退出与资源释放
- 依据：`docs/_ref/prd-v1.1.md` §14.6、`docs/_ref/final-plan.md` §8 M1-6
- 复核：r1 glm/qwen PASS、kimi CHANGES_REQUIRED（F-01/F-02）→ r2 **三家 PASS**，S1–S11 全 CLOSED

## 已实现

| 项 | 实现 |
| --- | --- |
| 菜单 | 打开快捷面板 / 管理短语 / 暂停或恢复快捷键 / 设置 / 恢复上次剪贴板内容 / 关于 / 退出 |
| 双击托盘 | 打开管理窗口 |
| 暂停/恢复 | 注销/重新注册热键；托盘提示文案切换；状态写入内存（不落盘） |
| 第二实例 | 命名互斥 + `RegisterWindowMessage("QuickPhrase.Activate")`；第二实例发消息后退出 0，第一实例打开面板 |
| 退出 | 注销热键、移除托盘图标、销毁面板/管理/设置窗口并释放 GDI；退出码 0 |
| 管理窗口关闭 | 仅隐藏，进程与托盘继续 |

## 实测（`scripts/tray-smoke.ps1`，19/19）

```
app running                          True
main window exists                   True
tray icon added                      True
hotkey registered                    True
hotkey is held while running         True
pause releases the hotkey            True
resume takes the hotkey again        True
pause hotkey                         True
resume hotkey                        True
second instance exits                True exit=0
second instance exits 0              True exit=0
second instance activation           True
second instance opens the panel      True
second instance (panel open) exits 0 True exit=0
second instance keeps an open panel  True
panel closes on WM_CLOSE             True
app exits 0                          True exit=0
hotkey released on exit              True
message loop ended                   True
```

热键相关四项目为真实 `RegisterHotKey` 探测，不是日志匹配。第二实例的「打开面板 / 已打开不关闭」由 `IsWindowVisible` 断言。

## 复核修复（r1）

| 发现 | 处置 |
| --- | --- |
| 第二实例与窗口创建竞态、失败静默 | `signal_existing` 有界重试（≤2 s），返回投递结果；失败弹窗提示 |
| 第二实例和托盘「打开快捷面板」复用切换逻辑，面板已开时反而关闭 | 拆成 `hotkey_panel`（切换）与 `open_panel`（幂等打开/置前） |
| 暂停/恢复与热键释放只匹配日志 | 改为真实热键占用探测；新增面板可见性断言 |
| 启动热键注册失败仍显示「暂停快捷键」 | 失败态菜单显示「重试注册快捷键」 |
| `TaskbarCreated` 重加托盘失败无日志 | 补 error 日志 |
| 窗口类激活消息字符串重复 | `app.rs` 复用 `single_instance::activation_message()` |
| 退出时可能有在途注入 | 注入中拒绝退出并提示稍候 |
| 冒烟按行号偏移、`-Wait` 无超时 | 改按字节偏移读日志（共享读写打开），第二实例带 10 s 超时 |
| 恢复剪贴板失败文案归因错误 | 改为「剪贴板内容已变化（可能已恢复过，或被其他程序修改）」 |

## 已知偏差 / 未覆盖

- 托盘气泡失败无降级反馈（`Shell_NotifyIconW` 返回值未消费）。
- 暂停状态不持久化，重启后恢复为启用；`settings.json` 的 `paused` 字段为预留，运行时未写入。
- 托盘图标为系统默认应用图标，正式图标属 M1-9。
- 真实鼠标右键菜单交互未自动化，脚本用 `WM_COMMAND` 驱动。
- 冒烟未覆盖剪贴板恢复入口（无快照/序列变化），该路径由设置窗口同语义代码与单测覆盖。

## 结论

托盘菜单、暂停/恢复、第二实例与退出释放已接入并通过自动冒烟（19/19，含真实热键与面板可见性探测）。r1/r2 发现全部闭环，**三家 r2 PASS，M1-6 过门，M1-7 可开。**

残留：暂停不持久化；托盘气泡失败无降级；默认图标（M1-9）；真实鼠标菜单未自动化。`open_panel` 的置前受前台锁影响时仍可见但不保证聚焦（已记 qwen minor）。