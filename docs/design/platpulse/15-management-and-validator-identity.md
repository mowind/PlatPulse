本文件属于 [PlatPulse 产品与技术设计](../platpulse.md)：§15–§15.5。章节保留原编号，引用写作 `design §X.Y`。

<a id="accepted-management-target"></a>

## 15. Agent/Node 管理、Validator 自动识别与提示确认（已确认，部分实现）

### 15.1 状态、范围与依据

本节来自已完成的 `/grill-with-docs` 访谈（Q1–Q22）及 Owner 的最终共识确认，是目标设计。后续实现规格按仓库约定进入 GitHub Issues，本节不代替具体接口设计或实现工单。实现状态：§15.2 第 1、2 项（Agent 接入引导、显示名称/备注）已由 issue #169 交付；§15.3 Node Purge 已单独交付；§15.2 第 3–5 项 Agent Removal 已由 issue #171 交付（含所属 Node 权威列表、未处理 Transfer 阻止、全凭证撤销、子 Node 级联清理与删除身份边界）；§15.6 Agent Attention Acknowledgment 已由 issue #172 交付（Server-owned occurrence/evidence 边界、共享持久确认、Overview 与 Agent Detail 的逐条与批量操作、失败可重试与审计）；§15.4 自动 Validator 身份与 Current Validator Status 已由 issue #173 交付（Server 从已校验 Network 与观测到的完整 P2P 公钥自动建立 Node Validator Link、换键关闭旧区间、Public/Node 视图消费 Server 投影，不再展示手工 role）。手工注册/绑定/角色写入端点已退役并返回明确的 GONE 状态。一次性 Validator 模型迁移（§15.5）已由 issue #174 交付：Server 启动迁移在同一 SQLite 事务内识别无 automatic Link 的旧手工一代，删除其 Link、current insight、ranking/counter history、daily/monthly 聚合与 Validator 身份本身，并写入 validator_model_migration 一次性标记；automatic 一代身份与数据保留，旧分类/变化基线同步失效，Node 监控历史、既有 Incident 与必要审计保留；§15.7 删除主体的通知取消与 Incident 标注已由 issue #175 交付（被删 Node/Agent/Host 退出当前评估并标注 subject_deleted_at 而不伪造恢复，未发送通知——含无 incident_id 的恢复事件——被取消，已发送事实保留，其他主体与共享 Validator 通知策略不受影响）；§15.7 的 Incident 列表、详情与持久确认已由 issue #203 交付（Owner 仅可对一次 occurrence 做共享、不可撤回、不可覆盖的确认，记录确认者与时间且不改健康/恢复/通知策略，恢复后复发为新的未确认 occurrence，主体删除与恢复相互区分；列表支持精确 `subject_key` 过滤，Node/Agent 详情提供跳转到该 subject 的 Incident 历史快捷入口，父 issue #202 Story 2）。

- Agent：Owner 接入引导、显示名称/备注修改、Agent Removal。
- Node：保留已有重命名，只扩展 Owner 显式 Node Purge；不提供 Admin 新建 Node 或远端采集配置编辑。
- Validator：自动身份对应替代手工 Link/角色；独立 Validator 数据和统计继续存在。
- 提示：Agent Attention Acknowledgment，而非 Agent 进程发起业务告警清除。
- 不变：无远程控制、Network Registry 信任边界、每 Node 观测隔离、last-good、不可变 Agent Report 与事务性 Receipt、Owner-only mutation、Public/Admin 分离和移动端可用性。

