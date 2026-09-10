# PlatPulse 产品与技术设计（当前实现基线）

## 1. 文档状态

- 状态：当前实现与边界基线；实现、迁移、运行时路由和本文件应相互校验。
- 适用范围：`platpulse-core`、`platpulse-agent`、`platpulse-server`、`platpulse-web`。
- 领域术语：以仓库根目录 `CONTEXT.md` 为准；本文件只说明这些术语在当前链路中的关系，不重复定义。
- 规范性用词：
  - 必须：实现和验收不可省略；
  - 不允许：违反即破坏边界或领域不变量；
  - 可以：允许实现，但不是当前核心链路的必需能力；
  - 明确未实现：当前代码和运行时边界均未提供，不应在页面、API 或数据模型中暗示已存在。
- 本文件区分三种状态：已实现的 Server/Agent/API 能力、当前 `platpulse-web` 实际注册的路由，以及明确未实现的产品边界。Validator、Geo、Peer、Alert、Notification、Backup/Restore、Retention、Operation、Node Transfer、Recovery/Rotation 等不再统一视为“未来功能”：其中 Server/API 和后台工作器已有实现，但很多扩展没有当前 SPA 页面；详见 §3、§8、§9 和 `docs/design/webui.md`。

---

## 2. 产品形态

产品分离参照 Komari（<https://github.com/komari-monitor/komari>），但监控对象是 PlatON Node 而不是服务器：

- Home：只读、以 Node 为中心的监控面。根路由 `/` 展示 Active Node 卡片，Network → Node → Node Detail 展开公共投影；Network Overview 还展示聚合 Peer Insight、country-only Geo Insight、Validator cards/history/analytics。Node Detail 由最近两个连续 Block Summary 推导出块间隔，但不展示 Bounded Block History 列表。Site Access Mode 为 Public 时匿名 Guest 可读选定 Public GET/SSE 路径；为 Private 时 Owner 或 Viewer 登录后可读。
- Admin：认证后的 Owner-only 系统概览与配置面。当前 SPA 路由覆盖 Overview、Agents、Nodes、Networks、Settings、Sessions 与 Audit；Server/Admin API 另外提供 Geo、Validator、Alert、Notification、Operation、Retention、Backup/Restore、Doctor、Transfer、People、Enrollment/Recovery/Rotation 等能力，但尚未全部注册为页面。
- 同一个 WebUI 承载 `/` 与 `/admin` 两组路由，使用不同的 DTO、查询缓存、权限和导航。
- 站点级 Site Access Mode（Public/Private）由 Owner 配置，变更记 Audit；当前默认 Private。Node DTO 和 Admin 页面仍保留 `visibility` 字段与 Owner mutation 作为兼容/诊断字段，但 Public 查询实际按 `lifecycle = active` 过滤，不按该字段隐藏 Home；站点模式才是有效的匿名访问开关。

目标架构：

~~~text
┌──────────────────────────────────────────────┐
│ Host                                         │
│                                              │
│  PlatON Node A ─┐                            │
│  PlatON Node B ─┼── platpulse-agent          │
│  PlatON Node C ─┘    - 本地采集器             │
│                       - 本地 Durable Spool    │
└───────────────────────┬──────────────────────┘
                        │ 出站 HTTPS AgentReport
                        ▼
┌──────────────────────────────────────────────┐
│ platpulse-server                             │
│ - Agent 鉴权与 Report Ingestion               │
│ - SQLite 当前投影 + 有界 Block History        │
│ - REST API + SSE invalidation                │
│ - 同源托管 WebUI 静态资源                     │
└───────────────────────┬──────────────────────┘
                        │ 同源 REST / SSE
                        ▼
┌──────────────────────────────────────────────┐
│ platpulse-web                                │
│ - Home：Network → Node → Node Detail          │
│ - Admin：概览与配置（不复制 Home Node Detail） │
└──────────────────────────────────────────────┘
~~~

### 2.1 边界

