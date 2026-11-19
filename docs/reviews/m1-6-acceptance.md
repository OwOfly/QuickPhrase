# M1-6 托盘验收记录（2026-11-19）

- 范围：托盘菜单、暂停/恢复、第二实例、退出与资源释放
- 依据：`docs/_ref/prd-v1.1.md` §14.6、`docs/_ref/final-plan.md` §8 M1-6
- 复核：待独立复核

## 已实现

| 项 | 实现 |
| --- | --- |
| 菜单 | 打开快捷面板 / 管理短语 / 暂停或恢复快捷键 / 设置 / 恢复上次剪贴板内容 / 关于 / 退出 |
| 双击托盘 | 打开管理窗口 |
| 暂停/恢复 | 注销/重新注册热键；托盘提示文案切换；状态写入内存（不落盘） |
| 第二实例 | 命名互斥 + `RegisterWindowMessage("QuickPhrase.Activate")`；第二实例发消息后退出 0，第一实例打开面板 |
| 退出 | 注销热键、移除托盘图标、销毁面板/管理/设置窗口并释放 GDI；退出码 0 |
| 管理窗口关闭 | 仅隐藏，进程与托盘继续 |

## 实测（`scripts/tray-smoke.ps1`，10/10）

```
app running                True
main window exists         True
tray icon added            True
hotkey registered          True
pause hotkey               True
resume hotkey              True
second instance exits 0    True exit=0
second instance activation True
app exits 0                True exit=0
hotkey released            True
```

## 已知偏差 / 未覆盖

- 托盘气泡失败无降级反馈（`Shell_NotifyIconW` 返回值未消费）。
- 暂停状态不持久化，重启后恢复为启用。
- 托盘图标为系统默认应用图标，正式图标属 M1-9。
- 真实鼠标右键菜单交互未自动化，脚本用 `WM_COMMAND` 驱动。

## 结论

托盘菜单、暂停/恢复、第二实例与退出释放已接入并通过自动冒烟；待复核后进入 M1-7 注入加固。