长期取舍见 [ADR 0004](../../adr/0004-owner-removal-and-node-purge.md) 与 [ADR 0005](../../adr/0005-automatic-validator-identity.md)。交互见 [WebUI §15](../webui/15-agent-node-management.md#accepted-management-ui-target)；Validator 适配与指标分别见 [Provider 设计](../validator-provider.md)、[指标设计](../validator-metrics.md)。

### 15.2 Agent 接入、元数据与移除

1. Admin 新增入口生成一次性 Enrollment Token 和接入指引。仅生成 Token 不创建离线 Agent 占位记录；Agent 成功 Enrollment 后才出现在列表。Token 仍是短期单次接入凭据，不是永久 Agent Credential。
2. Owner 可以编辑 Agent 显示名称和备注。Agent ID、实际 Host 信息、Server 推导的 liveness、Epoch、采集配置不是可编辑资料；没有名称时仍可用稳定 ID 标识。显示名称与备注由 Server 持久化至 `agents` 表并写入 Audit（issue #169）。
3. Agent Removal 的确认必须明确列出所属 Node 及不可逆后果。执行前重新校验所属关系；存在进行中的 Node Transfer 时先完成、取消或处理该 Transfer，不能静默夺走 Node。
4. 确认后撤销该 Agent 的全部凭证，将其从当前监控/正常列表移除，并对所属 Node 执行 §15.3 的永久清理。Agent 登记身份仅保留必要删除标记；既有 Alert Incident 证据和必要审计按 §15.7 保留。不能用 Recovery/Rotation 或迟到报告绕过移除重新启用同一身份。
5. 移除不会停止、卸载远端进程。UI 告知 Owner 仍须在 Host 上处理本地配置/进程；凭证撤销与 Agent Removal 不是同一个动作。

### 15.3 Node Purge 与接收边界

- “无效”是 Owner 的显式处置决定，不是新的健康状态。允许选择 Active、Retired、离线、长期失败或仍在本地 Inventory 中的 Node；不根据离线时间自动删除。
- Node Purge 永久删除该 Node 的当前观测、监控历史及 Node Validator Links，并从 Home、当前列表/统计、Attention 和实时告警评估中移除。它不是 Retired、隐藏开关或可恢复软删除。
- 清理范围包括该 Node 的 Block Summary、计数/高水位/coverage/gap 等监控状态，以及 Peer、metric 和其他 Node 观测历史；必要审计、删除身份及既有 Alert Incident 证据保留。不能误删共享 Agent/Host、Network 或其他 Node 的数据。
- 普通 Node Purge 不删除独立 Validator 历史，哪怕它是最后一个关联 Node；一次性旧 Validator 数据清理是 §15.5 的独立迁移，不是日常级联规则。
- 持久化最小删除标记，永久禁止同一 Node ID 自动重建。后续合法报告即使继续声明它，也不能重新写入该 Node 的投影、关联或历史；要重新监控部署，须在 Agent 本地生成新 Node ID，本次不提供恢复入口。
- 删除标记是 Server 接收权限边界，不是远端修改 Inventory。整体 Inventory 结构/归属校验依旧成立；对已清理 Node 的接收处置不得阻断同一份合法报告中其他有效 Node。实现需明确对应 Inventory、per-Node/per-sample Receipt dispositions，不能改写不可变报告或把部分接收偷换成整份成功。
- 删除与 ingestion、Transfer、Provider/聚合后台任务的并发需要统一校验：最终清理完成后，迟到写入不可重建数据。仍通过鉴权的重复报告按既有幂等 Receipt 返回，不重复应用已删除数据；不能为删除操作修改旧 Receipt 内容或删除去重边界。Agent 移除后的无效凭证仍直接拒绝。
- mutation 必须由 Server 做权限与影响范围校验、记录审计；不能在实际清理完成前报告最终成功，也不能依靠客户端从列表移除来宣称清理完成。

### 15.4 自动 Validator 身份与当前状态

- Server 从有效 Node 观测中提取并校验完整 P2P 公钥，与已校验 Network 对应的 PlatScan 匹配。PlatPulse Node UUID、短 fingerprint、显示名、IP、当前共识成员标志都不是替代查找键；不根据 Agent 声称的任意 URL 访问 Provider。
- 自动 Link 表达同一链上身份，不表达 Owner 所有权或 primary/standby/observer 角色。取消手工绑定及角色入口/写入路径，不保留手工兜底。
- Node 的链上密钥变化时结束旧 Link 区间，重新识别新身份。Node 监控历史保留；不同 Validator 的累计奖励和出块不能拼接。相同 Validator 可关联多个 Node，当前选择汇总按 Network 内 Validator 去重。
- 当前有效质押身份与当前共识参与能力、Node Health 分开。状态判定原则如下：

| 证据 | Current Validator Status | 其他展示 |
|---|---|---|
| 当前有效的候选、活跃或正在出块身份 | 是 | 保留独立活动/共识状态 |
| 锁定或退出中，且能确认质押身份仍有效 | 是 | 明示锁定/退出中，不能显示为正常运行 |
| 已完成退出，或可信地确认无当前质押身份 | 否 | 旧记录可属于历史身份，不代表当前有效 |
| 候选身份正处当前共识周期（PlatScan status 6，Activity `verifying`），质押在册 | 是 | 共识参与与身份有效性分开；不因入选/落选改变结论 |
| 信息不足/冲突，不能确认身份有效 | 未知 | 不推断为否 |
| Provider 失败或信息过期 | 不产生新的肯定/否定结论 | 保留 last-good 并标 Stale；从未成功则 Unknown |

缺少可信公钥、Network Identity 不匹配或 Network 未配置可用 Provider 时，不能猜测关联，也不能拿其他 Network 或旧手工关联代替。本次业务原则已确定；具体 PlatScan 字段/状态如何证明“当前有效”，尤其 locked/exiting 和权威否定的条件，仍需主源证据验证，见 Provider 设计。普通 HTTP 失败不是无质押的证据。2026-10-08 裁定（Owner 决定）：PlatScan status 6 的源码语义是当前共识周期中的在册候选人（`getCodeByStatus(CANDIDATE, isConsensus = 1, *)`），既有定义把它当作 “verification in progress” 判为未知，使正在参与共识的身份反而比空闲候选人更不确定（与“当前有效质押身份，而非当前共识成员”的意图相反）；因此改判为「是」，并同步更新 [CONTEXT.md](../../../CONTEXT.md)、[ADR 0005](../../adr/0005-automatic-validator-identity.md) 与 [Provider 设计](../validator-provider.md)。

实现状态：§15.4 的自动身份识别、当前状态投影与 Admin 呈现已由 issue #218 交付（见 WebUI §15.20）。Server 侧由 `validator/` 承担唯一投影（`project_activity`、`automatic_identity_reason`、`last_good_age_seconds`、`list_node_validator_identities`），Public 与 Admin 因此共用同一套判定，不会各自漂移；Admin 新增 `GET /api/admin/v1/validator-identities`、`/admin/validators`、`/admin/validators/:validatorId`，并在 Node 详情内嵌只含识别状态与该身份的公开关联是否投影的 `validator_identity`（不含所有权或角色）。Provider 未配置或不可读时一律为 `unknown`/`Not configured`/`Unknown` 并保留 stale last-good，绝不产生 `not_validator` 或新鲜的 0；缺键、Network Identity 不匹配时只给出未知状态与理由。链上密钥变化结束旧 Link 区间并按边界新开区间，而「无证据」状态（缺公钥、缺 Network Identity）不结束区间；Node Purge 只清该 Node 的识别状态与 Link：被 Purged Node 自己的区间随该 Node 删除而不可重建（读回为无关联），`linkCount` 相应减少，仅当被删 Node 是该身份唯一关联时才为 0；共享 Validator 身份、其保留证据以及其他 Node 的关联历史（含已结束区间）保留。2026-10-06 的 #202 修复将确认结论与详情指标分开持久化：`last_good_verdict_outcome`（`success`/`empty`）及 `last_good_verdict_received_at` 是当前状态/Activity 的唯一 last-good 依据，Public 与 Admin 共用 `project_verdict`。只有携带明确 Activity 的成功观测或严格校验的权威缺席更新该依据；指标-only 成功不覆盖或续期结论。`active → empty → error` 与首次 `empty → error` 均保留最新缺席并标 Stale，缺席也会在无刷新时自然陈旧；无可证明的确认时间则 Unknown，不从旧 Activity 猜测结论。`activityReceivedAt` 单独披露确认时间，`empty` 不续期旧质押/奖励/计数器的 `last_good_received_at`，也不生成指标快照或新鲜的 0。迁移 72 只为 `source = platscan` 的当前成功回填结论：已交付的 PlatScan 规范化器在每次成功中都携带明确 Activity；当前权威缺席则使用其自身 attempt receipt。其他来源的成功可能只有指标而沿用旧 Activity，旧失败记录也可能遗失中间缺席，因此这两类记录均保持 Unknown，等待真实确认。Admin 同时独立披露 ranking 的 outcome、freshness、attempt/last-good 时间和脱敏诊断，详情新鲜不能替排名背书。修复与实际回归/门禁结果见 [#202 Validator evidence repair](../../research/archive/issue-202-validator-evidence-fix.md)；locked/exiting 的权威否定条件仍需主源证据。

### 15.4.1 Validator 日/月快照趋势与配置日历边界（已由 issue #219 交付）

- 趋势以「配置日历的一日」为单位，而不是以 UTC 日为单位：`from`/`to` 用 UTC 瞬时表达，但归属到该瞬时所在的配置 IANA 本地日，`to` 含其落入的本地日，因此 N 天窗口对应 N+1 个配置本地日。日/月边界由 `local_day_bounds`、`local_period_at`、`local_midnight`（validator/analytics.rs）计算；DST 抹掉的午夜（例如 America/Santiago 的 00:00 不存在）取该本地日最早的合法瞬时，桶仍属于真实的本地日。配置时区来自 `[validator_provider] timezone`：配置加载时若不能解析成 IANA 名即 `ConfigError::InvalidValidatorTimezone` 启动失败，读取时若仍不能解析则返回 `InvalidTimezone` 400——两处都不回退成 UTC。
- 查询有界且披露完整：窗口上限 730 天（`TREND_MAX_WINDOW_DAYS`，超出即钳制并在响应里标 `clamped`），`limit` 默认 90、上限 366（`TREND_MAX_LIMIT`）；分页游标 `before` 是排他游标（严格更旧的配置本地日），响应给出 `continuation`；截断只丢最旧的日，且 `expectedDays`/`observedDays`/`missingDays` 只统计「已应答的那一段」，应答段之外的日绝不算作静默（`coverage` 因此区分 `empty` 与 `unavailable`）。
- 每个点披露真实的取样时间与来源：`sampleTime` 为 `provider` 或 `receipt`（provider_timestamp 优先、否则 receipt，与 §15.5 的 `analytics_period` 同一规则），并同时给出三个具名瞬时（sample_at、provider_timestamp、received_at），因此 provider 优先的取样时间绝不会脱离它旁边的 receipt 单独出现；`delaySeconds` 在任一时间不可用时为 null（绝不写 0），`clockSuspect` 标记 Provider 时间早于接收时间；行里没有的指标就是 Unknown，绝不呈现为 0。
- `coverage=partial` 有两种诚实读法，由应答自身的数字决定：已应答段内确有缺行（`missingDays > 0`）是「静默之外的缺失」，而 `missingDays == 0` 只是窗口未答完（truncated/clamped），此时绝不能说已答段内有缺失。
- 累计语义写死在契约里：`counterSemantics` 固定为 `"cumulative"`，reward/block 是累计值，WebUI 也标注为累计，绝不差分出周期收益或净收益；stake 是该次观测的余额快照，不是累计计数器，因此文案不得把 stake 归入累计措辞。
- 其他时区已存在的行既不合并、也不静默按 UTC 重桶：响应把它们计为 `foreignRows`/`foreignTimezones` 并明确披露，配置日历上的那几天仍如实显示为静默。
- Story 68：Purge 删除该 Node 的 `node_validator_links`，因此趋势里的关联列表只列出仍能解析到存活 Node 的区间，并给出 `deletedNodes` 与 `associationHistoryPartial=true`，说明已删 Node 的关联是「这里不可用」而不是「从未存在」，也绝不把它们重新挂到别的存活 Node 上；共享 Validator 身份与其已存快照日保留，趋势仍可读取。
- rank 归属由同一观测周期保证（复审 F1）：`record_daily_snapshot` 返回它真正落盘的那一天（`StoredDay { local_date, month_key }`），`apply_provider_result` 以 `AppliedDetail.stored_day` 把它交给 `record_snapshot_rank`（validator/analytics.rs），后者只更新那一天的行（权威缺席写 NULL），不再按 receipt 瞬时重新推算一个可能无行或错日的日期。因此 provider 时间戳落在前一个配置本地日时，rank 随那次观测写到那一天而不丢失；detail 失败（`stored_day: None`）或 ranking 失败的周期不写任何 rank，保留该日 last-good——这一行只有唯一的 sample time，rank 必须与它所属的观测同周期写入，否则会假借旧观测的时间戳。由于 `rebuild_monthly_aggregate` 没有零行保护，只有确实更新到该日行时才重建该月聚合。
- 表面：Admin 只读端点 `GET /api/admin/v1/validators/{validator_id}/trend`（Owner 守卫沿用 `http/mod.rs` 的 `/api/admin/v1` 嵌套，见 WebUI §15.21）。

### 15.5 一次性 Validator 模型迁移

- 迁移切换时立即停止以旧手工 Link 作为当前关联或回退，删除旧手工关联及旧 Validator 快照、ranking/counter history、daily snapshots、monthly aggregates；切换后通过自动识别重新采集和积累。
- 不清空 Node 的区块、Peer、metric 等监控历史，也不因 Validator 本地历史清理删除既有 Incident 和必要审计。
- 同步重建或失效化依赖旧模型的 current insight、分类/变化基线和汇总，不能只删历史表而让旧 change/counter 分类继续生效。迁移边界不能被当作一次真实的奖励下降、counter reset、告警恢复或新异常。
- 迁移与 Provider 写入、daily/monthly 重建协调，阻止旧一代任务回填已经清理的旧数据。验证依赖与清理范围后再执行；失败不能以半新半旧状态继续正常服务。这里规定迁移安全结果，不新增具体 migration 编号或执行脚本。
- 本地历史从切换后重新积累，先前已删历史不承诺恢复。PlatScan 再次返回的 lifetime blocks/rewards 仍是链上累计值，不是归零计数；当前选择总计可能暂时下降/未知，不能伪造连续覆盖或完整月份。
- 这是显式一次性模型转换，不改变常规 Retention 对 Validator 日/月数据的保护，也不授予普通 Node 删除操作清除共享 Validator 历史的能力。

实现状态：§15.5 已由 issue #174 交付。Server 启动迁移（`0053_validator_model_migration.sql`）在同一 SQLite 事务内识别并删除旧手工一代（无 automatic Link 的 Validator 身份）的 Link、current insight、ranking/counter history、daily/monthly 聚合及其身份本身，保留 automatic 一代的身份与数据，并写入 `validator_model_migration` 一次性标记。失败即整体回滚并停止启动；运行时身份发现与 Provider 刷新 worker 只在迁移提交后才启动，被删身份也不再拥有可写入的外键归属，因此旧一代任务无法回填。