- Agent 只能主动连接 Server；Server 不反向连接 Agent。
- Server 不向 Agent 下发 RPC Endpoint、命令、升级包或脚本（无远程控制）。
- WebUI 不直接连接 PlatON Node，只访问 Server API。
- Server 是信任边界：所有 Agent 上报字段必须重新校验，绝不把 Agent 输入当作可信数据。
- Home 与 Admin 使用不同 DTO、route group、查询缓存和权限；Public Projection 不是 Admin DTO 的运行时删字段版本。

---

## 3. 当前范围与产品边界

### 3.1 核心链路

1. 一个 Server、每个 Host 一个 Agent、一个 WebUI；一个 Agent 监控本 Host 上的多个 PlatON Node。
2. 多个 Agent 可以向同一个 Server 上报。
3. 每个 Node 的观察和错误状态完全独立；一个 Node 失败不阻塞同一 Agent 的其他 Node。
4. Home 展示 Node 当前状态、近期 Block History 衍生指标和公共 Peer/Geo/Validator 投影；Admin 提供系统概览并配置 Agent/Node/Network、全局历史窗口与访问控制。
5. 采集、上报或接收失败不会静默覆盖最后成功值，也不会重复写历史；Report Receipt 可以对 Node 和样本给出部分结果。
6. Server 重启后保留 Agent、Node、当前投影、Block History 及已配置的扩展投影。
7. WebUI 在 360px 手机宽度、平板和桌面宽度下可用。
8. 初始部署保持 Linux-first、单租户、单 Server、SQLite。

### 3.2 已实现的 Server/Agent 扩展

当前代码和迁移已经提供：Agent Enrollment、Recovery、Credential Rotation/Revocation、Boot/Shutdown 生命周期；Peer Snapshot、Peer Presence 与聚合历史；GeoLite2 country cache；Server-side Validator Registry/Links/PlatScan Provider、历史与日/月 analytics；Typed Alert/Incident/Silence/Maintenance；at-least-once Notification Delivery；Node Transfer；Retention、Operation、Backup/Restore、Doctor；独立 Prometheus metrics listener。它们由 Server/Admin API、CLI 或后台工作器提供，是否有当前 SPA 页面由 `docs/design/webui.md` 的路由矩阵单独决定。

### 3.3 明确未实现或明确排除

以下仍不是当前产品边界：完整交易 Body、Block Explorer/Archive、RPC Endpoint failover、Server 远程控制（命令、重启、升级或脚本下发）、多租户/HA/PostgreSQL/集群、SSO/OIDC/TOTP/WebAuthn，以及非 Linux Agent。当前 SPA 未注册的扩展页面也不应被链接为已存在的页面；这不等于对应的 Server/API 能力不存在。

---

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

每个 Node 独立采集：进程是否存在、PID 或 PID 文件、进程身份校验、进程 CPU/Memory、启动时间或运行时长、进程错误；配置 `data_directory` 时，Agent 每五分钟递归统计一次该 PlatON 数据目录内常规文件的逻辑大小，并同时记录其所在文件系统总容量，缓存结果，且不跟随符号链接。WebUI 可据此显示 Node Data 的占用进度；容量未知或无效时只显示目录大小，不伪造百分比。Agent 仍然只观察，不重启、不停止、不升级、不执行命令。

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

5m/1h country 聚合与 §5.8 的当前国家投影共用同一条保留规则：缓存行仍在硬保留边界内时按其最后已知国家解析（超出 24h TTL 的算 last-good），超出边界才计为未知。两者因此对同一缓存得出一致的 known/unknown 口径。

### 5.8 Peer 国家分布（Public Geo Insight）

