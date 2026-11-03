本文件属于 [PlatPulse 产品与技术设计](../platpulse.md)：§4–§5。章节保留原编号，引用写作 `design §X.Y`。

## 4. 运行时与领域模型

### 4.1 拓扑

- 一台 Host 运行一个 Agent；另一台 Host 上的 Node 属于另一个 Agent。
- 一个 PlatON Node 恰好有一个当前 RPC Endpoint 和一个 Network。
- Node 的 block、transaction、consensus、peer 与 error 观察按 Node 隔离，绝不合并成 Agent 级链视图。

### 4.2 Host

Host Observation 每个 Agent 只采集一次；多个 Node 视图引用同一份 Host 观测，不重复计入资源。

### 4.3 Agent

Agent 是绑定一个 Host 的采集进程，拥有：

- 稳定 Agent ID；
- Agent Credential：通过一次性 Enrollment Token 建立，或通过 Owner-authorized Recovery/Rotation 操作重新签发；Server 只保存不可逆凭据摘要。Enrollment、Recovery、Credential Rotation/Revocation 都已由 Server API/CLI seam 提供。Recovery 在同一 Agent 身份上重新签发凭据并推进 Agent Epoch，使旧 Epoch 的报告失效；Rotation 只签发新凭据（可按 overlap 策略保留旧凭据），Revoke 只立即使指定凭据失效，不推进 Agent Epoch；
- 本地 Node Inventory；
- 本地 Agent Store 与 Durable Spool。

Agent 不拥有 Network Registry、Site Access Mode 或用户权限。

### 4.4 PlatON Node

- 稳定 Node ID：改显示名、改 RPC Endpoint、换 Agent 都不改变身份；
- 一个当前 RPC Endpoint（`ipc://` / `ws://` / `wss://`），不支持 failover 列表；
- 一个 Network；
- 独立的当前状态、freshness 和错误。

### 4.5 Network 与 Network Registry

- Network Identity 由观测确定：genesis hash、chain ID、P2P network ID、address HRP；配置的显示名不是身份。
- Network Registry 由 Server 管理；Agent 只声明配置的 network key 和观测到的 identity，Server 校验但不自动改写 Registry。
- Network Identity Mismatch：Node 观测 identity 与注册 Network 不一致时，当前诊断继续，但 block 历史绝不并入注册 Network 的历史。

### 4.6 Node Inventory 与生命周期

