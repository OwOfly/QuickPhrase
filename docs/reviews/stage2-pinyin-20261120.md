# 阶段二：拼音与首字母检索（2026-11-20）

- 基础：`fec8145`（快速新增）
- 范围：FR-12 标题拼音全拼、紧凑全拼与首字母检索
- 不变：数据 schema、注入、热键、托盘、面板排序

## 交付

| 项 | 实现 |
| --- | --- |
| 依赖 | `pinyin 0.11`，仅启用 `plain` 特性，MIT；离线编译，无运行时网络 |
| 索引 | `Snippet::rebuild_blob` 在标题/正文小写之外追加标题的完整拼音（空格分隔）、紧凑全拼、首字母；`search_blob` 为内存派生字段，不写入 `data.json`/导出 |
| 检索 | blob 命中但标题/正文无字面命中的 ASCII 查询按拼音命中处理，得分 90，低于正文 100；不产生标题/正文高亮 |
| 单字符 | 拼音回退要求查询至少 2 个 ASCII 字母，避免单字母命中大量首字母；含换行的查询禁用拼音回退 |
| 升级 | 启动加载时重建全部 `search_blob`；旧数据无需迁移，不持久化派生字段，导出体积不因拼音膨胀 |
| 文案 | 面板/管理搜索提示改为“搜索标题、正文或拼音…” |

## 复核（r1 → r2）

- 三家独立只读复核（glm / kimi / qwen）。
- r1：glm PASS；kimi CHANGES_REQUIRED，指出派生拼音写入 `searchBlob`/导出可能把合法库推过 20 MB 导入上限（major）；qwen PASS。另有首尾换行绕过拼音门控、bench 仍用旧两段 blob、缺字面 100 vs 拼音 90 测试、README 体积过期等 minor/nit。
- 修复：`search_blob` 改为不序列化的派生字段；导出/`data.json` 不再含拼音，加载/导入重建；首尾换行（含 CRLF）禁用拼音；bench 改用生产 5 段 blob 并加入拼音查询；补字面/拼音排序、无高亮、CRLF、字面换行测试；更新注释与 README 体积。
- r2：glm / kimi / qwen 全部 PASS；仅余注释/CRLF 测试 nit（已补），不阻塞。

## 验证

- `cargo fmt --check` PASS
- `cargo clippy --release` 0 警告
- `cargo test` 130/130（拼音键、`tk`/`tuikuan` 命中、单字符/换行不命中、字面 100 高于拼音 90、跨标题/正文换行不误命中、派生 blob 不导出）
- `--search-bench`（5 段 blob + 拼音查询）：n=1000，p50=314us，p95=550us，max=766us
- panel-smoke 10/10；tray-smoke 19/19
- `scripts/package.ps1` PASS：exe 1,011,200 bytes；zip 558,778 bytes；SHA256 `f88079f57074df85da98296cf65fe5b85adb481b3b8c8c3be0c539763264c551`

## 说明

- 拼音只索引标题，避免正文 10k 字符导致检索 blob 膨胀；正文拼音检索不支持。
- `search_blob` 使用 `#[serde(default, skip_serializing)]`：旧数据可加载并重建，新导出/`data.json` 不保存派生拼音，避免 20 MB 导入上限被膨胀突破。
- 拼音命中无高亮区间，列表仍显示原标题/正文。
- 拼音数据使 exe 增加约 230 KB。
- 查询含中文/换行时仍只按字面匹配，避免标题/正文边界误命中。

## 残留

- 正文拼音、多音字自定义、繁体/生僻字覆盖未做。
- 拼音命中无高亮；排序固定 90 分，未按全拼/首字母细分。
- `search_blob` 仅内存存在，10k 条加载时重建需要额外 CPU/内存；未做 10k 长标题实测。
- 新 `data.json`/导出不含 `searchBlob`，旧二进制（含拼音 r1 内部构建）无法直接读取；失败干净、不损坏数据，但不支持降级读取。该拼音版本未对外发布，接受此单向兼容。
- 真实输入法/跨应用检索未验证。