- 统计对象是每个 Active Node 的 Peer 记录，不是唯一 IP、去重 Peer 或受监控 Node 的部署位置。同一 Peer 被多个 Nodes 观察、或同一 Node 的多个 Peer 共用一个公网 IP，都分别计数；缓存按规范化 IP 复用不改变计数口径。
- Server 在同一 Network 的 Active Node 集合与一致 Peer 数据基础上生成国家计数与未知数量，并由同一投影给出分母；Public DTO 与生成客户端同步，浏览器不用独立 Peer 总数相减。Node visibility 是 Admin-only 元数据，不参与 Public 过滤（与现有 Public Projection 一致）。
- 未知桶包含：无可用公网 IP 的记录，以及有公网 IP 但没有可保留国家结果的记录。两个数量都由 Server 计算下发，理由只在 Server 能证实时才细分：归一到相同空值的原因不凭空区分，浏览器也不做减法推导。
- 尚在有限保留边界内的 last-good 国家结果保留国家归属并标为 Stale（不计入未知）；超出保留边界则转为未知。已知国家缺少底图代表点不是未知国家。
- 成功空 Peer Snapshot 是权威零；never-observed 或没有可靠分母时保持 unavailable/unknown 且不输出计数。Geo database/国家结果状态与 Peer 采集状态、新鲜度互相独立：Geo Current 不代表 Peer 在线或刚刚采集，采集失败保留的 Peer Snapshot 仍按自身状态展示。
- 每个 Network 的 Public Geo Insight 是独立投影；本契约不提供跨 Network 的全局去重 Peer 总数，也不把缺失 Network 当零或制造完整性百分比。部分覆盖时投影如实标注 scope，Public 只含国家代码与计数，不含原始 IP、RPC Endpoint、精确位置或敏感错误。

---

## 6. Block Summary 与 Block History

### 6.1 Block Summary

保留完整的当前 Block Summary 字段：Node/Network Identity、height/hash/parent hash、链上 block timestamp、Agent `observed_at`、transaction hash count、可选相邻区块间隔、`source`（`subscription` 或 `gap_backfill`）以及 Block Production Attribution。Attribution 中 Coinbase、Seal Signer Match、Protocol Proposer 三者区分；还保留 signer fingerprint、Node key validity/history evidence、seal recovery rule/evidence 和 attribution reason，不得坍缩成单一推断标志。缺乏验证证据时 signer/proposer 显式为 Unknown。**（暂定，Q40：这是唯一保留的显式复杂度权衡，未来可能重议。）**

最新区块通过每 Node 独立的 Head Subscription 触发，按 header hash 完成 Block Resolution（获取并校验区块）。订阅只是正常实时采集触发器；显式恢复计划会对有限高度范围执行 point query。StartupRace、Restart、Reconnect、HeightJump、QueueOverflow、Shutdown 都可以触发有界 Gap Backfill，受高度跨度、查询数量和时间预算限制；普通相邻 head 不会退回轮询。

### 6.2 归属与放置

- Agent 把新采集的 Block Summary 放进 AgentReport 顶层 `block_summaries[]`，把显式缺口放进顶层 `history_gaps[]`；二者按 Node 归属。
- Agent 端只保留有界的 Block Summary 与 History Gap 待上报项，不拥有 Server 的历史高水位或网络级历史。
- Server 从已验证 report 追加近期 Block History，并以 Node 为范围维护 Historical High-Water Mark、coverage intervals、可恢复 Gap coverage、chain divergence 与 resync replay 诊断。
- History 是 best-effort，但不是“从不恢复”：显式 Gap Backfill 只在对应的 open recoverable gap 内接受；超出配置边界的范围会留下 gap。普通 resync replay（低于 high-water mark 且不在 open gap）不重复写入或计数。

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

CLI：`enroll`、`generate-node-id`、`validate-config`、`collect-report`、`run`、`shutdown`、`persist-report`。`enroll` 使用一次性 Enrollment Token 与 Server 建立 Agent 身份并保存凭证；Recovery/Rotation 由 Server/Admin credential seams 提供。

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

[nodes.process]
pid_file = "/var/run/platon.pid"
~~~

Server 不下发或修改 `nodes.rpc_endpoint`。

### 7.2 Agent Store 与 Durable Spool

流程：

~~~text
每 Node 长连接 Head Subscription → 解析、校验并持久化待上报 Block Summary
按 `collection_interval_seconds` 采集当前观测 → 生成完整 AgentReport → 校验 → 写入 Agent Store
独立发送循环 → 最老报告优先 → 校验完整 Report Receipt → 按 per-Node/per-sample 结果应用 → 事务性删除已确认报告
~~~

