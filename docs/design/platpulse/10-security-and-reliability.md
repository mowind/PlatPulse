本文件属于 [PlatPulse 产品与技术设计](../platpulse.md)：§10–§11.5。章节保留原编号，引用写作 `design §X.Y`。

## 10. 身份与安全

### 10.1 Agent 身份

- Agent Credential 由 Server 的 Enrollment/Recovery/Rotation seams 发行和管理；Agent 通过一次性 Enrollment Token 获取并保存凭据，凭据只能访问 Agent API；
- Server 不保存 Credential 明文；凭据文件 Agent 用户可读、其他用户不可读；
- Enrollment、Recovery、Rotation 和 Revoke 都已提供相应 credential seam：Recovery 额外推进 Agent Epoch 并拒绝旧 Epoch 报告，Rotation 只更新凭据集合（可保留短暂 overlap），Revoke 只立即使指定凭据失效；当前 SPA 没有 Enrollment/Recovery/Rotation 专用管理路由，但 Agent Detail 已提供单个凭证撤销。§15.2 的新增接入入口和 Agent Removal 是待实现能力；
- 不静默创建相同身份的第二个 Agent；
- Human Session 不能访问 Agent API。

### 10.2 Human 身份

当前支持两种人类主体：Owner 与 Viewer（初始化/CLI 或 Owner Admin 可创建）。Viewer 可登录并读取 Home，即使 Site Access Mode 为 Private；只有 Owner 可访问 Admin。支持 Session Cookie、Argon2id 密码哈希、登录限流、Session 撤销、基础 CSRF/Origin 校验。Session 只有绝对生命周期（自创建起 7 天，即 `expires_at`），没有空闲超时：长时间开着但不操作的 Home/Admin 页面不会因此被登出；`last_seen_at` 只作为 Admin 可见的活动证据（最多每 60 秒写入一次），不参与有效性判定。Session 在越过 `expires_at`、被显式登出或 Owner 撤销、或所属用户改密/改角色/被禁用时结束；Session Cookie 不设 `Max-Age`，关闭浏览器即丢弃。匿名 Guest 的 Home 可读性由 Site Access Mode 决定（默认 Private）。

「读不到 Session」不等于「Session 失效」：Session 查询本身因数据库不可用（重启、损坏、连接耗尽）而失败时，受保护路由回答 `503 unavailable`，不是 `401 auth_required`；绑定 Session 的 Admin/Public 流在重查读不到数据库时保持连接并在下一秒重查，但「保持连接」不是「确认授权」：读不到数据库期间流不发送任何失效事件（游标不前进，事件不会丢），也不延长 Session 的绝对有效期——连接时已核验的 `expires_at` 在本地独立检查，超过它即发送 `reset` 并关闭流。只有明确的 `401`（`auth_required`）才是「未登录」，其余失败都是 Server 暂时不可达（issue #267）。

### 10.3 传输

- 开发模式仅允许 loopback 明文 HTTP；
- 生产必须 HTTPS/TLS 或明确配置的可信反向代理；
- URL 不携带凭证；日志与错误脱敏；
- Public DTO、Admin DTO、Agent DTO 不得互相复用后在前端删字段。

---

## 11. 可靠性与故障语义

### 11.1 Agent 故障

- 无法连接 Server：报告保留在 Durable Spool；
- 进程重启：恢复未投递报告；
- 关闭：尽力保存最终报告，不无限阻塞；
- Spool 溢出：丢弃最老的未确认历史报告并记录诊断，保留当前状态采集；
- Agent Store 损坏：fail-closed，等待人工处理。

### 11.2 Node 故障

- 一个 Node 的 RPC 失败不影响其他 Node；
- 失败更新状态与错误，LastGood 保留展示并带显式错误与年龄；
- 从未成功则显示 Unknown；
- 绝不渲染为 `0`、`false` 或 Healthy。

### 11.3 Server 故障

