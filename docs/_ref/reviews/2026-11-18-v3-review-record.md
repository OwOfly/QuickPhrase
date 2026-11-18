# QuickPhrase v3 focused review 记录（2026-11-18）

## 快照

| 文件 | SHA256 |
| --- | --- |
| `final-plan.md`（v3） | `7ba2f41db6ef2e30a529f559c97bc638a93041a8d9e15611bf53ec3833e50bc8` |
| `2026-11-18-findings-register.md` | `1207fae74925e7dfc5348242c85dab54ca266329579504526586b0dc5a749e3c` |

三家均核对哈希通过后才开始复核。

## 各家结论

| 模型 | 结论 | CLOSED | PARTIAL/OPEN | 新问题 |
| --- | --- | ---: | ---: | --- |
| glm-5.3#max | CHANGES_REQUIRED | 15 | 4（B1、M2、M5、M11） | N1 major（Document 身份可跨 A→B） |
| kimi-k3.1-pro#max | CHANGES_REQUIRED | 11 | 8（B1、B3、B5、M2、M3、M5、M10、M11） | N1 blocker（用户规则越权）、N2 blocker（浏览器 LF 绕过）、N3 major（Document 冲突） |
| qwen4-max#max | CHANGES_REQUIRED | 12 | 7（B5、M2、M5、M7、M9、M10、M11） | N1 blocker（用户规则越权）、N2 major（Excel 散格） |

## 根因归并

| 根因 | 命中 | 处置 |
| --- | --- | --- |
| R1 Document 级元素一允一拒，且身份跨 A→B 仍成立 | glm N1、kimi N3、qwen M11、主席 C1 | v4 统一为只复制 |
| R2 `rules-user.json` 可按 exe 把区域放宽为 `TEXT_EDITOR` | qwen N1、kimi N1、主席 C3 | v4 用户规则只能收紧；删除示例；硬规则清单补齐 |
| R3 浏览器 LF 通道打开后可能命中网页终端/发送区域 | kimi N2、glm 矛盾项、主席 C2 | v4 V1 不做浏览器多行；E1 推迟 M2，逐宿主 × 控件类型 |
| R4 验收缺口：横切未枚举、M5 警示/恢复、只读控件、注入中销毁、恢复受限 | 三家 + 主席 | v4 补横切 6 项、T13–T17、只读检查、§6.5 |
| R5 Excel 网格被当 Word 放行多行 CR | qwen N2 | v4 Excel 单列，多行只复制 |
| R6 M1 表把缓冲行混入模块合计 | qwen M9 | v4 缓冲移出表；22 + 4 分列 |

## 分歧裁决

- glm 判 B5 CLOSED，qwen/kimi 判 PARTIAL 并给出可复现配置路径（`rules-user` 示例）。以代码/配置路径为准，**B5 为 PARTIAL**。
- glm 与 qwen 对 M3 判断不同（CLOSED vs CLOSED，kimi PARTIAL 因缺验收用例）。采纳 kimi：补 T17。
- Document 处置：glm 给“二选一”，qwen/kimi 要求统一。选择“只复制”，理由是 `CompareElements` 在 Document 级无法区分页内 A/B，保留单行等于留下 B1 的后门。

## v4 变更

见 `docs/_ref/final-plan.md` §0 对照表。核心：Document/ValuePattern-only/只读 → 只复制；用户规则只收紧；浏览器多行 V1 不做；Excel 只复制；横切与反例补齐；工期 36 人日不变。

## 未闭环 / 待用户

- 三项产品口径待确认：浏览器多行、Excel 多行、VS Code 多行。
- v4 尚未复核；建议只做“6 个根因”抽查，不再全量三模型。