`collection_interval_seconds` 可配置范围为 1–300 秒，默认 5 秒；`inventory_revision` 默认 1。`[backfill]` 默认 `max_height_span = 256`、`max_block_count = 128`、`max_time_ms = 5000`，边界分别为 1–1,000,000、1–100,000、1–60,000 ms。Durable Spool 默认容量 2 MiB、最大年龄 24 小时，并在约 1.5 MiB 时预留 flush 空间；单个 AgentReport body 的 Server 上限为 8 MiB，Agent 接近 2 MiB 或样本阈值时提前 flush。区块订阅、当前观测采集、报告组装与发送相互解耦；线上仍只使用完整、不可变的 AgentReport，不按数据类型拆分 wire protocol。

要求：

- 报告 bytes 不可变；重试使用相同 `report_id` 与 bytes；
- 投递失败保留原报告；最老报告优先投递；
- Spool 有明确的大小和年龄上限；
- 溢出时丢弃最老的未确认历史报告、记录诊断日志，并保留当前状态的采集与投递；
- Spool 不是用户可见历史；
- Agent Store 损坏时 fail-closed：停止继续伪造报告，等待人工处理。

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

## 8. Server 设计

### 8.1 职责

- Agent 鉴权与 Report Ingestion；
- 所有 Agent 字段的重校验（信任边界）；
- Network Registry；
- Node 当前投影（Current Projection）；
- 有界 Block History 与全局历史窗口；
- Public / Admin API（不同 DTO 与 route group）；
- SSE invalidation；
- 人类登录（Owner 与 Viewer）与 Audit；Site Access Mode 决定 Guest 是否可以匿名读取 Home，Private 模式仍允许已认证 Owner/Viewer；
- Validator/Geo/Peer/Alert/Notification/Operation/Retention/Backup/Restore/Doctor 等 Server-side extensions 及其后台工作器；
- 同源托管 WebUI 静态资源。

Server 不连接 Node RPC、不远程控制、不根据 Agent 输入自动创建 Network、不用 Agent 级视图合并多 Node 链状态。

### 8.2 最小数据模型

Server schema 当前为 41（Agent schema 独立为 13）。物理 SQLite schema 是规范化表族，而不是一个 `node_current_state` 或单一 `block_history` 表；逻辑上至少包括：

~~~text
身份与访问       users / sessions / enrollment_tokens / recovery_tokens / audit_events
Agent 生命周期   agents / agent_credentials / boot + shutdown + spool diagnostics
拓扑与归属       networks / nodes / node_transfers
当前 Component   component_status + host/node process/data/chain/rpc/peer tables
区块与缺口       block_summaries / history state + coverage + gaps / sequence gaps
Peer              current peers / peer capabilities / presence intervals / 5m + 1h aggregates
Geo               geo location cache
Validator         validators / links / current insight / ranking-counter history / daily-monthly analytics
运营扩展         alert rules/incidents/silences/maintenance; notifications; operations; retention; backups/restore
设置与审计       server_settings / audit events
~~~

具体表名、列和迁移顺序以 `crates/platpulse-server/migrations/` 与 `database.rs` 为准；新增扩展不会被伪装成核心 projection。Retention 由 per-family policy 控制，Global History Window 仍是独立的 Block History 设置。

### 8.3 Report Ingestion 事务

1. 验证 Agent Credential；
2. 验证 Agent ID、Epoch、Boot lifecycle、Report ID、sequence 与 body hash；
3. 验证 Node Inventory 与 Node 归属；
4. 验证 Network Identity、Component revisions、Block Summary 和 History Gap 边界；
5. 幂等检查：重复 report 返回同一 Receipt，不重复写投影与历史；
6. 更新 Agent、Host 与各 Node 当前投影；
7. 追加合法的 Block History，记录 coverage/divergence/gap 状态（受全局窗口约束）；
8. 计算 Inventory、per-Node 和 per-sample dispositions，写入完整 Report Receipt；
9. 在同一事务中评估 Alert/Notification side effects；
10. 提交事务；
11. 事务提交后才发布受影响资源的 Admin/Public SSE invalidation。

`partially_accepted` 表示同一个事务中部分 Node/sample 被接受、其余被拒绝，不表示半提交。任一步骤失败都回滚投影、历史、Receipt 和告警副作用；回滚不会发布 invalidation。Post-commit invalidation 是通知层行为，客户端必须用 REST 重新读取权威 DTO。