- 重启后从 SQLite 恢复当前投影与 Block History；
- 重复 Report 不重复追加历史；
- Public WebUI SSE 连接中显示 `Connecting to live updates`，断开显示 `Live updates paused`，并通过 REST 重新获取；连接成功的正向传输状态有意保持沉默，不渲染也不播报任何提示——它不改变访问者应当做的事或应当相信的事，只表示通道已打开，这些传输状态均不代表 Node 健康或观测新鲜度（工单 #138 取代 #126 验收标准中"连接成功显示 `Live updates connected`"一条）。Admin 保持紧凑的既有 `Starting`/`Current` 传输文案。

### 11.4 数据边界

当前观测/历史数据按用途分为多类，而不是只有两种：

~~~text
Current Projection   Agent/Node/Host/Peer/Validator 的最新有效状态（含 LastGood）
Block History        全局 History Window 约束的近期 Block Summary（best-effort）
Gap/Divergence       有界 History Gap、coverage、resync 与 chain-divergence 证据
Peer History         Peer Presence 与 5m/1h aggregate（按 retention policy）
Validator History    ranking/counter history、daily snapshots、monthly aggregates
Metric History       Node 与共享 Host metric history/export（原始样本按 raw_metric_sample family，默认 24 小时；两个聚合层把可回答窗口延伸到 30 天，见 §11.6）
Sync/Consensus State Node 的同步/共识状态变更日志（`node_state_observations` 与它的账本 `node_state_series_state`，按 observation_state family 默认 30 天；同一窗口的五个区块高度数值走 Metric History，见 §11.7）
Operations/Audit     Alert/Notification/Operation/Backup/Restore/Doctor 与审计记录
~~~

Server 仍不会用零值填充缺失区间；Retention 按 data family 分别配置，Block History Window 是其中独立的一项。

每个 family 的保留契约由 Server 的 retention catalog 声明（`crates/platpulse-server/src/retention.rs`）：策略类别（raw / investigation / contract）给出安全下限（raw 至少 24h，investigation 与必要的 aggregate/state 至少 30d），family 自有 min/max/default 只能在此之上收紧或放宽，Server 拒绝任何低于下限的值；未启用或未支持的 family 不声明任何 cleanup target，因此不会暗示已保存其未实现的历史。支持的 family 声明其 cleanup target（固定 SQL、有界批量、恰好一个绑定参数），执行时按 preview 冻结的 cutoff 逐批运行；估计计数是 Server 给出的上界而不是冻结行集，也不是执行配额：某批返回的行数少于批量上限即证明该 cutoff 之后已无过期行，视为该目标完成；某批满额则继续；实际释放行数可以超过估计值，结果按 Server 记录的真实释放数报告。plan 在排队瞬间冻结（scope、policy version、每个 family 的 cutoff），执行期间不再读取实时策略，因此运行中的策略变更既不会加长也不会扩大正在执行的范围。preview 的 policy version 由该 family 记录的值与时间戳摘要而成，并使重预览判定同时比较 retention_days 本身，因此任何会改变计划内容的编辑都被判为过时；preview 校验与入队是两条语句，排队瞬间的校验存在极窄窗口，但已冻结的 plan 使该窗口无法扩大或加长将要释放的行（把校验并入同一事务留待后续票据）。已记录的历史在更长保留下不会被自动缩短。

按批准计划执行、命令去重与诚实的取消（issue #211）：每次清理执行都必须绑定一个仍然有效的 preview，Server 在执行前重新校验 policy version、scope 与每个 family 的 cutoff，任何一项变化都返回 `409 retention_preview_stale` 且不入队。Owner 的一次确认是一个持久化的命令身份（请求体中的 `requestId`，与中间件生成的 HTTP 关联 id 不同）：同一 `requestId` 的重复提交（双击、第二个标签页、响应丢失后的重试）不会产生第二次清理，Server 按 `operation_requests` 的 `(kind, intent_fingerprint)` 唯一索引回放已记录的结果，Retention 的 intent fingerprint 是 `retention_run:<previewId>`；同一 `requestId` 用于不同意图返回 `409 operation_request_id_conflict`，而不是静默执行一个新动作。同一时刻只允许一个 queued/running 的 `retention_run`（部分唯一索引），因此第二次确认要么回放已记录的任务，要么被明确拒绝（`409 retention_run_in_progress`）；命令记录的存活期与 preview 一致（24h）。取消是显式记录而不是回滚：queued 的清理在取消当刻由 Server 写入终态与结果（已释放 0 行、未完成目标数、按 family 明细与说明），running 的清理在下一个安全检查点停止并记录已释放行数与未完成目标数；已释放的数据永不回滚。重启后被中断的清理按已记录的 plan 汇总已释放行数与未完成目标数，而不是一条通用错误。释放数据与其记账在同一次提交的同一事务内完成，因此不存在“行已删除而记录仍称从未触碰”的状态。从未开始且没有计划工作的任务不编造结果（queued 的 Doctor 诊断取消后 `result_json` 保持为空，`doctor::last_run` 因此不会把一次取消当成一次诊断报告）。

