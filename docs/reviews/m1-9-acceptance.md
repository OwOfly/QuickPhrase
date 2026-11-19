# M1-9 打包验收记录（2026-11-19）

- 范围：应用图标、版本资源、发布 zip、SHA256、安装/卸载说明、许可证
- 依据：`docs/_ref/final-plan.md` §8 M1-9、PRD §14.8
- 复核：r1 glm PASS、kimi/qwen INCOMPLETE（仅因哈希工具缺失）→ r2 **三家 PASS**，T1–T8 全 CLOSED

## 已实现

| 项 | 实现 |
| --- | --- |
| 图标 | `scripts/make-icon.ps1` 用 GDI+ 生成 16/24/32/48/64/128/256 多尺寸 `assets/quickphrase.ico`（紫底白色气泡 + 三行文本） |
| 图标接线 | `app.rc` 资源 `1 ICON`；主窗口与托盘用 `LoadIconW(instance, 1)`，缺失时回退系统图标 |
| 版本资源 | `1 VERSIONINFO`：文件/产品版本 0.1.0、产品名「QuickPhrase 快语」、公司、原始文件名 `QuickPhrase.exe`、MIT |
| 打包脚本 | `scripts/package.ps1`：`cargo metadata` 读版本 → 构建 → 资源版本与 Cargo 版本不一致即失败 → 清空并校验暂存目录 → 仅允许 EXE/README/LICENSE → zip → 校验 zip 内容 → ≤15 MB（超限删除产物）→ SHA256SUMS |
| 许可 | 新增 MIT `LICENSE`；Cargo 元数据一致 |
| README | 构建、打包、安装/卸载、开机启动清理、数据目录、当前限制 |
| 发布物 | `dist/QuickPhrase-0.1.0-win-x64.zip` + `dist/SHA256SUMS.txt` |

## 实测

```
exe bytes=498176
zip bytes=261769
sha256=80b3d9ee29148ab34535ce7ffd0fe43abb0ed9c4f47af321e3426b91de04d887
fileVersion=0.1.0 product=QuickPhrase 快语
zip contents: LICENSE, QuickPhrase.exe, README.md
```

- Windows 资源检查：RT_ICON 资源 ID 1 存在；RT_VERSION 资源 ID 1 存在。
- ICO 结构：7 个条目（16/24/32/48/64/128/256），目录偏移连续、无尾随字节，全部为 PNG 数据；见 `icon-structure.txt`。
- 打包体积 255 KB，远低于 15 MB 预算。

## 复核修复（r1）

| 发现 | 处置 |
| --- | --- |
| 发布版 About 仍带内部里程碑标签 | 去掉里程碑字样，仅保留产品名与版本 |
| 体积校验在写 SHA256 之后 | 先校验，超限删除 zip 再报错 |
| 资源版本未与 Cargo 版本比对 | 打包前断言 FileVersion/ProductVersion |
| 版本解析依赖 Cargo.toml 首个 version | 改用 `cargo metadata` |
| 暂存目录清理失败被忽略、可能带入残留文件 | 清理失败即中止；暂存与 zip 内容都断言仅三文件 |
| 文件末尾缺换行 | 补尾换行 |
| ICO 结构无产物层证据 | 生成 `icon-structure.txt`（目录项、偏移、PNG 标记） |

## UI 打磨后重打包（2026-11-19）

UI 深度打磨（`4979c06`、`ef20a0a`、`e55f45a`，见 `docs/reviews/ui-polish-20261119.md`）只改视觉与布局，但改变了二进制，故重新打包：

```
exe bytes=517632
zip bytes=272369
sha256=8e46db7cf657b5dec3e958c190b5f464bf3d2999cabe34b9344c2309103f4a40
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希**取代**上方初次打包哈希；`dist/SHA256SUMS.txt` 已同步。打包脚本的资源版本、暂存目录与 zip 内容断言全部通过，仍未自动创建或推送 `v0.1.0` 标签。

## 已知偏差 / 未覆盖

- 未做代码签名，Windows SmartScreen 可能提示未知发布者。
- 无安装器（MSI/Inno），采用 zip 便携分发；卸载为手动删除目录。
- 图标视觉效果仅在资源层验证，锁屏下未截图。
- 未在干净虚拟机做首次运行/杀软/公司策略验证。
- 版本仍为 0.1.0（V1 首发）；打标签命令由脚本打印，未自动 push。

## 结论

图标、版本资源、发布 zip、哈希与安装说明已落地。r1/r2 发现全部闭环，**r2 三家 PASS，M1-9 过门，M1 全部里程碑完成。**

残留：无代码签名、无安装器、干净环境与锁屏截图未验证。`v0.1.0` 标签命令由 `scripts/package.ps1` 打印，待人工执行。