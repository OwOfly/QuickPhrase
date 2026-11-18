# QuickPhrase v3 focused review brief

- 轮次：v3 定向复核（focused）
- 复核对象：`D:\Code\AIProject\quickphrase\docs\_ref\final-plan.md`（final-plan v3）
- 待闭环清单：`D:\Code\AIProject\quickphrase\docs\_ref\reviews\2026-11-18-findings-register.md`
- 背景规格：`D:\Code\AIProject\quickphrase\docs\_ref\prd-v1.1.md`
- 快照哈希：`final-plan.md` = `7ba2f41db6ef2e30a529f559c97bc638a93041a8d9e15611bf53ec3833e50bc8`
- 清单哈希：`2026-11-18-findings-register.md` = `1207fae74925e7dfc5348242c85dab54ca266329579504526586b0dc5a749e3c`
- 三家必须基于以上两个哈希的同一份文件复核；哈希不符即停止。

## 任务

这不是一轮全新方案评审，而是**闭环核验**。

1. 对清单中的每一项（B1–B5、M1–M14），在 v3 中找到对应条款，判定 `CLOSED / PARTIAL / OPEN`，并给出证据：章节号 + 引用原文（≤2 行）。
2. 指出 v3 内部**自相矛盾**的条款（例如某处允许、另一处禁止）。
3. 只报**新引入**的 blocker / major：v3 为了解决旧问题而制造的新风险。不要重新争论已经拍板的方向（Rust、原生控件、终端多行只复制），除非发现事实错误。
4. 对每条新问题给出：触发条件、影响、证据（章节/原文）、必改要求。

## 判定规则

- `CLOSED` 必须同时满足：有可执行条款、有失败退路、有验收方式。
- 只写了原则、没写判据 → `PARTIAL`。
- 两个条款互相冲突 → 相关项全部 `PARTIAL`，并单独列入矛盾清单。
- 不得用“看起来考虑到了”作为 CLOSED 依据。

## 输出格式（严格）

```
## 闭环表
| ID | 状态 | 证据（章节 + 原文） | 缺口 |
| --- | --- | --- | --- |
...

## 内部矛盾
- [章节A] 与 [章节B]：...

## 新问题
### N1（blocker|major）
- 触发条件：
- 影响：
- 证据：
- 必改：

## 结论
PASS | CHANGES_REQUIRED | INCOMPLETE
一句话理由。
```

## 约束

- 只读。禁止修改文件、禁止执行 shell、禁止 git 操作。
- 所有结论必须有 v3 原文支撑；找不到就写 `OPEN`，不要猜。
- 不超过 2,500 中文字。