### 11.5 低空间保护（issue #212）

**后端适用边界。** 本节描述的低空间保护只适用于 SQLite 后端。按 [ADR 0013](../../adr/0013-postgresql-server-storage-backend.md)，PostgreSQL 后端（0.3.0 目标，实施中，尚未实现）不提供容量保护：数据库所在磁盘归数据库运维方所有，`[capacity]` 与 PostgreSQL 后端同时配置将在启动时被拒绝，Doctor 的 storage 检查与 Admin 容量面报告“不适用”；磁盘写满时写入失败，Ingestion 返回可重试的 503，而不是静默丢数据。

Server 启动时解析可选的 `[capacity]` 段（crates/platpulse-server/src/config.rs），由 crates/platpulse-server/src/capacity.rs 按 hysteresis 采样挂载点可用空间：

- `enabled = true` 时必须同时给出 `pause_below_bytes` 与 `resume_above_bytes`，不设默认阈值；契约是 `resume_above_bytes >= pause_below_bytes > 0`，采样间隔限 5..86400 秒（默认 60）。缺少阈值时启动直接失败并说明原因，而不是替部署方猜一个 GiB 数字。
- 进入保护时向 `capacity_protection_intervals` 写入一行，用当时的真实容量填 `opened_total_bytes`/`opened_available_bytes`，`started_reason` 为 `low_space`，并以部分唯一索引保证同一时刻最多一个未结束区间；只有恢复到 `resume_above_bytes` 以上才结束该区间（`ended_reason` 为 `resumed`），保护被配置关闭时以 `protection_disabled` 结束，两条路径都保留原始开区间证据供审计。
- 保护生效期间 Report 仍完整校验、当前投影与 Report Receipt 照常写入，但可选的 metric history（Host/Node series）不再追加：每次被跳过的样本在同一 ingestion 事务内累加到 `capacity_skipped_series`（`scope_kind`/`scope_key`/`metric`/`dimension` 加 `skipped_count` 与首个/最近跳过时间戳，`dimension` 是 Host 存储系列的挂载路径、其余序列为空串，见 §11.6 的 issue #215），因此样本未落库与缺口已记账要么一起提交、要么一起回滚；记账失败会回滚整个 Report 并返回可重试的 503，历史因此不会静默丢失，也不会为了腾空间删除已有数据或降低精度。
- 跳过只作用于可选 metric history 的追加。Block History、Receipt、LastGood 与 current projection 的语义不变，Receipt 的 samples/disposition 也不把跳过的可选样本算作已接受；可见缺口由 `capacity_skipped_series` 与 Admin capacity 面表达（见 docs/design/webui.md 第 15.11 节）。
- 采样失败是 fail-open：保留当前 state 并记录 `sampling_error`，不因读不到 statvfs 就伪造缺口。重启时 `reconcile` 接管已打开的区间：仍低于 pause 阈值就继续保护，否则以 `resumed` 或 `protection_disabled` 结束。
- Admin 通过 `GET /api/admin/v1/capacity`（Owner-only）看到带年龄的容量状态、最近区间与被跳过的序列；Doctor 的 storage 检查在保护生效时报 FAIL、未配置时报 NOT_CONFIGURED、采样异常报 WARNING；`/metrics`（独立内部监听器，主监听器按设计把该路径当作 not-found）暴露 `platpulse_capacity_total_bytes`、`platpulse_capacity_available_bytes`（未知时省略而不是写 0）与 `platpulse_capacity_paused`。
