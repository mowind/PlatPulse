本文件属于 [PlatPulse 产品与技术设计](../platpulse.md)：§6–§7。章节保留原编号，引用写作 `design §X.Y`。

## 6. Block Summary 与 Block History

### 6.1 Block Summary

保留完整的当前 Block Summary 字段：Node/Network Identity、height/hash/parent hash、链上 block timestamp、Agent `observed_at`、transaction hash count、可选相邻区块间隔、`source`（`subscription` 或 `gap_backfill`）以及 Block Production Attribution。Attribution 中 Coinbase、Seal Signer Match、Protocol Proposer 三者区分；还保留 signer fingerprint、Node key validity/history evidence、seal recovery rule/evidence 和 attribution reason，不得坍缩成单一推断标志。缺乏验证证据时 signer/proposer 显式为 Unknown。**（暂定，Q40：这是唯一保留的显式复杂度权衡，未来可能重议。）**

最新区块通过每 Node 独立的 Head Subscription 触发，按 header hash 完成 Block Resolution（获取并校验区块）。订阅只是正常实时采集触发器；显式恢复计划会对有限高度范围执行 point query。StartupRace、Restart、Reconnect、HeightJump、QueueOverflow、Shutdown 都可以触发有界 Gap Backfill，受高度跨度、查询数量和时间预算限制；普通相邻 head 不会退回轮询。

### 6.2 归属与放置

- Agent 把新采集的 Block Summary 放进 AgentReport 顶层 `block_summaries[]`，把显式缺口放进顶层 `history_gaps[]`；二者按 Node 归属。
- Agent 端只保留有界的 Block Summary 与 History Gap 待上报项，不拥有 Server 的历史高水位或网络级历史。
- Server 从已验证 report 追加近期 Block History，并以 Node 为范围维护 Historical High-Water Mark、coverage intervals、可恢复 Gap coverage、chain divergence 与 resync replay 诊断。
- History 是 best-effort，但不是“从不恢复”：显式 Gap Backfill 只在 Server 自己观察到的空洞内接受——Agent 声明的 open recoverable gap、高于 high-water mark 的新高度，或 Server 已覆盖区间之间既无任何 coverage 记录、也未被 Server 自己写入的 `server_rejected` gap 占据的高度（issue #281；Agent 回传的拒绝副本不算 Server 的判定，ADR 0011 决策 3 不变）；低于该 Node 首个已覆盖高度、已覆盖、已确认缺失或 Server 拒绝的高度一律 `GapBackfillOutsideOpenGap`。超出配置边界的范围由 Agent 声明为 `unrecoverable_backfill` gap，Server 只为其中未覆盖、未被 Server 拒绝的部分打开 open recoverable gap；Agent 则把剩余范围放入有界的本地 backfill backlog（每 Node 最多 16 段、单步受更小的查询数与时间预算约束、连续 8 步无进展即放弃），之后在实时订阅间隙逐步补齐。普通 resync replay（低于 high-water mark 且不在空洞内）不重复写入或计数。
- 空洞有终态（issue #281）：被补齐的高度合并进 covered 区间；一段 open recoverable gap 在整个原始 Block History 保留窗口内都未再被补齐时，保留清理周期把它转为 `permanent_gap`（确认缺失，不再接受回填），并合并相邻同状态区间，使 coverage 行数不随时间无界增长；当 Agent 声明的 `unrecoverable_backfill` History Gap 范围内不再有 open gap 时，Server 写入其 `resolved_at`（修复前已补齐但没有 coverage 记录的高度在此时补记为 covered，避免原始摘要过期后被重复计数）；结算是有界的（issue #282）：一个 gap 只会在其最后一个 open 高度离开 open 状态时变为已结算，因此 Report Ingestion 只检查本次 Report 关闭的高度与新声明的 `unrecoverable_backfill` 范围周围的未解决 gap（不触及 open 高度的实时 tip Report 不做任何结算查询），保留清理只结算本轮确认缺失的区间周围的 gap，并每轮按 keyset 游标额外检查一批（128 条）其余未解决 gap 作为兜底；两条路径都由未解决 gap 与 open 区间的部分索引支撑，耗时超过 500 ms 记 warning，并通过 `platpulse_history_gap_settlement_*` 指标按 `report`/`retention` 路径计数；Agent 回传的 `server_rejected` 副本在超过原始 Block History 窗口后也被标记 resolved，两者之后都由 history gap 保留策略清理。Admin Node Detail 的 Diagnostics 用 `block_history_holes` 显示等待回填与确认缺失的高度及最新 20 段空洞，独立于 Server 拒绝诊断。

