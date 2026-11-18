# M1-1 存储验收记录（2026-11-18）

- 范围：`data.json` / `settings.json` / `rules-user.json` 的 schema、原子写、备份、恢复、只读模式、revision
- 依据：`docs/_ref/final-plan.md` §6.1、§6.2、§8 M1-1
- 复核：待独立复核

## 已实现

| 项 | 实现 |
| --- | --- |
| 文件 | `%LocalAppData%\QuickPhrase\{data,settings,rules-user}.json` + `backups\` |
| schema | `schemaVersion=1`，camelCase 字段与 §6.2 一致 |
| 数据模型 | Category / Snippet（含 pinned、sortOrder、useCount、lastUsedAt、lastInsertStatus、deletedAt、searchBlob） |
| revision | 每次 `save_data` 递增，写失败回滚计数 |
| 原子写 | 临时文件 → `sync_all` → 重读比对 → 轮转备份 → `ReplaceFileW`；失败回退 `MoveFileExW(REPLACE_EXISTING)` |
| 首写 | 目标不存在时 `MoveFileExW` + `MOVEFILE_WRITE_THROUGH` |
| 备份 | 写入前复制当前文件到 `backups\<name>-<UTC 时间戳>.json`，保留最新 5 份 |
| 恢复 | 主文件解析失败 → 按时间倒序尝试 5 份备份 → 成功即回写主文件 |
| 只读 | 全部副本损坏、schema 高/低于程序 → `read_only`，不写入，`notice` 说明原因 |
| 种子 | 首次启动写入 3 条短语 + 2 个分类，内容与 M0 硬编码一致 |
| 面板接线 | 面板列表改为读取 `storage::panel_items()`，不再有硬编码 `ITEMS` |
| 使用计数 | 注入 worker 结束后 `record_use(id, status)`，状态为 `accepted/partial/rejected` |
| 单元测试 | 9 个：种子、revision、record_use、备份裁剪、损坏恢复、全损坏只读、schema 更新、无临时文件、100 轮模拟故障 |

## 实测

```
storage: dir=C:\Users\Administrator\AppData\Local\QuickPhrase revision=1 snippets=3 read_only=false notice=None
```

- 首次启动生成 `data.json`（2342 B）、`settings.json`、`rules-user.json`。
- 面板冒烟 10/10（列表已来自存储）。
- 一次复制路径提交后 `revision=2`，首条 `useCount=1`、`lastInsertStatus=rejected`，证明计数与状态落盘。

## 已知偏差 / 未覆盖

- **100 次故障注入**为模拟损坏（截断主文件），非真实断电/磁盘满；真实故障注入待补。
- 迁移链为空：当前只有 v1，低于 v1 一律只读；没有真实迁移样本。
- 备份轮转按时间戳文件名排序，同一毫秒内多次写入可能同名覆盖（概率极低）。
- 未加跨进程文件锁；依赖单实例互斥，进程内由 `Mutex<Store>` 串行化。
- 设置与规则只落盘，未接 UI/规则求值（M1-5/M1-7）。
- `searchBlob` 已生成并持久化，检索打分在 M1-2。
- 面板只在启动时读取一次快照；M1-4 的 CRUD 需要重新加载。

## 结论

M1-1 的 schema、原子写、备份、恢复与只读模式具备单测与运行时证据；待独立复核后进入 M1-2。