以下为当前实现；已确认的 Agent Removal / Node Purge 及删除后的接收边界见 §15.2–§15.3。Retired 与 Purged 不可混用。Server 托管声明版本见 [§15.10](15.10-server-managed-inventory-revision.md#server-managed-inventory-revision-target)，不改变本地配置所有权。

- Node Inventory 是 Agent 本地配置声明的完整 Node 集合，整体校验通过才生效，不允许提交半份 Node Inventory。
- 仍在新 Node Inventory 中的 Node 是 Active；从最新有效 Node Inventory 消失的是 Retired；Agent 停止上报（失联）不等于 Retired。
- Retired Node 保留身份和历史，不再产生新的当前观察。
- 所有 Active Node 都出现在 Home；整体可读性由 Site Access Mode 决定（没有按 Node 的可见性开关）。
- Node Transfer / Pending Transfer：已实现为 Owner-authorized 两阶段流程。Pending 时 source Agent 保持权威；只有 target Agent 在上报中声明同一 Node ID 且 Network Identity 通过校验后才切换归属，冲突、取消或过期不会静默改变归属。

---

## 5. 观测与采集

### 5.1 观测维度

每个 Component Observation 同时表达三个独立维度；其中 Value/Freshness 是由 wire 字段和 Server 时间派生的投影标签，不是单独的协议 enum：

~~~text
Collection State: Starting | Ok | Error | Disabled | Unsupported
Value State:      Current | LastGood | AuthoritativeEmpty | None
Freshness State:  Fresh | Stale | Unknown
~~~

wire 层还携带 `attempted_at`、`latest_observed_at`、`received_at`（仅 Server 填充）、`state_revision`、`value_revision`、可选的 `latest` 和有界 `error { code, message }`。`state_revision` 跟踪状态/错误/尝试时间，`value_revision` 跟踪最后成功值；成功的权威空集合是值，不等于缺失。

规则：

- 采集失败只更新状态与错误，绝不覆盖 LastGood；
- Unknown、Stale、从未观测、Disabled、Unsupported 绝不渲染为 `0`、`false` 或 Healthy；
- 采集成功且结果为空是 AuthoritativeEmpty，不是 Unknown；
- 一个 Component 失败不阻塞同一 Node 的其他 Component；一个 Node 失败不阻塞同一 Agent 的其他 Node；
- Host Observation 每 Agent 采集一次，被 Node 视图引用而不重复计入。

### 5.2 Host Observation

每 Agent 采集一次：CPU、Load、Memory、Disk/Mount、Network Throughput、Agent 与 Server 时间偏差（Clock Unreliable 诊断）、Agent Store/Spool 状态。

### 5.3 Node Process Observation

每个 Node 独立采集：进程是否存在、PID 或 PID 文件、进程身份校验、进程 CPU/Memory、启动时间或运行时长、进程错误；进程来源只来自 Node 显式声明的 selector：`systemd_unit`、`pid_file` 或 `supervisor`，其中 `supervisor` 以 `program` 指定 `supervisorctl` 程序名，`numprocs > 1` 的 program 使用 `group:process` 形式；未声明 selector 时进程组件保持 Disabled，绝不猜测进程身份。配置 `data_directory` 时，Agent 每五分钟递归统计一次该 PlatON 数据目录内常规文件的逻辑大小，并同时记录其所在文件系统总容量，缓存结果，且不跟随符号链接。WebUI 可据此显示 Node Data 的占用进度；容量未知或无效时只显示目录大小，不伪造百分比。Agent 仍然只观察，不重启、不停止、不升级、不执行命令。

### 5.4 Node RPC Observation

每个 Node 独立采集：RPC 可连接性、Client Version、实际探测得到的 RPC Namespace/Method Capability、NodeInfo 中的 Network Identity 与当前 RPC 错误。wire 中的 `monotonic_elapsed_ms` 是整次 Node 采集耗时，不是 RPC latency 字段；当前 `RpcCurrent` 不提供单独的延迟测量。Network Identity 包含 genesis hash、chain ID、P2P network ID，以及可选的 address HRP。

### 5.5 Node Chain Observation

每个 Node 独立采集 `NodeChainObservation`：

- RPC、Sync、Consensus（包含 `validator` 当前成员标志）；
- Network Identity 与慢变 Node static metadata；
- 可选的 `peers` Peer Snapshot；成功的空 Peer 列表是权威空值，省略字段不是空列表；
- Component 的状态、时间戳和 state/value revisions。

Block Summary 不嵌入 `NodeObservation`，而是放在 AgentReport 顶层的 `block_summaries[]`；History Gap 同样放在顶层。当前仍不保存完整交易内容。

### 5.6 Consensus 字段

Consensus 表示 Node 当前的协议状态，不等于 Validator 管理。当前字段包括：

- `epoch`
- `view_number`
- `validator`（当前 validator pool 是否包含此 Node）
- `highest_qc_block`
- `highest_lock_block`
- `highest_commit_block`

`validator = true` 只表示当前共识成员标志，不创建 Validator、Node Validator Link，也不证明某个 Block 由该 Node 生产。独立的 Validator Registry、Link、Provider、历史和 analytics 由 Server 维护。

### 5.7 Peer Snapshot、Presence 与聚合

当前 Agent 可采集有界 Peer Snapshot（成功的空列表是权威值），Server 持久化当前 Peer、Peer Presence Interval 和 5m/1h 聚合及 country 聚合。Public 只暴露聚合 Peer Insight/选定 Node 的 Peer History，不暴露 Peer 地址或身份列表；Admin 也对敏感字段做脱敏。采集失败保留最后成功值及其年龄。

当前 Peer 的写入是 diff 而不是重建（issue #271）：一份 Report 只读一次该 Node 的当前 Peer 与 capability，只有新增、离开和属性变化的 Peer 才产生写语句，重复的快照一条 Peer 写语句都不发，也不刷新任何行级时间戳。因此 Peer Snapshot 的新鲜度取自 `peers` 组件自己的值时刻（`component_status` 里该组件 last-good 值的时间戳，时间基准按 ADR 0014 是观测时间而不是服务器接收时间），而不是由 Peer 行何时被写入推断；一个 Peer 的 `updated_at` 只记录它的属性最后一次变化，不能当作这份快照的年龄。Peer Presence Interval 的开闭边界与这次 diff 共用同一次读取，因此一份快照只把该 Node 的 Peer 集合读一遍。

5m/1h country 聚合与 §5.8 的当前国家投影共用同一条保留规则：缓存行仍在硬保留边界内时按其最后已知国家解析（已到或缺失行内 `expires_at` 的算 last-good），到达边界才计为未知。两者因此对同一缓存得出一致的 known/unknown 口径。

### 5.8 Peer 国家分布（Public Geo Insight）

- 统计对象是每个 Active Node 的 Peer 记录，不是唯一 IP、去重 Peer 或受监控 Node 的部署位置。同一 Peer 被多个 Nodes 观察、或同一 Node 的多个 Peer 共用一个公网 IP，都分别计数；缓存按规范化 IP 复用不改变计数口径。
- Server 在同一 Network 的 Active Node 集合与一致 Peer 数据基础上生成国家计数与未知数量，并由同一投影给出分母；Public DTO 与生成客户端同步，浏览器不用独立 Peer 总数相减。Node visibility 是 Admin-only 元数据，不参与 Public 过滤（与现有 Public Projection 一致）。
- 未知桶包含：无可用公网 IP 的记录，以及有公网 IP 但没有可保留国家结果的记录。两个数量都由 Server 计算下发，理由只在 Server 能证实时才细分：归一到相同空值的原因不凭空区分，浏览器也不做减法推导。
- 尚在有限保留边界内的 last-good 国家结果保留国家归属并标为 Stale（不计入未知）；超出保留边界则转为未知。已知国家缺少底图代表点不是未知国家。
- 成功空 Peer Snapshot 是权威零；never-observed 或没有可靠分母时保持 unavailable/unknown 且不输出计数。Geo database/国家结果状态与 Peer 采集状态、新鲜度互相独立：Geo Current 不代表 Peer 在线或刚刚采集，采集失败保留的 Peer Snapshot 仍按自身状态展示。
- 每个 Network 的 Public Geo Insight 是独立投影；本契约不提供跨 Network 的全局去重 Peer 总数，也不把缺失 Network 当零或制造完整性百分比。部分覆盖时投影如实标注 scope，Public 只含国家代码与计数，不含原始 IP、RPC Endpoint、精确位置或敏感错误。

### 5.9 Geo Provider、后台解析与缓存（issue #132、#134、#135、#136）

- Geo Provider 由 Owner 在 Settings 选择，当前实现 `Disabled`、`Local MMDB`、`IPinfo` 与 `GeoJS`；一次只用一个，不自动回退。只有本 Server 真正实现的标识会出现在管理界面与可持久化的 `provider` 值中，未实现的供应商（例如 IP-API）不在其中，也没有任意 URL 或第三方 MMDB 自动下载控件。Admin 诊断逐项给出 `sends_peer_addresses` 与该 Provider 的固定外发告知 `disclosure`（含它自己的目的地），Settings 直接渲染服务端下发的句子，不在浏览器里自行拼接，因此"告知里的目的地"与"请求真正发往的端点"来自同一个常量、不会漂移；只有 Owner 显式选择才启用，升级不会打开外部查询。
- 选择持久化在 `server_settings`（`geo_provider` + `geo_provider_generation`），每次变更推进 generation。升级时若该键不存在，由部署是否配置本地 MMDB 决定：已配置本地数据库的安装继续用 `Local MMDB`，没有配置的保持 `Disabled`，两条路径都不新增外部请求。MMDB 路径只来自 Server 配置（`[geo] mmdb_path` 或 CLI），不是 Admin 可写字段。
- 国家解析只在后台执行：Report Ingestion 事务只记录当前 Peer 引用，不读 MMDB、不做外部 HTTP，也不写 `geo_location_cache`（最后引用时间的刷新、未被引用行的删除与容量上界都属于同一个维护周期，见下一条）；`geo_backfill` 负责调度、按规范化公网 IP 去重、有限并发、有界批量（`MAX_BACKFILL_BATCH`）、无国家结果的小时级重试间隔，以及每轮结束后的 realtime 失效。慢查询、外部服务慢或被限流都不会占用 Receipt 事务，也不影响报告接收与就绪状态。Provider 只决定"单个地址如何解析"：Local MMDB 在阻塞线程读文件（`MAX_BACKFILL_CONCURRENCY`），每个 External Geo Provider 在有界外发边界发 HTTP（`GEOJS_MAX_CONCURRENCY` / `IPINFO_MAX_CONCURRENCY`，均低于本地并发）；调度、去重、保留、generation 校验与缓存写入由所有 Provider 完全共用，所以切换供应商只改变目的地与字段拼写，不改变统计对象。
- 结果按 Geo Provider 与规范化 IP 隔离存放在 `geo_location_cache`：国家代码、状态（`current` / `no_country` / `failed`）、最近尝试时间、最近成功时间、出生时间、有效期与最后引用时间。所有 Geo Provider 的成功国家解析共用 7 天 TTL：写入的 `expires_at` 取成功时刻加 7 天与行出生时刻加 30 天的较早者；到达硬边界后的成功解析重建行并重置出生时间。已有且仍保留的缓存行不会因 TTL 变更而重写 `expires_at`，常规调度与新鲜度继续以行内原值为准，直到下一次成功解析才按新规则写入。投影与 5m/1h country 聚合只读当前 provider 的行；切换 provider 后旧 provider 的行立即删除，维护周期（60s）先把所有当前引用地址的最后引用时间刷新一遍，再在同一轮里删除未被引用的行并执行容量上界，所以一个地址的最后引用时间最多滞后一个周期、未被引用的原始 IP 仍在一个周期内清除（issue #271）；已到或缺失 `expires_at` 但仍在 30 天硬边界内的国家结果继续作为 last-good Stale；失败或无国家结果更新状态与尝试时间，但不延长已有结果的有效期，也不把 last-good 刷成 Current；到达边界则回到未知。
- 每个 External Geo Provider 都只走自己的固定 HTTPS 端点：IPinfo 为旧式 `https://ipinfo.io/{ip}/json`（读取 `country`，不替换为 Lite API），GeoJS 为 `https://get.geojs.io/v1/ip/geo/{ip}.json`（读取 `country_code`）。两者都不携带 token、不提供凭据 UI，也不把文档或定价页的免费额度当作供应商承诺。只发送经 Server 校验的规范化公网字面 IP：私网／loopback／link-local／保留地址在外发边界再次拒绝，主机名从不解析 DNS，请求目的地不可配置，重定向一概不跟随（`redirect::Policy::none()`）。边界包含有界超时、有界响应体、有限并发与有界重试/排队，机制由 `geo_external` 共用，每个 Provider 只提供编译期 profile（目的地、国家码字段、界限、稳定理由与署名）。
- 结果解释由共享外发边界统一处理并区分四类：国家码字段存在且合法是 `Country`，字段缺失、为 null 或为空串是权威 `NoCountry`，出现但不是两字母 ASCII 国家码是畸形结果（按失败处理），429 是 `RateLimited`；非成功 HTTP、传输失败、超时、超长响应体、非 JSON 与"成功但非 JSON 对象"都是失败。只有国家码字段被读取，provider 差异在这一层适配：IPinfo 读 `country`，GeoJS 读 `country_code`（GeoJS 的 `country` 是英文全名，任何情况下都不当国家码读取）。国家码只做形状校验（两个大写 ASCII 字母），与 Local MMDB 路径共用同一个过滤器：字段按 ISO 3166-1 alpha-2 定义，本 Server 不固化一份会过期的国家清单，畸形值在所有路径上都成不了国家。
- 429 不写入任何按地址的尝试记录：地址保持 pending，改由有界的进程内 Provider 级退避（60s 起、翻倍、上限 15 分钟，任何正常响应即清零）决定重试窗口；写入路径本身也拒绝 `RateLimited`，即使将来有调用方绕过调度也不会留下"失败尝试"。非成功 HTTP、传输失败、超时、超长响应体与畸形 JSON 都按失败记录，只记尝试、绝不擦除 last-good。
- 外部 Provider 的状态是外发路径的真实状态：每个 Provider 各自记录最近一次外发是否产生了可用结果与限流窗口，被限流或最近一次尝试无可用结果时 Admin 与 Public 都报 `error`（Public 只给固定非敏感理由），下一次正常响应即回到 `current`。退避窗口与失败标志按 Provider 隔离，切换供应商不会让 A 的迟到交换决定 B 的窗口或状态，诊断中的 `rate_limited_until` 也取自当前选中的 Provider。因为 External Geo Provider 不读数据库，它们的诊断不含 build epoch、digest 或"数据库加载时间"，`last_success_at` 来自按 Provider 隔离的缓存行。
- 每次后台写入在事务内重新校验持久化 selection 与进程内 generation：配置变更后迟到的任务不能回写成新配置的结果。cleanup 只应用硬保留边界、当前引用规则与容量上界：有效期到达只把结果标为 Stale last-good，不会仅因到期而删除仍在硬边界内的国家结果。
- 管理接口：`PUT /api/admin/v1/geo/provider`（Owner-only、Origin/JSON/CSRF、审计 `geo_provider_changed`）在事务内推进 generation 并返回新的诊断；`POST /api/admin/v1/geo/refresh`（同样 Owner-only、Origin 与 CSRF 校验、审计 `geo_refresh_requested`；与其它无请求体的管理动作一致，不要求 JSON Content-Type，因为不存在可被跨站表单伪造的请求文档）触发全局强制刷新；`GET /api/admin/v1/geo` 提供 provider、generation、可选列表（含 `sends_peer_addresses`）、数据库状态、缓存国家数、完整待解析队列长度、最近成功时间与外部 Provider 的 `rate_limited_until`，并附带 `refresh`（最近一次强制刷新的运行态与计数）与 `refresh_unavailable_reason`（Disabled／不可用时的稳定说明），均不含路径、原始 IP、请求 URL 或供应商错误原文。Provider 为 Disabled 但本地数据库已配置且不可用时，诊断仍显示该数据库的错误说明，Owner 在选择前即可看到。
- 全局强制刷新（issue #136）是真实的重新解析而不是清缓存：范围在 run 开始时冻结为全部 Networks 的 `current_node_peers` 中当前引用的、经信任边界校验的规范化公网 IP，按地址去重，与 Home 筛选无关，不翻查无引用历史，也不向 Agent 下发任何采集或控制命令；之后新增的 Peer 引用仍由常规后台路径处理，所以运行中的分母不会漂移。每个地址绕过缓存有效期真正重新解析，并通过与自动路径完全同一条 generation 校验的缓存写入落库（`geo_refresh` 与 `geo_backfill::run_pass` 共用 `resolve_address`、去重、有限并发与保留规则），因此 IPinfo 与 GeoJS 无需新增 provider 分支即自动获得相同行为；只有清缓存、只返回 accepted 或等待下一次 Agent 上报都不构成完成——范围内全部地址都得到权威结果（成功／无国家／失败）才进入 `completed`。同一 provider+generation 的重复提交加入同一个 run（响应 `started: false`），不同配置的请求先把旧 run 标记为 superseded；同一时刻只有一个 run，运行期间常规后台 pass 只让出该 run 冻结范围内、尚未完成的地址，其余引用（包括运行中新增的 Peer）继续按既有节奏解析，因此一个地址不会被两条路径同时解析，中途新增的引用也不会被推迟到 run 结束。run 结束时主动唤醒后台路径。计数一律按地址查询：`total_lookups` / `completed_lookups` / `resolved_lookups` / `no_country_lookups` / `failed_lookups` / `rate_limited_lookups`，并单独给出 `peer_records_in_scope`（当前引用这些地址的 Peer 记录数），与地图按 Node 计数 Peer 记录的口径并列而不互相推算；一个地址服务多条 Peer 记录时两个数字保持不同。provider/generation 变化、停用、限流或 Server 停机都会真实终止：终态为 `aborted` 并带稳定、无路径的 `abort_code`/`abort_reason`，未查询的地址保持 pending，绝不虚报成功；429 只计入 `rate_limited_lookups`、绝不当作失败结果或 completed，迟到任务的结果继续被 generation 校验拒绝。终态只存在于进程内，重启后不保留"进行中"的假象；刷新与自动补全的结果都通过既有 `geo` realtime 失效通知 Admin 与 Public。
- Public 仍然只见国家代码、计数、Provider 自有的稳定状态与署名：署名随所选 Provider 变化（Local MMDB 为 MaxMind 署名，IPinfo 为 IPinfo 署名，GeoJS 为 GeoJS 来源加 MaxMind 原句，Disabled 无署名），浏览器不自行拼接；外部 Provider 的失败理由对 Public 只呈现固定非敏感文案，切换 Provider 后旧来源的结果不会以新 Provider 的名义出现（退出选择的 Provider 的行会被立即清理）。
- IPinfo 票（#134）不含 GeoJS、全局强制刷新与第三方 MMDB 自动下载；GeoJS 票（#135）复用同一条后台执行路径与 provider 隔离把它接通；全局强制刷新（#136）在同一条执行路径上实现，未新增 provider 分支、未新增分叉调度。浏览器 E2E 只验证两个外部 Provider 选项、各自的知情说明、Disabled 时刷新不可用的明确说明，以及 Local MMDB fixture 下的真实强制重查；绝不在真实浏览器会话里选中或触发外部 Provider：E2E 用的 Server 没有（也不允许有）可配置目的地，选中即意味着把 Peer 地址发给真实第三方，违反"CI 不向供应商发送真实 Peer IP"。外发行为（成功、无国家、畸形响应、HTTP 失败、429、超时、恢复、慢响应、切换与迟到结果）以及强制重查绕过有效缓存、部分失败计数、限流终止与 Peer/IP 双口径，由进程内的确定性替身与真实 MMDB fixture 覆盖。
- IPinfo 旧式接口的核查结论（2026-08，issue #134）：该无 token 路径在官方文档中与 Lite API 并列存在，官方支持文档称无账号公共 API 为每源 IP 每天 1,000 次、旧式 Free API 为每月 50,000 次，两者口径不一致，因此本实现只把它当作有界、可失败的外部依赖，不承诺额度、不规避限流。官方条款允许为内部业务目的使用查询内容，禁止转售或再分发，付费档位标注 `Attribution required`，故 Public 在 IPinfo 下展示 IPinfo 署名。未实测项：未对真实服务发起联调（需另行授权），因此响应字段、限流响应头与真实限额均未验证；实现只依赖已确认的 `country` 字段与 429 状态码。

- GeoJS 接口与许可核查结论（2026-09，issue #135，详见 `docs/research/geojs-provider.md`）：固定 HTTPS 端点 `https://get.geojs.io/v1/ip/geo/{ip}.json`；无鉴权、无 token；文档声明当前无限流但不承诺额度，TOS 禁止"excessive"使用并保留限流与封禁权，因此实现只把它当作有界、可失败的外部依赖。二字母代码在 `country_code`（`country` 是英文全名，不读取）；**无国家地址返回 HTTP 200 且国家相关键整体缺失**，这与 IPinfo 的"字段缺失/为空"语义一致，因此共享边界把"对象缺少国家码字段"判为权威 NoCountry；畸形 IP 返回 404 HTML，本 Server 只发送经校验的规范公网字面量所以不可达，即便出现也按失败处理。同族的 `/v1/ip/country/` 端点字段名与空值语义都相反，不得与之互换。GeoJS 自身 TOS 无署名条款，但其数据来源为 MaxMind GeoLite，故 Public 署名同时给出 GeoJS 来源与 MaxMind 要求的原句。未实测项：未对真实服务发起联调（需另行授权），真实限流行为、响应头与所服务的 GeoLite 数据版本均未验证；GeoJS 背后法律实体无一手来源，不作事实陈述。

---