### 6.3 全局历史窗口

- Server 保留一个全局可配置的时间窗口，Admin 可动态修改。
- 窗口变更立即生效；缩短窗口时异步删除过期历史；延长窗口不能恢复已删除或已错过的数据。
- 必须提供安全的 min/max/default 边界；边界外的值被拒绝。
- 每次变更把 old/new 值和操作者写入 Audit Event。

### 6.4 明确排除

当前实现没有 Block Explorer，也不保存完整交易 Body。History Gap、Gap Backfill、Historical High-Water Mark、coverage interval、chain divergence 与 resync replay 已作为 Agent 有界待上报项和 Server 有界 ingestion 处理实现；它们仍不构成归档或完整交易查询。

---

## 7. Agent 设计

### 7.1 配置与 CLI

CLI：`enroll`、`generate-node-id`、`validate-config`、`collect-report`、`run`、`shutdown`、`persist-report`、`recover`、`prepare-upgrade`（issue #189 的 v1 升级准备桥接，只在冻结 v1 配置下运行；成功不自动恢复采集）、`checkpoint create`（issue #190 的 Agent 侧协调检查点，保留精确 Agent Store、原配置、凭证与已验证声明证据；不自动恢复采集）。`enroll` 使用一次性 Enrollment Token 与 Server 建立 Agent 身份并保存凭证；Recovery/Rotation 由 Server/Admin credential seams 提供。

~~~toml
server_url = "https://monitor.example.com"
credential_file = "/var/lib/platpulse-agent/credential"
state_db = "/var/lib/platpulse-agent/agent.db"
collection_interval_seconds = 5

[[nodes]]
node_id = "..."
network_key = "platon-mainnet"
rpc_endpoint = "ipc:///var/lib/platon/data/platon.ipc"
data_directory = "/var/lib/platon/data"

# Node process selector 三选一；supervisor 形式下 numprocs > 1 使用 group:process：
#   [nodes.process]
#   kind = "systemd_unit"
#   unit = "platon-validator.service"
#   [nodes.process]
#   kind = "pid_file"
#   path = "/var/run/platon.pid"
[nodes.process]
kind = "supervisor"
program = "platon-validator-a"
~~~

Server 不下发或修改 `nodes.rpc_endpoint`。

### 7.2 Agent Store 与 Durable Spool

流程：

~~~text
每 Node 长连接 Head Subscription → 解析、校验并持久化待上报 Block Summary
按 `collection_interval_seconds` 采集当前观测 → 生成完整 AgentReport → 校验 → 写入 Agent Store
独立发送循环 → 最老报告优先 → 校验完整 Report Receipt → 按 per-Node/per-sample 结果应用 → 事务性删除已确认报告
~~~

