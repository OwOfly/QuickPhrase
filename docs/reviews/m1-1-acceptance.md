# M1-1 存储验收记录（2026-11-18）

- 范围：`data.json` / `settings.json` / `rules-user.json` 的 schema、原子写、备份、恢复、只读模式、revision
- 依据：`docs/_ref/final-plan.md` §6.1、§6.2、§8 M1-1
- 复核：r1 三家 CHANGES_REQUIRED → r2 **三家 PASS** → r3 增量 **glm PASS、kimi PASS（S5 后续修复）、qwen PASS**

## 已实现

| 项 | 实现 |
| --- | --- |
| 文件 | `%LocalAppData%\QuickPhrase\{data,settings,rules-user}.json` + `revision.hwm` + `backups\` |
| schema | `schemaVersion=1`，camelCase 字段与 §6.2 一致 |
| 数据模型 | Category / Snippet（含 pinned、sortOrder、useCount、lastUsedAt、lastInsertStatus、deletedAt、searchBlob） |
| revision | 每次 `save_data` 递增，写失败回滚计数；`revision.hwm` 预留高水位，恢复后不复用旧号 |
| 原子写 | 临时文件 → `sync_all` → 重读比对 → 轮转备份 → `ReplaceFileW`；失败回退 `MoveFileExW(REPLACE_EXISTING)` |
| 首写 | 目标不存在时 `MoveFileExW` + `MOVEFILE_WRITE_THROUGH` |
| 备份 | 写入前复制当前文件到 `backups\<name>-<UTC 时间戳>-<序号>.json`，保留最新 5 份 |
| 恢复 | 主文件解析失败 → 按时间倒序尝试 5 份备份 → 成功即回写主文件 |
| 只读 | 全部副本损坏、schema 高/低于程序 → `read_only`，不写入，`notice` 说明原因 |
| 种子 | 首次启动写入 3 条短语 + 2 个分类，内容与 M0 硬编码一致 |
| 面板接线 | 面板列表改为读取 `storage::panel_items()`，不再有硬编码 `ITEMS` |
| 使用计数 | 注入 worker 结束后 `record_use(id, status)`；`useCount`/`lastUsedAt` 只在 `accepted` 时更新，`lastInsertStatus` 记录全部三态 |
| 单元测试 | 22 个：种子、revision、record_use（含 rejected 不计数）、备份裁剪、损坏恢复、主文件缺失恢复、恢复不污染备份、全损坏只读、缺失+备份全坏只读、settings 缺失+备份坏只读、schema 新旧、只读拒绝写入、写失败不泄漏 revision（含 HWM 领先）、无临时文件、revision 不复用、未来 schema 不被覆盖、100 轮模拟故障 |

## 实测

```
storage: dir=C:\Users\Administrator\AppData\Local\QuickPhrase revision=1 snippets=3 read_only=false notice=None
```

- 首次启动生成 `data.json`（2342 B）、`settings.json`、`rules-user.json`。
- 面板冒烟 10/10（列表已来自存储）。
- 一次复制路径提交后 `revision=2`，首条 `lastInsertStatus=rejected`；`useCount` 不变（§7 口径，单测覆盖）。

## 复核修复（r1）

| 发现 | 处置 |
| --- | --- |
| 备份裁剪删除失败会无限自旋 | 改为有界循环，删除失败即 `break` 并记 warn；只枚举一次 |
| 恢复回写把损坏主文件轮转进备份 | 恢复路径 `save_data_with(false)`，不轮转 |
| 主文件缺失不查备份直接 seed | 缺失且存在备份时先恢复最新有效备份 |
| settings/rules 版本不校验、恢复不回写 | 两者版本高于程序即只读；恢复后回写主文件 |
| `useCount` 对 rejected 也 +1 | 仅 `accepted` 递增；三态都记 `lastInsertStatus`；保存失败回滚内存字段 |
| 备份同毫秒同名覆盖 | 文件名追加单调序号 |
| 备份复制崩在半途 | 先写 `.tmp` 再改名 |
| 恢复后 revision 与已丢版本重号 | 新增 `revision.hwm` 高水位：写前预留，恢复取 max+1 |
| 未来 schema 被恢复流程覆盖 | 解析前先 peek 版本字段，高版本直接只读，不走恢复 |
| settings/rules 全坏静默重置 | 置 `notice` 并进入只读 |
| 首写/设置/规则写失败无提示 | 置 `initial_save_failed` / `settings_write_failed` / `rules_write_failed` |
| `ReplaceFileW` 失败后的回退 | 回退前检查目标仍存在，否则不降级 |
| 主文件缺失且备份全坏仍 seed | 有备份但全部不可用 → 只读，不覆盖 |
| 备份复制未刷盘 | 备份 `.tmp` 改名前后 `sync_all` |
| 写失败回滚用了 `next_revision - 1` | 改为恢复调用前的 revision，HWM 领先时不泄漏递增 |
| 备份测试 `<= 5` 恒真 | 收紧为 `== 5` |
| 陈旧 `.tmp` 残留 | 启动时清理 |

## 已知偏差 / 未覆盖

- **100 次故障注入**为模拟损坏（截断主文件），非真实断电/磁盘满；真实故障注入待补。
- 迁移链为空：当前只有 v1，低于 v1 一律只读；没有真实迁移样本。
- 未加跨进程文件锁；依赖单实例互斥，进程内由 `Mutex<Store>` 串行化。
- 设置与规则只落盘，未接 UI/规则求值（M1-5/M1-7）。
- `searchBlob` 已生成并持久化，检索打分在 M1-2。
- 面板每次热键激活都重新读取 `storage::panel_items()`（不是只在启动时读一次）。

## 结论

M1-1 的 schema、原子写、备份、恢复与只读模式具备单测与运行时证据。r3 增量复核三家无阻塞项（kimi 的 S5 只读句柄问题已由 `2dd7dea` 修复）。**M1-1 过门，M1-2 可开。**

残留（不阻塞）：100 次故障注入为模拟截断；迁移链为空；无跨进程文件锁；设置/规则未接 UI 与规则求值；检索打分在 M1-2。