### 8.4 HTTP API

运行时路由按三个独立 group 组织；下面是当前能力边界（`docs/openapi/openapi.json` 是由已注解 operation 生成的 DTO/客户端参考，不替代运行时路由注册）：

**Agent group（Agent Credential；需要 TLS 或可信 HTTPS proxy）：**

~~~text
GET  /api/agent/v1/time
POST /api/agent/v1/enroll
POST /api/agent/v1/recover
POST /api/agent/v1/reports
~~~

`/time` 与 `/reports` 是运行时 Agent 路由；当前 OpenAPI path 注册覆盖 enrollment/recovery，但这两个 handler 尚未完整纳入生成 spec，Agent 集成应以运行时路由和 wire types 为准。

**Public group（Guest 只在 Public Site Access Mode 下进入允许匿名的读路径；Owner/Viewer Session 可读 Private Home）：**

~~~text
GET  /api/public/v1/access
GET  /api/public/v1/events
POST /api/public/v1/login
POST /api/public/v1/logout
GET  /api/public/v1/session
GET  /api/public/v1/networks
GET  /api/public/v1/networks/{network_key}
GET  /api/public/v1/nodes/{node_id}
GET  /api/public/v1/nodes/{node_id}/history
GET  /api/public/v1/nodes/{node_id}/history/export
GET  /api/public/v1/nodes/{node_id}/metrics
GET  /api/public/v1/nodes/{node_id}/peer-history
GET  /api/public/v1/validators/{validator_id}/history
GET  /api/public/v1/validators/{validator_id}/analytics
~~~

`peer-history` 是选定 Node 的聚合历史，不存在 Network-wide 的替代 endpoint。Public `networks` 列表只由有 Active Node 的 Network 产生；Network detail 在没有 Active Node 时返回 `404 not_found`，不是 `200 {nodes: []}`。

**Admin group（Human Session + Owner role）：**