`collection_interval_seconds` 可配置范围为 1–300 秒，默认 5 秒；`inventory_revision` 默认 1。发送循环对**单次 HTTP 发送**施加 `sender_deadline_ms`（默认 5000 ms），它约束的是 Agent 等待 Server 响应的时间，而不是 Server 处理一份 report 的成本：deadline 到期后 Agent 放弃本次响应，但 Server 侧已经开始的事务可能仍然提交，因此该 report 可能已经落地。`[backfill]` 默认 `max_height_span = 256`、`max_block_count = 128`、`max_time_ms = 5000`，边界分别为 1–1,000,000、1–100,000、1–60,000 ms。Durable Spool 由 `[spool]` 配置：`max_bytes` 默认 512 MiB（边界 512 KiB–1 GiB）、`max_age_seconds` 默认 24 小时（边界 60 秒–30 天）；flush 预留按容量的 3/4 推导（默认 384 MiB），不是独立配置项，避免与容量上限自相矛盾。默认容量按**约 4 小时**的 Server 停机反推：约 60 个 peer、1 s 采集间隔下每份报告约 33.7 KB、约 0.83 份/s，512 MiB 实测可容纳约 5.3 小时；通用换算 `可承受停机时长 ≈ max_bytes ÷（平均报告体积 × 生成速率）` 与示例见 [部署文档的容量换算](../../deployment.md#durable-spool-capacity-and-the-downtime-it-covers)。配置在 Agent 启动时写入 Agent Store，之后的清理、`host.spool` 诊断与 Admin 证据面板都以该记录为准；未配置时保持上述默认。单个 AgentReport body 的 Server 上限为 8 MiB，Agent 接近配置容量或样本阈值时提前 flush。区块订阅、当前观测采集、报告组装与发送相互解耦；线上仍只使用完整、不可变的 AgentReport，不按数据类型拆分 wire protocol。

要求：

- 报告 bytes 不可变；重试使用相同 `report_id` 与 bytes；
- 投递失败保留原报告；最老报告优先投递；
- Spool 有明确的大小和年龄上限；
- 溢出时丢弃最老的未确认历史报告、记录诊断日志，并保留当前状态的采集与投递；
- Spool 不是用户可见历史；
- 投递余量必须真正收敛：只要 Spool 里还有待投递报告，发送循环就在同一轮里连续排空，不等待下一个采集 tick，也不再受「本轮至多几份」的批大小限制；追赶吞吐必须至少是稳定态生产速率（由 `collection_interval_seconds` 决定）的 3 倍，且采集节奏与积压深度无关——否则投递速率恰好抵消采集速率，重启后的积压会永久滞留在 Spool 中；排空只在成功取到空队列、发送失败（等下一次重试）或 Agent 关闭时结束——不停在「只剩一份」的边界：留一份给下一个 tick 会让停机后的 Spool 永远归不了零，而多排一份的成本就是一次普通发送；被拒收的 Receipt 每应用一份就立即输出诊断（首次立即、后续按 repeating 汇总），不等整轮排空结束；
- Agent Store 损坏时 fail-closed：停止继续伪造报告，等待人工处理。

`sender_deadline_ms` 与重启恢复的关系：deadline 只覆盖**单次 HTTP 发送**，不覆盖 Server 处理一份 report 的成本，也不覆盖 Server 的重启窗口（本机 1.19 GB 数据库的实测重启窗口约 3 s，且此时 Listener 尚未就绪，Agent 的失败只是重试，报告仍留在 Spool 中）。因此 deadline 不应随恢复期长短调整：只要稳定态每报告处理时间远小于 deadline，追赶就不会因为 deadline 而反复回滚已经完成的工作。deadline 到期只放弃**那一次**发送：同一轮里已经收到并应用的 Receipt 仍然计数、对应报告照常出队，只有超时的那一份留在 Spool 中等下一轮重试。

### 7.3 AgentReport

~~~text
AgentReport (protocol major = 1)
├── protocol_version
├── agent_id / agent_epoch
├── boot_id / previous_boot_id / boot_transition
├── report_sequence / report_id / generated_at
├── agent_version / agent_capabilities[]
├── inventory { revision, nodes[] }
├── host                 # 一次 Host Observation
├── nodes[]              # 每个 Node 的完整当前 Component view
├── block_summaries[]    # 新采集的每 Node Block Summary
└── history_gaps[]       # 新声明的每 Node History Gap
~~~

- 报告 bytes 不可变、以 `report_id` 重试；Server 在反序列化前执行 8 MiB body 上限，再重新校验所有身份、时间、数量和边界；
- `boot_transition`/`previous_boot_id` 表达 Continuing、Closing、DrainedPrevious 生命周期；v1 的 `recovered_after_stale` 保留但无效；
- 不允许 Agent-level chain state；Component 的 `null`/省略、权威空值和数字零必须按协议语义区分；
- Host 只出现一次，Node current view 与 block/gap sample 都按 Node 隔离。

### 7.4 Report Receipt

本节及上节 wire 示意为当前 v1 基线；v2 移除 Agent 声明中的 revision、在 Receipt 返回接受版本，见 [§15.10](15.10-server-managed-inventory-revision.md#server-managed-inventory-revision-target)。v2 已实现为独立协议 major 与独立路由，向冻结 v1 直接增添字段仍被禁止。

Receipt 是精确的、可幂等重放的报告确认，不是归档。它同时保留报告级、Inventory、Node current 和 sample/range 结果：

~~~text
ReportReceipt
├── report_id / report_body_sha256
├── disposition: accepted | partially_accepted | rejected
├── server_version / supported_protocol_majors / server_time
├── inventory: accepted | unchanged | rejected (可选)
├── rejections[]                  # code / retryable / reason
├── nodes[]                       # current accepted/rejected + revisions
└── samples[]                     # Block/Gap accepted/retryable_rejected/terminal_rejected
~~~

整体 Inventory 失败不会被拆成“半份 Inventory”；但一个合法 report 可以在一个事务中提交部分 Node/sample 结果并返回 `partially_accepted`。Agent 对 retryable sample 重新排队，对 terminal sample 写 rejection ledger；原始 report 只有在完整 Receipt 应用事务提交后才删除。

---
