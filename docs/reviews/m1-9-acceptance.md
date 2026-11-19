# M1-9 打包验收记录（2026-11-19）

- 范围：应用图标、版本资源、发布 zip、SHA256、安装/卸载说明、许可证
- 依据：`docs/_ref/final-plan.md` §8 M1-9、PRD §15
- 复核：待独立复核

## 已实现

| 项 | 实现 |
| --- | --- |
| 图标 | `scripts/make-icon.ps1` 用 GDI+ 生成 16/24/32/48/64/128/256 多尺寸 `assets/quickphrase.ico`（紫底白色气泡 + 三行文本） |
| 图标接线 | `app.rc` 资源 `1 ICON`；主窗口与托盘用 `LoadIconW(instance, 1)`，缺失时回退系统图标 |
| 版本资源 | `1 VERSIONINFO`：文件/产品版本 0.1.0、产品名「QuickPhrase 快语」、公司、原始文件名 `QuickPhrase.exe`、MIT |
| 打包脚本 | `scripts/package.ps1`：读 Cargo 版本 → 构建 → 暂存 EXE/README/LICENSE → zip → SHA256SUMS → 校验 ≤15 MB |
| 许可 | 新增 MIT `LICENSE`；Cargo 元数据一致 |
| README | 构建、打包、安装/卸载、开机启动清理、数据目录、当前限制 |
| 发布物 | `dist/QuickPhrase-0.1.0-win-x64.zip` + `dist/SHA256SUMS.txt` |

## 实测

```
exe bytes=498176
zip bytes=261800
sha256=d45f4b246dfd263f8acbfcf7386db3d5ec8e7ab9437a246f87f67f085b43c9cc
fileVersion=0.1.0 product=QuickPhrase 快语
zip contents: LICENSE, QuickPhrase.exe, README.md
```

- Windows 资源检查：RT_ICON 资源 ID 1 存在；RT_VERSION 资源 ID 1 存在。
- 打包体积 255 KB，远低于 15 MB 预算。

## 已知偏差 / 未覆盖

- 未做代码签名，Windows SmartScreen 可能提示未知发布者。
- 无安装器（MSI/Inno），采用 zip 便携分发；卸载为手动删除目录。
- 图标视觉效果仅在资源层验证，锁屏下未截图。
- 未在干净虚拟机做首次运行/杀软/公司策略验证。
- 版本仍为 0.1.0（V1 首发）；打标签命令由脚本打印，未自动 push。

## 结论

图标、版本资源、发布 zip、哈希与安装说明已落地；待独立复核后结束 M1。