~~~text
/api/admin/v1/overview
/api/admin/v1/agents[/{agent_id}][/audit|/recover|/credentials/*]
/api/admin/v1/agents/enroll-token
/api/admin/v1/nodes[/{node_id}][/{history|peer-history|peer-churn|metadata|visibility|transfers|validator-links}]
/api/admin/v1/networks[/{network_key}][/validators]
/api/admin/v1/people[/{user_id}][/role|/status|/reset-password]
/api/admin/v1/sessions[/revoke-others|/{session_id}/revoke]
/api/admin/v1/access-mode
/api/admin/v1/history-window[/impact]
/api/admin/v1/audit
/api/admin/v1/events
/api/admin/v1/geo
/api/admin/v1/validators[/{validator_id}][/history|/analytics]
/api/admin/v1/validator-links[/{link_id}][/end]
/api/admin/v1/transfers/{transfer_id}[/cancel]
/api/admin/v1/alerts/*
/api/admin/v1/notifications/*
/api/admin/v1/operations/*
/api/admin/v1/retention/*
/api/admin/v1/backups/*
/api/admin/v1/restore/*
/api/admin/v1/doctor
~~~

这些 family 表示当前实际 route 集合；每个 operation 的 GET/POST/PUT/PATCH/DELETE 方法、参数和响应以源 handler/OpenAPI operation 为准。Session 撤销是 `POST /api/admin/v1/sessions/{session_id}/revoke`，不是 DELETE。运行时 handlers 还可能返回 OpenAPI 未列出的 typed `ApiErrorBody`（特别是 503/500），客户端必须把任何非 2xx 当作错误处理。

### 8.5 Admin Overview 契约

`GET /api/admin/v1/overview` 是 Owner 的当前分诊响应，包含 `generated_at`、`summary` 与当前 `attention[]` 队列；不包含完整 Node Detail、完整 Agent Detail、历史图表或远程操作命令。Attention 与 summary 在同一响应中返回，但当前实现通过独立数据库读取组合，不承诺跨资源的同一时刻原子快照。

`summary` 由 Server 派生，逻辑分组为：

```text
summary.agents: total, online, offline, unknown
summary.nodes: total, active, healthy, unhealthy, unknown, retired
summary.networks: total, with_identity_mismatch
```

不变量为 `nodes.active = nodes.healthy + nodes.unhealthy + nodes.unknown`、`nodes.total = nodes.active + nodes.retired`。Retired Node 不参与实时健康分桶或当前 Attention Item。当前 `AdminOverviewSummary` 没有 `published` 字段；Node list/detail DTO 仍保留 `visibility` 与对应 Owner mutation/filter 作为 legacy compatibility/diagnostic surface。Public 查询不使用它过滤 Home，Site Access Mode 是匿名访问范围的唯一有效站点级开关。

`AttentionItem.kind`、`severity` 与 `subject_kind` 是 typed Server contract，不是浏览器任意字符串。`subject_kind` 限定为 `agent`、`node`、`network`、`settings`。当前 kind 为 `agent_offline`、`agent_spool_fatal`、`agent_spool_overflow`、`agent_report_gap`、`agent_security_event`、`agent_shutdown_incomplete`、`node_unhealthy`、`node_health_unknown`、`node_resync`、`node_identity_mismatch`。severity 只有 `critical` 与 `warning`：Spool fatal/overflow、security event、unhealthy Node 与 Network Identity Mismatch 为 Critical；Agent offline、report gap、incomplete shutdown、unknown Node health 与 resync 为 Warning。新 Agent 在尚无 accepted report 时是 Unknown 而非 Offline；新 Active Node 在 Server-owned first-observation grace period 内是 Starting，不提前产生 unknown-health Attention。Server 按 Critical 优先、权威观察时间与稳定身份排序；WebUI 可以按 Subject 分组展示，但不能丢弃 Item 或重算 severity。

Public 与 Admin Projection 都由 Server 计算 Health，浏览器不得自行重算；但当前实现是 route-specific precedence，而非一个可复用的 canonical evaluator：Public `health_for` 会纳入 process/identity 错误、RPC、Sync、Consensus 与 freshness，Admin `derive_health` 主要基于 RPC、Sync、Consensus 与 freshness，identity mismatch 另作为 Admin attention/identity disposition。因而同一 Node 在两种 DTO 中可能有不同的主标签；两边都必须保持 Unknown、Stale、Disabled、Unsupported 与从未观察输入不因缺省而成为 Healthy。

当前 SPA 对未注册的扩展页面使用 Admin fallback；Server/API 可以先于页面实现，但不得把 fallback 误写成可用页面。

---

## 9. WebUI

页面与交互契约的权威文档是 `docs/design/webui.md`；此处只列当前边界和路由事实。Server 扩展 API 存在不代表 `platpulse-web` 已注册对应页面。

### 9.1 Home

只读 Public Projection，Network → Node → Node Detail：

- Network 列表；
- Network 概览（Active Node 列表/卡片）；
- Node Detail：一个 Komari 风格的紧凑主卡片展示 Node 名称、Node Health Summary、Node status、独立 Validator role、进程运行时间、PlatON 进程 CPU、进程内存占比、Node Data 大小/容量、`HEAD / QC / LOCKED / COMMITTED / VALIDATOR`、进程启动时间和 Agent 最后上报时间；主卡片使用中性细边框，不显示彩色顶部/边缘色条。CPU、Memory 与 Node Data 在主卡片内沿用 Home Node 卡片的紧凑当前值和进度条层级。Details 仅展示四张等高、缩小内边距与图表高度的一分钟图表卡片：Host 网络上下行、Peer 连接数、最近连续区块间隔与最新 Block Summary 交易数；Network 与 Connections 使用折线图，Block time 与 Transactions 使用柱状图；不伪造中间点，不以 0 替代未知值。进程内存占比使用该进程 RSS 除以所属 Host 总内存；不展示 Bounded Block History 列表或历史导出。

Home 不展示：凭证、RPC Endpoint 原文、内部错误堆栈、Agent/Host 拓扑、任何操作入口。已退休/已删除/未知 Node 使用不泄漏信息的 unavailable 文案。Site Access Mode 为 Private 时 Home 路由要求已认证 Owner 或 Viewer；为 Public 时允许匿名 Guest 读取允许的 Public projection 路径。

### 9.2 Admin

认证后使用，但 Admin 路由由 Owner role guard 保护，Viewer 只能访问 Home：页面组为：

1. Overview；
2. Agents；
3. Nodes；
4. Networks；
5. Settings（按顺序包含 History Window 与 Site Access Mode）；
6. Sessions 与 Audit。

Server Admin API 另有 People、Validator、Alert、Notification、Operation、Retention、Backup/Restore、Doctor、Transfer 和 Agent credential operations；当前 SPA 没有对应注册路由。

Settings 是当前 SPA 全局配置的唯一 canonical route（`/admin/settings`）；旧的 `/admin/history-window` 与 `/admin/site-access` 不重定向，而是进入 Admin 的 Section not found fallback。Server API 仍分别提供 `/api/admin/v1/history-window` 与 `/api/admin/v1/access-mode`。

Admin 的 Node 页面聚焦配置与诊断（显示名、RPC Endpoint 诊断、Node Inventory/生命周期、freshness 摘要），不复刻 Home 的完整 Node Detail。Admin 不执行远程 Node 操作。

### 9.3 REST / SSE / 响应式

- REST 是唯一权威数据来源；SSE 只发 invalidation/reset 信号，不携带完整业务 DTO；
- 收到 invalidation 后 WebUI 重新读取对应 REST 资源；
- Home 与 Admin 查询缓存隔离；权限变化先清敏感缓存再重新验证；
- 必须在 360×800、390×844、768×1024、1280×800 可用：无水平溢出、表格降级为卡片、键盘可完成登录/导航/主要操作、状态不只靠颜色、Reduced Motion 下不依赖动画表达状态。

---

## 10. 身份与安全

### 10.1 Agent 身份

- Agent Credential 由 Server 的 Enrollment/Recovery/Rotation seams 发行和管理；Agent 通过一次性 Enrollment Token 获取并保存凭据，凭据只能访问 Agent API；
- Server 不保存 Credential 明文；凭据文件 Agent 用户可读、其他用户不可读；
- Enrollment、Recovery、Rotation 和 Revoke 都已提供相应 credential seam：Recovery 额外推进 Agent Epoch 并拒绝旧 Epoch 报告，Rotation 只更新凭据集合（可保留短暂 overlap），Revoke 只立即使指定凭据失效；当前 SPA 不提供 credential 管理页面，但 Server/Admin API 已提供 token、recover、rotate、revoke seam；
- 不静默创建相同身份的第二个 Agent；
- Human Session 不能访问 Agent API。

### 10.2 Human 身份

当前支持两种人类主体：Owner 与 Viewer（初始化/CLI 或 Owner Admin 可创建）。Viewer 可登录并读取 Home，即使 Site Access Mode 为 Private；只有 Owner 可访问 Admin。支持 Session Cookie、Argon2id 密码哈希、登录限流、Session 撤销、基础 CSRF/Origin 校验。匿名 Guest 的 Home 可读性由 Site Access Mode 决定（默认 Private）。

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
- Public WebUI SSE 连接中显示 `Connecting to live updates`，连接成功显示 `Live updates connected`，断开显示 `Live updates paused`，并通过 REST 重新获取；这些传输状态不代表 Node 健康或观测新鲜度。Admin 保持紧凑的既有 `Starting`/`Current` 传输文案。

### 11.4 数据边界

当前观测/历史数据按用途分为多类，而不是只有两种：

~~~text
Current Projection   Agent/Node/Host/Peer/Validator 的最新有效状态（含 LastGood）
Block History        全局 History Window 约束的近期 Block Summary（best-effort）
Gap/Divergence       有界 History Gap、coverage、resync 与 chain-divergence 证据
Peer History         Peer Presence 与 5m/1h aggregate（按 retention policy）
Validator History    ranking/counter history、daily snapshots、monthly aggregates
Metric History       Node metric history/export（按各 family policy）
Operations/Audit     Alert/Notification/Operation/Backup/Restore/Doctor 与审计记录
~~~

Server 仍不会用零值填充缺失区间；Retention 按 data family 分别配置，Block History Window 是其中独立的一项。

---

## 12. 部署

- Agent：Linux x86_64/aarch64，建议 systemd；每 Host 一个 Agent；独立 state directory；credential 与 SQLite 严格权限；只需访问本地 RPC Endpoint 与 Server HTTPS 地址。
- Server：单进程、单 SQLite、同源托管 WebUI；`/health/live` 只判 event loop 存活，`/health/ready` 同时检查 `sqlite`、`owner`、`web_assets`、`shutdown`、`critical_workers`、`corruption` 六个组件，并以 200/503 表达整体结果；Backup/Restore 是显式运维流程，不是启动流程。
- WebUI：React + Vite 构建为静态资源，由 Server 同源托管，生产环境不单独运行 Node.js。

---

## 13. 当前实现验收基线

### 13.1 采集与上报

- 一个 Agent 配置两个 Node 可同时采集；
- 一个 Node RPC 失败时另一个 Node 仍能上报；
- Host Observation 只采集一次；
- Agent 重启后未投递报告不丢失；
- Server 收到重复报告不重复写投影与历史；
- Receipt 校验失败时 Agent 保留原报告。

### 13.2 区块历史

- 最新/新采集的 Block Summary 位于 AgentReport 顶层 `block_summaries[]`，含高度、Hash、时间、交易数量与 Block Production Attribution；`nodes[]` 只承载 Node current observations；
- Server 从已验证 report 中的 accepted Block/sample facts 追加 Block History；`partially_accepted` 不等于整份 report 丢弃；
- 历史窗口缩短后过期历史被异步删除；显式 History Gap 记录保持有界且去重；
- Network Identity Mismatch 时历史不并入注册 Network。

### 13.3 Server 与权限

- Owner 未初始化时 Server 报告 setup required；
- Agent Credential 不能访问 Human API，Human Session 不能访问 Agent API；
- Site Access Mode 为 Private 时未登录请求不能读 Home API，已认证 Owner/Viewer 可读 Home；为 Public 时匿名 Guest 可读允许的 Home 路径，Admin 仍需 Owner 登录；
- 所有 mutation 有基础 CSRF/Origin 校验与 Audit；
- 历史窗口与 Site Access Mode 变更记录 old/new 值与操作者，边界外的值被拒绝。

### 13.4 WebUI

- Home 从 Network 列表进入 Node Detail；站点 Private 时 Home 要求 Owner/Viewer 登录，Public 时允许匿名 Guest 读取；
- Node Detail 使用无彩色边缘条的 Komari 风格紧凑首卡片，显示当前 Health、Node status、独立 Validator role、进程运行时间、PlatON 进程 CPU/Memory、Node Data、Head/QC/Locked/Committed/Validator；Details 展示四张等高紧凑卡片，其中 Network 与 Connections 使用折线图，Block time 与 Transactions 使用柱状图；Network tab 还展示选定 Node 的聚合 Peer History；Bounded Block History 不在页面展示，最近两个连续 Block Summary 仅用于计算区块间隔；
- Admin 的 Node 页面不复制 Home 的完整 Node Detail；
- SSE 断开显示 `Live updates paused`；invalidation 后通过 REST 重取；
- 360px 无水平溢出；Unknown、Stale、Error 不被渲染成 Healthy。

---

## 14. 设计原则

PlatPulse 当前实现是：

~~~text
轻量 Agent + 可靠 Report Spool
+ 一个中心 Server
+ SQLite 规范化当前/历史/运营投影 + 有界 Block History
+ 一个清晰的 Web Dashboard（当前 Home 只读 / Admin Owner 配置）
+ 独立的 Validator、Geo、Peer、Alert、Notification 与运维扩展边界
~~~

任何新增设计都必须先回答：

1. 是否直接服务于 Node 监控主链路？
2. 是否必须进入 Agent、Server 或 WebUI 的核心边界？
3. 延后是否会阻塞 Agent → Server → WebUI 链路？
4. 是否可以作为独立扩展，而不是提前进入核心协议与数据库？

未来扩展必须满足：不改变核心边界、不向核心协议塞字段、不创建没有真实场景支撑的抽象，并有独立的设计、测试与回滚边界。
