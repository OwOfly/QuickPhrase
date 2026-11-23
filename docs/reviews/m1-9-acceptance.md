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

## UI v2 打磨后重打包（2026-11-20）

Fluent/Direct2D 界面重做及复核修复后再次打包：

```
exe bytes=750080
zip bytes=460842
sha256=d05546afb418247f6e5a735f86511d23f044b8861ddca889191ba9449b5be821
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代此前所有 UI 迭代的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。

## 数据闭环后重打包（2026-11-20）

阶段一（导入/导出、备份恢复、回收站清理、暂停持久化、便携目录，见 `docs/reviews/stage1-data-20261120.md`）经 r1–r4 三家复核修复后进入发布包：

```
exe bytes=772608
zip bytes=473128
sha256=da4a6d9c87c153e46aaa24b42157719006f0553e41c993f22ac0c27bd967bef6
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代 UI v2 的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。`cargo test` 123/123，panel-smoke 10/10，tray-smoke 19/19；smoke.ps1 仅内存项为 UI v2 既有偏差（基线复测 40.44 MB，阈值 35 MB）。

## 阶段二排序增强后重打包（2026-11-20）

阶段二首个模块（面板最近使用 / 最常使用排序及管理窗口编辑框防御修复，见 `docs/reviews/stage2-sort-20261120.md`）经 r1/r2 三家复核后进入发布包：

```
exe bytes=780288
zip bytes=476304
sha256=c1a2544fb4744b529b915cf0741e6b64914821926b3925eebe1cc36ca93ed007
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代阶段一的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。`cargo test` 127/127，panel-smoke 10/10，tray-smoke 19/19。

## 阶段二快速新增后重打包（2026-11-20）

托盘“新增短语”入口（见 `docs/reviews/stage2-quickadd-20261120.md`）进入发布包：

```
exe bytes=780800
zip bytes=476473
sha256=cb46ec689e307fc4804c0e9438e7a408112a0080abc83a4b459d1200005c08f6
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代阶段二排序的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。`cargo test` 127/127，panel-smoke 10/10，tray-smoke 19/19。

## 阶段二拼音检索后重打包（2026-11-20）

标题拼音/首字母检索（见 `docs/reviews/stage2-pinyin-20261120.md`）进入发布包：

```
exe bytes=1011200
zip bytes=558778
sha256=f88079f57074df85da98296cf65fe5b85adb481b3b8c8c3be0c539763264c551
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代快速新增的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。`cargo test` 130/130，panel-smoke 10/10，tray-smoke 19/19。

## 阶段二变量模板后重打包（2026-11-20）

`{date}` / `{time}` / `{datetime}` 模板与面板预览（见 `docs/reviews/stage2-templates-20261120.md`）进入发布包：

```
exe bytes=1012224
zip bytes=559470
sha256=9dbb43f7cdce1ff2089d369523cf568631853d7b8018826f47da563ed7a9b4da
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代拼音检索的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。`cargo test` 133/133，panel-smoke 10/10，tray-smoke 19/19。

## 阶段二按应用输入例外后重打包（2026-11-20）

设置 →“应用输入例外”按 exe 覆盖 Unicode/粘贴模式（见 `docs/reviews/stage2-apps-20261120.md`）进入发布包：

```
exe bytes=1021952
zip bytes=563881
sha256=c5e7864afccf5b36abc1e35c84a184c0182b0a4e71eaa5ba78f0589b24db3045
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代变量模板的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。`cargo test` 134/134，panel-smoke 10/10，tray-smoke 19/19。

## 阶段三标签检索后重打包（2026-11-22）

短语标签与检索（见 `docs/reviews/stage3-tags-20261122.md`）进入发布包：

```
exe bytes=1028608
zip bytes=566684
sha256=ec36f3652e6f9c07055a7ad5493a7a58ea4c77e443b69709b3fc046c78aa1a3f
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代按应用输入例外的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。`cargo test` 136/136，panel-smoke 10/10，tray-smoke 19/19。

## 阶段三短语专属快捷键后重打包（2026-11-22）

短语专属快捷键（FR-15，见 `docs/reviews/stage3-hotkeys-20261122.md`）进入发布包：

```
exe bytes=1049088
zip bytes=578064
sha256=7f8c0f933b2d3d350a6b72a4c3d090d55632a874be6802c7f252d6b63c9ea516
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代标签检索的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。`cargo test` 144/144，panel-smoke 10/10，tray-smoke 19/19，host-matrix（隔离数据）10/10，snippet-hotkey-smoke 16/16。

## 阶段三标签筛选与批量标签后重打包（2026-11-22）

标签筛选与管理列表批量标签（见 `docs/reviews/stage3-tagfilter-batch-20261122.md`）进入发布包：

```
exe bytes=1064960
zip bytes=585869
sha256=28f3a5847cd53c0e6a82e02bf1017cd38a3a5e6cfec7aa0d450378331bba8511
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代短语专属快捷键的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。`cargo test` 147/147，panel-smoke 10/10，tray-smoke 19/19，host-matrix（隔离数据）10/10，snippet-hotkey-smoke 16/16，tag-batch-smoke 8/8。

## 阶段三标签云与全选后重打包（2026-11-23）

标签云、多标签 AND 与管理“全选”（见 `docs/reviews/stage3-tagcloud-20261123.md`）进入发布包：

```
exe bytes=1074688
zip bytes=590171
sha256=20a1ea3f2da94b20295f35cfa93fd8e0fa4de477a80368839174d4701267980d
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代标签筛选与批量标签的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。`cargo test` 150/150，panel-smoke 10/10，tray-smoke 19/19，tag-batch-smoke 10/10，host-matrix（隔离数据）10/10，snippet-hotkey-smoke 16/16。

## 列表卡片右边框修复后重打包（2026-11-23）

管理窗口“选中卡片右侧被白色车道刷掉”的修复（见 `docs/reviews/manage-card-right-edge-20261123.md`）进入发布包：

```
exe bytes=1075712
zip bytes=590329
sha256=4e89ba752a5d0ab5908af73f840443dc961db93d29e730371a15097420f8122a
fileVersion=0.1.0 product=QuickPhrase 快语
```

本哈希取代标签云与全选的 zip 哈希；`dist/SHA256SUMS.txt` 已同步。`cargo test` 150/150，`cargo fmt --check` / `cargo clippy --release` PASS，panel-smoke 10/10，tag-batch-smoke 10/10。glm / kimi / qwen 复核 PASS（kimi/qwen 提出的截图路径状态边界已修复并 focused 复验）。未创建标签，待人工桌面矩阵。

## 已知偏差 / 未覆盖

- 未做代码签名，Windows SmartScreen 可能提示未知发布者。
- 无安装器（MSI/Inno），采用 zip 便携分发；卸载为手动删除目录。
- 图标视觉效果仅在资源层验证，锁屏下未截图。
- 未在干净虚拟机做首次运行/杀软/公司策略验证。
- 版本仍为 0.1.0（V1 首发）；打标签命令由脚本打印，未自动 push。

## 结论

图标、版本资源、发布 zip、哈希与安装说明已落地。r1/r2 发现全部闭环，**r2 三家 PASS，M1-9 过门，M1 全部里程碑完成。**

残留：无代码签名、无安装器、干净环境与锁屏截图未验证。`v0.1.0` 标签命令由 `scripts/package.ps1` 打印，待人工执行。