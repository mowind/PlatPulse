# PlatPulse 产品与技术设计（当前实现基线与已确认演进）

## 1. 文档状态

- 状态：§1–§14 保留当前实现与边界基线；[§15](#accepted-management-target)记录已确认、部分实现的演进——已实现部分以对应 GitHub Issue 为准，未实现部分不能据此宣称新 API、页面或迁移已交付。实现、迁移、运行时路由和本文件应相互校验。
- §15.10 记录 issue #182 已接受的 Server 托管 Inventory Revision 目标。v2 协议、Server 分配与 Agent 确认已由 issue #186 在隔离的新部署链路上实现；存量迁移的准备、协调检查点（issue #189、#190）与离线基线转换/配置迁移/离线校验（issue #191），以及协调生产切换、v1 仅重放及恢复业务写入前的检查点回退（issue #192）均已实现。v1 手工 revision 与 §15.9 守卫继续服务于显式 opt-in 的 v1 入口。
- 适用范围：`platpulse-core`、`platpulse-agent`、`platpulse-server`、`platpulse-web`。
- 领域术语：以仓库根目录 [CONTEXT.md](../../CONTEXT.md) 为准；词汇表已纳入本次确认的目标语义。§1–§14 中旧的 Inventory 生命周期、手工 Validator Link 和未确认提示描述是实现基线；涉及本次变化时以 §15 的目标契约为准，不把两种状态混写。
- 规范性用词：
  - 必须：实现和验收不可省略；
  - 不允许：违反即破坏边界或领域不变量；
  - 可以：允许实现，但不是当前核心链路的必需能力；
  - 明确未实现：当前代码和运行时边界均未提供，不应在页面、API 或数据模型中暗示已存在。
- 本文件区分三种状态：已实现的 Server/Agent/API 能力、当前 `platpulse-web` 实际注册的路由，以及明确未实现的产品边界。Validator、Geo、Peer、Alert、Notification、Backup/Restore、Retention、Operation、Node Transfer、Recovery/Rotation 等不再统一视为“未来功能”：其中 Server/API 和后台工作器已有实现，部分扩展已注册 SPA 页面（例如 Settings 中的 Geo provider），其余仍只有 API；详见 §3、§8、§9 和 `docs/design/webui.md`。

---

## 2. 产品形态

产品分离参照 Komari（<https://github.com/komari-monitor/komari>），但监控对象是 PlatON Node 而不是服务器：

- Home：只读、以 Node 为中心的监控面。根路由 `/` 以 All Networks 展示 Active Node 卡片，并进入 Node Detail 展开公共投影。Node Detail 由最近两个连续 Block Summary 推导出块间隔，但不展示 Bounded Block History 列表。Site Access Mode 为 Public 时匿名 Guest 可读选定 Public GET/SSE 路径；为 Private 时 Owner 或 Viewer 登录后可读。
- Admin：认证后的 Owner-only 系统概览与配置面。当前 SPA 路由覆盖 Overview、Agents、Nodes、Networks、Settings、Sessions 与 Audit；Settings 现在包含 Geo provider 选择（Disabled / Local MMDB / IPinfo / GeoJS）。Server/Admin API 另外提供 Validator、Alert、Notification、Operation、Retention、Backup/Restore、Doctor、Transfer、People、Enrollment/Recovery/Rotation 等能力，但尚未全部注册为页面。
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
│ - Home：All Networks → Node → Node Detail     │
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

当前代码和迁移已经提供：Agent Enrollment、Recovery、Credential Rotation/Revocation、Boot/Shutdown 生命周期；Peer Snapshot、Peer Presence 与聚合历史；provider-keyed country cache 与后台国家解析（Settings 可选择 Disabled / Local MMDB / IPinfo / GeoJS）；Server-side Validator Registry/Links/PlatScan Provider、历史与日/月 analytics；Typed Alert/Incident/Silence/Maintenance；at-least-once Notification Delivery；Node Transfer；Retention、Operation、Backup/Restore、Doctor；独立 Prometheus metrics listener。它们由 Server/Admin API、CLI 或后台工作器提供，是否有当前 SPA 页面由 `docs/design/webui.md` 的路由矩阵单独决定。

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

以下为当前实现；已确认的 Agent Removal / Node Purge 及删除后的接收边界见 §15.2–§15.3。Retired 与 Purged 不可混用。Server 托管声明版本见 [§15.10](#server-managed-inventory-revision-target)，不改变本地配置所有权。

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

5m/1h country 聚合与 §5.8 的当前国家投影共用同一条保留规则：缓存行仍在硬保留边界内时按其最后已知国家解析（超出 24h TTL 的算 last-good），超出边界才计为未知。两者因此对同一缓存得出一致的 known/unknown 口径。

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
- 国家解析只在后台执行：Report Ingestion 事务只记录当前 Peer 引用（并更新最后引用时间），不读 MMDB、不做外部 HTTP；`geo_backfill` 负责调度、按规范化公网 IP 去重、有限并发、有界批量（`MAX_BACKFILL_BATCH`）、无国家结果的小时级重试间隔，以及每轮结束后的 realtime 失效。慢查询、外部服务慢或被限流都不会占用 Receipt 事务，也不影响报告接收与就绪状态。Provider 只决定"单个地址如何解析"：Local MMDB 在阻塞线程读文件（`MAX_BACKFILL_CONCURRENCY`），每个 External Geo Provider 在有界外发边界发 HTTP（`GEOJS_MAX_CONCURRENCY` / `IPINFO_MAX_CONCURRENCY`，均低于本地并发）；调度、去重、保留、generation 校验与缓存写入由所有 Provider 完全共用，所以切换供应商只改变目的地与字段拼写，不改变统计对象。
- 结果按 Geo Provider 与规范化 IP 隔离存放在 `geo_location_cache`：国家代码、状态（`current` / `no_country` / `failed`）、最近尝试时间、最近成功时间、出生时间、有效期与最后引用时间。投影与 5m/1h country 聚合只读当前 provider 的行；切换 provider 后旧 provider 的行立即删除，未被任何当前 Peer 引用的行在维护周期内清理，超出 24h TTL 但仍在 30 天硬边界内的结果继续作为 last-good Stale，失败只记录尝试、绝不把 last-good 刷成 Current，超出边界则回到未知。
- 每个 External Geo Provider 都只走自己的固定 HTTPS 端点：IPinfo 为旧式 `https://ipinfo.io/{ip}/json`（读取 `country`，不替换为 Lite API），GeoJS 为 `https://get.geojs.io/v1/ip/geo/{ip}.json`（读取 `country_code`）。两者都不携带 token、不提供凭据 UI，也不把文档或定价页的免费额度当作供应商承诺。只发送经 Server 校验的规范化公网字面 IP：私网／loopback／link-local／保留地址在外发边界再次拒绝，主机名从不解析 DNS，请求目的地不可配置，重定向一概不跟随（`redirect::Policy::none()`）。边界包含有界超时、有界响应体、有限并发与有界重试/排队，机制由 `geo_external` 共用，每个 Provider 只提供编译期 profile（目的地、国家码字段、界限、稳定理由与署名）。
- 结果解释由共享外发边界统一处理并区分四类：国家码字段存在且合法是 `Country`，字段缺失、为 null 或为空串是权威 `NoCountry`，出现但不是两字母 ASCII 国家码是畸形结果（按失败处理），429 是 `RateLimited`；非成功 HTTP、传输失败、超时、超长响应体、非 JSON 与"成功但非 JSON 对象"都是失败。只有国家码字段被读取，provider 差异在这一层适配：IPinfo 读 `country`，GeoJS 读 `country_code`（GeoJS 的 `country` 是英文全名，任何情况下都不当国家码读取）。国家码只做形状校验（两个大写 ASCII 字母），与 Local MMDB 路径共用同一个过滤器：字段按 ISO 3166-1 alpha-2 定义，本 Server 不固化一份会过期的国家清单，畸形值在所有路径上都成不了国家。
- 429 不写入任何按地址的尝试记录：地址保持 pending，改由有界的进程内 Provider 级退避（60s 起、翻倍、上限 15 分钟，任何正常响应即清零）决定重试窗口；写入路径本身也拒绝 `RateLimited`，即使将来有调用方绕过调度也不会留下"失败尝试"。非成功 HTTP、传输失败、超时、超长响应体与畸形 JSON 都按失败记录，只记尝试、绝不擦除 last-good。
- 外部 Provider 的状态是外发路径的真实状态：每个 Provider 各自记录最近一次外发是否产生了可用结果与限流窗口，被限流或最近一次尝试无可用结果时 Admin 与 Public 都报 `error`（Public 只给固定非敏感理由），下一次正常响应即回到 `current`。退避窗口与失败标志按 Provider 隔离，切换供应商不会让 A 的迟到交换决定 B 的窗口或状态，诊断中的 `rate_limited_until` 也取自当前选中的 Provider。因为 External Geo Provider 不读数据库，它们的诊断不含 build epoch、digest 或"数据库加载时间"，`last_success_at` 来自按 Provider 隔离的缓存行。
- 每次后台写入在事务内重新校验持久化 selection 与进程内 generation：配置变更后迟到的任务不能回写成新配置的结果。cleanup 只应用硬保留边界、当前引用规则与容量上界：24h 过期只把结果标为 Stale last-good，绝不删除仍在硬边界内的国家结果。
- 管理接口：`PUT /api/admin/v1/geo/provider`（Owner-only、Origin/JSON/CSRF、审计 `geo_provider_changed`）在事务内推进 generation 并返回新的诊断；`POST /api/admin/v1/geo/refresh`（同样 Owner-only、Origin 与 CSRF 校验、审计 `geo_refresh_requested`；与其它无请求体的管理动作一致，不要求 JSON Content-Type，因为不存在可被跨站表单伪造的请求文档）触发全局强制刷新；`GET /api/admin/v1/geo` 提供 provider、generation、可选列表（含 `sends_peer_addresses`）、数据库状态、缓存国家数、完整待解析队列长度、最近成功时间与外部 Provider 的 `rate_limited_until`，并附带 `refresh`（最近一次强制刷新的运行态与计数）与 `refresh_unavailable_reason`（Disabled／不可用时的稳定说明），均不含路径、原始 IP、请求 URL 或供应商错误原文。Provider 为 Disabled 但本地数据库已配置且不可用时，诊断仍显示该数据库的错误说明，Owner 在选择前即可看到。
- 全局强制刷新（issue #136）是真实的重新解析而不是清缓存：范围在 run 开始时冻结为全部 Networks 的 `current_node_peers` 中当前引用的、经信任边界校验的规范化公网 IP，按地址去重，与 Home 筛选无关，不翻查无引用历史，也不向 Agent 下发任何采集或控制命令；之后新增的 Peer 引用仍由常规后台路径处理，所以运行中的分母不会漂移。每个地址绕过 24h 有效期真正重新解析，并通过与自动路径完全同一条 generation 校验的缓存写入落库（`geo_refresh` 与 `geo_backfill::run_pass` 共用 `resolve_address`、去重、有限并发与保留规则），因此 IPinfo 与 GeoJS 无需新增 provider 分支即自动获得相同行为；只有清缓存、只返回 accepted 或等待下一次 Agent 上报都不构成完成——范围内全部地址都得到权威结果（成功／无国家／失败）才进入 `completed`。同一 provider+generation 的重复提交加入同一个 run（响应 `started: false`），不同配置的请求先把旧 run 标记为 superseded；同一时刻只有一个 run，运行期间常规后台 pass 只让出该 run 冻结范围内、尚未完成的地址，其余引用（包括运行中新增的 Peer）继续按既有节奏解析，因此一个地址不会被两条路径同时解析，中途新增的引用也不会被推迟到 run 结束。run 结束时主动唤醒后台路径。计数一律按地址查询：`total_lookups` / `completed_lookups` / `resolved_lookups` / `no_country_lookups` / `failed_lookups` / `rate_limited_lookups`，并单独给出 `peer_records_in_scope`（当前引用这些地址的 Peer 记录数），与地图按 Node 计数 Peer 记录的口径并列而不互相推算；一个地址服务多条 Peer 记录时两个数字保持不同。provider/generation 变化、停用、限流或 Server 停机都会真实终止：终态为 `aborted` 并带稳定、无路径的 `abort_code`/`abort_reason`，未查询的地址保持 pending，绝不虚报成功；429 只计入 `rate_limited_lookups`、绝不当作失败结果或 completed，迟到任务的结果继续被 generation 校验拒绝。终态只存在于进程内，重启后不保留"进行中"的假象；刷新与自动补全的结果都通过既有 `geo` realtime 失效通知 Admin 与 Public。
- Public 仍然只见国家代码、计数、Provider 自有的稳定状态与署名：署名随所选 Provider 变化（Local MMDB 为 MaxMind 署名，IPinfo 为 IPinfo 署名，GeoJS 为 GeoJS 来源加 MaxMind 原句，Disabled 无署名），浏览器不自行拼接；外部 Provider 的失败理由对 Public 只呈现固定非敏感文案，切换 Provider 后旧来源的结果不会以新 Provider 的名义出现（退出选择的 Provider 的行会被立即清理）。
- IPinfo 票（#134）不含 GeoJS、全局强制刷新与第三方 MMDB 自动下载；GeoJS 票（#135）复用同一条后台执行路径与 provider 隔离把它接通；全局强制刷新（#136）在同一条执行路径上实现，未新增 provider 分支、未新增分叉调度。浏览器 E2E 只验证两个外部 Provider 选项、各自的知情说明、Disabled 时刷新不可用的明确说明，以及 Local MMDB fixture 下的真实强制重查；绝不在真实浏览器会话里选中或触发外部 Provider：E2E 用的 Server 没有（也不允许有）可配置目的地，选中即意味着把 Peer 地址发给真实第三方，违反"CI 不向供应商发送真实 Peer IP"。外发行为（成功、无国家、畸形响应、HTTP 失败、429、超时、恢复、慢响应、切换与迟到结果）以及强制重查绕过有效缓存、部分失败计数、限流终止与 Peer/IP 双口径，由进程内的确定性替身与真实 MMDB fixture 覆盖。
- IPinfo 旧式接口的核查结论（2026-08，issue #134）：该无 token 路径在官方文档中与 Lite API 并列存在，官方支持文档称无账号公共 API 为每源 IP 每天 1,000 次、旧式 Free API 为每月 50,000 次，两者口径不一致，因此本实现只把它当作有界、可失败的外部依赖，不承诺额度、不规避限流。官方条款允许为内部业务目的使用查询内容，禁止转售或再分发，付费档位标注 `Attribution required`，故 Public 在 IPinfo 下展示 IPinfo 署名。未实测项：未对真实服务发起联调（需另行授权），因此响应字段、限流响应头与真实限额均未验证；实现只依赖已确认的 `country` 字段与 429 状态码。

- GeoJS 接口与许可核查结论（2026-09，issue #135，详见 `docs/research/geojs-provider.md`）：固定 HTTPS 端点 `https://get.geojs.io/v1/ip/geo/{ip}.json`；无鉴权、无 token；文档声明当前无限流但不承诺额度，TOS 禁止"excessive"使用并保留限流与封禁权，因此实现只把它当作有界、可失败的外部依赖。二字母代码在 `country_code`（`country` 是英文全名，不读取）；**无国家地址返回 HTTP 200 且国家相关键整体缺失**，这与 IPinfo 的"字段缺失/为空"语义一致，因此共享边界把"对象缺少国家码字段"判为权威 NoCountry；畸形 IP 返回 404 HTML，本 Server 只发送经校验的规范公网字面量所以不可达，即便出现也按失败处理。同族的 `/v1/ip/country/` 端点字段名与空值语义都相反，不得与之互换。GeoJS 自身 TOS 无署名条款，但其数据来源为 MaxMind GeoLite，故 Public 署名同时给出 GeoJS 来源与 MaxMind 要求的原句。未实测项：未对真实服务发起联调（需另行授权），真实限流行为、响应头与所服务的 GeoLite 数据版本均未验证；GeoJS 背后法律实体无一手来源，不作事实陈述。

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

`collection_interval_seconds` 可配置范围为 1–300 秒，默认 5 秒；`inventory_revision` 默认 1。发送循环对**单次 HTTP 发送**施加 `sender_deadline_ms`（默认 5000 ms），它约束的是 Agent 等待 Server 响应的时间，而不是 Server 处理一份 report 的成本：deadline 到期后 Agent 放弃本次响应，但 Server 侧已经开始的事务可能仍然提交，因此该 report 可能已经落地。`[backfill]` 默认 `max_height_span = 256`、`max_block_count = 128`、`max_time_ms = 5000`，边界分别为 1–1,000,000、1–100,000、1–60,000 ms。Durable Spool 默认容量 2 MiB、最大年龄 24 小时，并在约 1.5 MiB 时预留 flush 空间；单个 AgentReport body 的 Server 上限为 8 MiB，Agent 接近 2 MiB 或样本阈值时提前 flush。区块订阅、当前观测采集、报告组装与发送相互解耦；线上仍只使用完整、不可变的 AgentReport，不按数据类型拆分 wire protocol。

要求：

- 报告 bytes 不可变；重试使用相同 `report_id` 与 bytes；
- 投递失败保留原报告；最老报告优先投递；
- Spool 有明确的大小和年龄上限；
- 溢出时丢弃最老的未确认历史报告、记录诊断日志，并保留当前状态的采集与投递；
- Spool 不是用户可见历史；
- 投递余量必须真正收敛：只要队列里多于一份报告，发送循环就按批清空，使追赶吞吐高于采集速率；否则投递速率恰好抵消采集速率，重启后的积压会永久滞留在 Spool 中；
- Agent Store 损坏时 fail-closed：停止继续伪造报告，等待人工处理。

`sender_deadline_ms` 与重启恢复的关系：deadline 只覆盖**单次 HTTP 发送**，不覆盖 Server 处理一份 report 的成本，也不覆盖 Server 的重启窗口（本机 1.19 GB 数据库的实测重启窗口约 3 s，且此时 Listener 尚未就绪，Agent 的失败只是重试，报告仍留在 Spool 中）。因此 deadline 不应随恢复期长短调整：只要稳定态每报告处理时间远小于 deadline，追赶就不会因为 deadline 而反复回滚已经完成的工作。

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

本节及上节 wire 示意为当前 v1 基线；v2 移除 Agent 声明中的 revision、在 Receipt 返回接受版本，见 [§15.10](#server-managed-inventory-revision-target)。v2 已实现为独立协议 major 与独立路由，向冻结 v1 直接增添字段仍被禁止。

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

Server schema 当前为 56（Agent schema 独立为 15）。物理 SQLite schema 是规范化表族，而不是一个 `node_current_state` 或单一 `block_history` 表；逻辑上至少包括：

~~~text
身份与访问       users / sessions / enrollment_tokens / recovery_tokens / audit_events
Agent 生命周期   agents / agent_credentials / boot + shutdown + spool diagnostics
拓扑与归属       networks / nodes / node_transfers
当前 Component   component_status + host/node process/data/chain/rpc/peer tables
区块与缺口       block_summaries / history state + coverage + gaps / sequence gaps
Peer              current peers / peer capabilities / presence intervals / 5m + 1h aggregates
Geo               provider-keyed country cache (geo_location_cache) + server_settings selection
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
10. 提交事务（Geo 国家解析不在事务内，提交后只唤醒后台解析路径）；
11. 事务提交后才发布受影响资源的 Admin/Public SSE invalidation。

`partially_accepted` 表示同一个事务中部分 Node/sample 被接受、其余被拒绝，不表示半提交。任一步骤失败都回滚投影、历史、Receipt 和告警副作用；回滚不会发布 invalidation。Post-commit invalidation 是通知层行为，客户端必须用 REST 重新读取权威 DTO。

### 8.4 HTTP API

运行时路由按三个独立 group 组织；下面是当前能力边界（`docs/openapi/openapi.json` 是由已注解 operation 生成的 DTO/客户端参考，不替代运行时路由注册）：

**Agent group（Agent Credential；需要 TLS 或可信 HTTPS proxy）：**

~~~text
GET  /api/agent/v1/time
GET  /api/agent/v1/preparation
POST /api/agent/v1/enroll
POST /api/agent/v1/recover
POST /api/agent/v1/reports
POST /api/agent/v2/reports
~~~

`/time`、`/reports`、`/api/agent/v2/reports` 与 `/preparation`（issue #189 的只读 v1 升级准备基线）是运行时 Agent 路由；当前 OpenAPI path 注册覆盖 enrollment/recovery，但这些 handler 尚未完整纳入生成 spec，Agent 集成应以运行时路由和 wire types 为准。

**Public group（Guest 只在 Public Site Access Mode 下进入允许匿名的读路径；Owner/Viewer Session 可读 Private Home）：**

~~~text
GET  /api/public/v1/access
GET  /api/public/v1/events
POST /api/public/v1/login
POST /api/public/v1/logout
GET  /api/public/v1/session
GET  /api/public/v1/networks
GET  /api/public/v1/nodes/{node_id}
GET  /api/public/v1/nodes/{node_id}/history
GET  /api/public/v1/nodes/{node_id}/history/export
GET  /api/public/v1/nodes/{node_id}/metrics
GET  /api/public/v1/nodes/{node_id}/peer-history
~~~

`peer-history` 是选定 Node 的聚合历史，不存在 Network-wide 的替代 endpoint。Public `networks` 列表只由有 Active Node 的 Network 产生。

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
/api/admin/v1/geo[/provider|/refresh]
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

这些 family 表示当前实际 route 集合；每个 operation 的 GET/POST/PUT/PATCH/DELETE 方法、参数和响应以源 handler/OpenAPI operation 为准。备份创建与恢复是**离线操作**（[ADR 0008](../adr/0008-offline-server-backup.md)）：服务进程不创建 backup artifact，因此 `/api/admin/v1/backups` 只保留列表/详情/verify，`POST /api/admin/v1/backups` 随 `backup_create` Operation 一起移除。Session 撤销是 `POST /api/admin/v1/sessions/{session_id}/revoke`，不是 DELETE。运行时 handlers 还可能返回 OpenAPI 未列出的 typed `ApiErrorBody`（特别是 503/500），客户端必须把任何非 2xx 当作错误处理。

### 8.5 Admin Overview 契约

以下为 §8.5 的当前实现。§15.6 的 Attention Acknowledgment 已由 issue #172 交付：`GET /api/admin/v1/overview` 的 Agent Item 现在带 Server-owned 的 occurrence/evidence 边界（`evidence_key`），已确认的同一次发生不再出现；它不是浏览器隐藏规则或 Alert Incident 恢复。

`GET /api/admin/v1/overview` 是 Owner 的当前分诊响应，包含 `generated_at`、`summary` 与当前 `attention[]` 队列；不包含完整 Node Detail、完整 Agent Detail、历史图表或远程操作命令。Attention 与 summary 在同一响应中返回，但当前实现通过独立数据库读取组合，不承诺跨资源的同一时刻原子快照。

`summary` 由 Server 派生，逻辑分组为：

```text
summary.agents: total, online, offline, unknown
summary.nodes: total, active, healthy, unhealthy, unknown, retired
summary.networks: total, with_identity_mismatch
```

不变量为 `nodes.active = nodes.healthy + nodes.unhealthy + nodes.unknown`、`nodes.total = nodes.active + nodes.retired`。Retired Node 不参与实时健康分桶或当前 Attention Item。当前 `AdminOverviewSummary` 没有 `published` 字段；Node list/detail DTO 仍保留 `visibility` 与对应 Owner mutation/filter 作为 legacy compatibility/diagnostic surface。Public 查询不使用它过滤 Home，Site Access Mode 是匿名访问范围的唯一有效站点级开关。

`AttentionItem.kind`、`severity` 与 `subject_kind` 是 typed Server contract，不是浏览器任意字符串。`subject_kind` 限定为 `agent`、`node`、`network`、`settings`。当前 kind 为 `agent_offline`、`agent_spool_fatal`、`agent_spool_overflow`、`agent_report_gap`、`agent_security_event`、`agent_shutdown_incomplete`、`agent_inventory_rejected`、`node_unhealthy`、`node_health_unknown`、`node_resync`、`node_identity_mismatch`。severity 只有 `critical` 与 `warning`：Spool fatal/overflow、security event、Inventory rejection、unhealthy Node 与 Network Identity Mismatch 为 Critical；Agent offline、report gap、incomplete shutdown、unknown Node health 与 resync 为 Warning。新 Agent 在尚无 accepted report 时是 Unknown 而非 Offline；新 Active Node 在 Server-owned first-observation grace period 内是 Starting，不提前产生 unknown-health Attention。Server 按 Critical 优先、权威观察时间与稳定身份排序；WebUI 可以按 Subject 分组展示，但不能丢弃 Item 或重算 severity。

Public 与 Admin Projection 都由 Server 计算 Health，浏览器不得自行重算；但当前实现是 route-specific precedence，而非一个可复用的 canonical evaluator：Public `health_for` 会纳入 process/identity 错误、RPC、Sync、Consensus 与 freshness，Admin `derive_health` 主要基于 RPC、Sync、Consensus 与 freshness，identity mismatch 另作为 Admin attention/identity disposition。因而同一 Node 在两种 DTO 中可能有不同的主标签；两边都必须保持 Unknown、Stale、Disabled、Unsupported 与从未观察输入不因缺省而成为 Healthy。

当前 SPA 对未注册的扩展页面使用 Admin fallback；Server/API 可以先于页面实现，但不得把 fallback 误写成可用页面。

---

## 9. WebUI

页面与交互契约的权威文档是 `docs/design/webui.md`；此处只列当前边界和路由事实。Server 扩展 API 存在不代表 `platpulse-web` 已注册对应页面。

### 9.1 Home

只读 Public Projection，All Networks → Node → Node Detail：

- All Networks（Network 筛选与 Active Node 列表/卡片）；
- Node Detail：当前获用户批准的展示目标以 [WebUI Node Detail 契约](webui.md#node-detail-composition-page-home-node) 为准（本次仅更新文档，不声明实现或验收完成），替代旧的单一 hero 主卡片／四图说明。保留 1280px shell、无卡片标题区、四张摘要卡、三个独立观测面板及同一 60 秒六图顺序：PlatON 进程 CPU、进程内存、共享 Host 上传/下载、Peer 入站/出站、区块间隔、每块交易数；前四折线、后两柱状，手机一列／md（768px）起两列／xl（1280px）起三列，plot 与 SVG 在 lg 下为 7.25rem、lg 起为 6.25rem。标题只显示 Node Health 与 Activity；Linked Validator 概览头部独立显示 Validator badge（Current Validator Status／有效 staking 身份）、Current 提示（Provider freshness/state）与 Activity，共识成员信息留在 Chain info。标题及 Linked Validator 概览复用现有 Activity badge 语义，不替代 Node Health；三个观测面板在 lg（1024px）起并排，低于 lg 堆叠。Linked Validator 六指标窄屏两列／lg 三列，保留完整整数及源可用奖励精度，非紧凑标题换行。Node ID 保持现有完整 UUID；仅 Validator ID 缩写但支持完整复制及键盘/触控展开，复制失败提供可选择的完整值。Head 摘要增加 Node Head - Observed Network Head 有符号差值，仅当前有效且参考为 high-confidence 时显示，否则 dash 加原因；QC/Locked/Committed 保留绝对高度并相对当前 Node Head 显示差值，stale 绝对值标注且不算差值。Last report 在页面可见时每秒更新相对年龄，旁列低强调但可读的精确 UTC；无效/缺失为 Unknown，未来时间有警告，独立展示时钟不改变领域 freshness。原生诊断披露默认折叠，完整状态及 provenance 可展开，相关独立错误保持可见。Node Detail 静态表面 hover 不改变 opacity；Home Node card 明确使用交互样式，其他页面不变。保留 last-good、未知不作零、Host 每 Agent 只采一次和进程内存 RSS / Host 总内存口径；不展示 Bounded Block History 或历史导出。验收覆盖 360/390/768/1280/1440，详见 WebUI 文档。

Home 顶部为六卡片紧凑概览，顺序为 Active Nodes、Healthy Nodes、Cumulative blocks、Attention、Networks、Cumulative rewards。保留当前四项统计的 Network 筛选口径：Active 为选中 Active Node 数，Healthy 为其中 Healthy 数，Attention 为其余 Node 数（含 Unknown），Networks 为选中 Public Network 分组数；排序只改变 Node 顺序。累计两卡片是 Cross-Network Validator Overview：对当前选择内 Server 已按 Network 去重的累计出块/奖励精确数值相加，不重新累加 Node、不跨 Network 合并身份；奖励只是各 Network 原生单位数字相加，不表示同一资产余额或法币估值。每张累计卡片提供可访问的 Breakdown 对话框，保留精确值、逐 Network 独立的 eligible/known/stale 分母与计数、未关联 Node 和缺失/部分/last-good 语义，替代独立 totals 区域。整个页面仍以 1280px 为上限；桌面 >=1024px 概览左右约 5:6，地图占较大一侧，使六张卡片保持原四卡 2×2 布局的紧凑高度（44px 标题行内放标题与指标图标或 Breakdown 控件，数值在其下，不设页脚行），左侧三列两行，六卡在地图带内贴底对齐，使概览到下方网络分组的间距与网络分组到节点卡片的一致（16px）；右侧地图 >=1280px（xl，页面达到上限、列宽不再变化）改用上游 22rem 固定带，地图按 width:100% 依世界自身比例定高并被完整容纳（该带高于世界自然高度，不再裁切）；1280px 以下保持 2:1 比例轨道。不沿用 37:61/232px 固定带。更窄屏幕沿用上游构图：地图在先、统计在后（DOM 仍把统计放在前以利读屏，靠 CSS order 在 lg 以下还原该视觉顺序），>=640px 三列、手机两列。Node grid 保留 auto-fill/minmax(300px,1fr) 列规则；Node 卡片自然高度，不因相邻卡片拉伸。统计与地图共用浅绿渐变与淡网格，Network 筛选同步改变概览、下方 Node 列表和地图范围。地图只使用 Server 提供的国家计数与国家代表点：国家按 Peer 记录逐 Node 计数（不按 IP 去重），未知国家不绘制，无法绘制或缺少代表点的国家保留可访问文字统计，不使用 `[0, 0]` 或随机点回退，也不表示受监控 Node 的部署位置。底图为固定版本、本地托管的世界国家几何（Natural Earth 1:110m Admin 0 Countries，公有领域），由 `platpulse-web/scripts/build-world-geometry.mjs` 离线生成并随 WebUI 静态资源同源托管；运行时不访问地图 CDN 或在线瓦片。Geo 停用、底图加载失败或地图渲染失败只在概览局部降级，不影响统计、筛选、排序与 Node 卡片。

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
- Enrollment、Recovery、Rotation 和 Revoke 都已提供相应 credential seam：Recovery 额外推进 Agent Epoch 并拒绝旧 Epoch 报告，Rotation 只更新凭据集合（可保留短暂 overlap），Revoke 只立即使指定凭据失效；当前 SPA 没有 Enrollment/Recovery/Rotation 专用管理路由，但 Agent Detail 已提供单个凭证撤销。§15.2 的新增接入入口和 Agent Removal 是待实现能力；
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
- Public WebUI SSE 连接中显示 `Connecting to live updates`，断开显示 `Live updates paused`，并通过 REST 重新获取；连接成功的正向传输状态有意保持沉默，不渲染也不播报任何提示——它不改变访问者应当做的事或应当相信的事，只表示通道已打开，这些传输状态均不代表 Node 健康或观测新鲜度（工单 #138 取代 #126 验收标准中"连接成功显示 `Live updates connected`"一条）。Admin 保持紧凑的既有 `Starting`/`Current` 传输文案。

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

- Agent：Linux x86_64/aarch64，建议 systemd；每 Host 一个 Agent；独立 state directory；credential 与 SQLite 严格权限；只需访问本地 RPC Endpoint 与 Server HTTPS 地址。仅当 Node 声明 `supervisor` selector 时，Agent 运行用户还需要能执行 `supervisorctl` 并访问 supervisor 的 control socket。
- Server：单进程、单 SQLite、同源托管 WebUI；`/health/live` 只判 event loop 存活，`/health/ready` 同时检查 `sqlite`、`owner`、`web_assets`、`shutdown`、`critical_workers`、`corruption` 六个组件，并以 200/503 表达整体结果；Backup/Restore 是显式运维流程，不是启动流程。非开发模式下 Server 以 SQLite `locking_mode = EXCLUSIVE` 独占数据库文件，外部 SQLite 连接（含 CLI 写入）会在 Server 运行期间被隔离，因此只能在 Server 停止时执行；开发模式保留 SQLite 常规锁，允许本地工具与 e2e fixture 直接读写运行中的数据库。
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
- Home 概览为六项当前选择统计 + 透明 Peer 国家地图，保留 1280px 页面上限与现有 Node grid 列规则；Network 筛选同步作用于四项原统计、累计出块/奖励数值概览、Node 列表与地图。桌面 >=1024px 统计/地图约 5:6（地图占较大一侧，统计保持原四卡时的紧凑高度），统计三列两行；地图 >=1280px 用上游 22rem 固定带，低于该宽度保持 2:1 比例轨道；低于该宽度统计在地图上方，>=640px 三列、手机两列；逐 Network 精确值、coverage/stale、未关联 Node 通过累计卡片的 Breakdown 访问；
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

<a id="accepted-management-target"></a>

## 15. Agent/Node 管理、Validator 自动识别与提示确认（已确认，部分实现）

### 15.1 状态、范围与依据

本节来自已完成的 `/grill-with-docs` 访谈（Q1–Q22）及 Owner 的最终共识确认，是目标设计。后续实现规格按仓库约定进入 GitHub Issues，本节不代替具体接口设计或实现工单。实现状态：§15.2 第 1、2 项（Agent 接入引导、显示名称/备注）已由 issue #169 交付；§15.3 Node Purge 已单独交付；§15.2 第 3–5 项 Agent Removal 已由 issue #171 交付（含所属 Node 权威列表、未处理 Transfer 阻止、全凭证撤销、子 Node 级联清理与删除身份边界）；§15.6 Agent Attention Acknowledgment 已由 issue #172 交付（Server-owned occurrence/evidence 边界、共享持久确认、Overview 与 Agent Detail 的逐条与批量操作、失败可重试与审计）；§15.4 自动 Validator 身份与 Current Validator Status 已由 issue #173 交付（Server 从已校验 Network 与观测到的完整 P2P 公钥自动建立 Node Validator Link、换键关闭旧区间、Public/Node 视图消费 Server 投影，不再展示手工 role）。手工注册/绑定/角色写入端点已退役并返回明确的 GONE 状态。一次性 Validator 模型迁移（§15.5）已由 issue #174 交付：Server 启动迁移在同一 SQLite 事务内识别无 automatic Link 的旧手工一代，删除其 Link、current insight、ranking/counter history、daily/monthly 聚合与 Validator 身份本身，并写入 validator_model_migration 一次性标记；automatic 一代身份与数据保留，旧分类/变化基线同步失效，Node 监控历史、既有 Incident 与必要审计保留；§15.7 删除主体的通知取消与 Incident 标注已由 issue #175 交付（被删 Node/Agent/Host 退出当前评估并标注 subject_deleted_at 而不伪造恢复，未发送通知——含无 incident_id 的恢复事件——被取消，已发送事实保留，其他主体与共享 Validator 通知策略不受影响）。

- Agent：Owner 接入引导、显示名称/备注修改、Agent Removal。
- Node：保留已有重命名，只扩展 Owner 显式 Node Purge；不提供 Admin 新建 Node 或远端采集配置编辑。
- Validator：自动身份对应替代手工 Link/角色；独立 Validator 数据和统计继续存在。
- 提示：Agent Attention Acknowledgment，而非 Agent 进程发起业务告警清除。
- 不变：无远程控制、Network Registry 信任边界、每 Node 观测隔离、last-good、不可变 Agent Report 与事务性 Receipt、Owner-only mutation、Public/Admin 分离和移动端可用性。

长期取舍见 [ADR 0004](../adr/0004-owner-removal-and-node-purge.md) 与 [ADR 0005](../adr/0005-automatic-validator-identity.md)。交互见 [WebUI §15](webui.md#accepted-management-ui-target)；Validator 适配与指标分别见 [Provider 设计](validator-provider.md)、[指标设计](validator-metrics.md)。

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
| 验证中、信息不足/冲突，不能确认身份有效 | 未知 | 不推断为否 |
| Provider 失败或信息过期 | 不产生新的肯定/否定结论 | 保留 last-good 并标 Stale；从未成功则 Unknown |

缺少可信公钥、Network Identity 不匹配或 Network 未配置可用 Provider 时，不能猜测关联，也不能拿其他 Network 或旧手工关联代替。本次业务原则已确定；具体 PlatScan 字段/状态如何证明“当前有效”，尤其 locked/exiting/verifying 和权威否定的条件，仍需主源证据验证，见 Provider 设计。普通 HTTP 失败不是无质押的证据。

### 15.5 一次性 Validator 模型迁移

- 迁移切换时立即停止以旧手工 Link 作为当前关联或回退，删除旧手工关联及旧 Validator 快照、ranking/counter history、daily snapshots、monthly aggregates；切换后通过自动识别重新采集和积累。
- 不清空 Node 的区块、Peer、metric 等监控历史，也不因 Validator 本地历史清理删除既有 Incident 和必要审计。
- 同步重建或失效化依赖旧模型的 current insight、分类/变化基线和汇总，不能只删历史表而让旧 change/counter 分类继续生效。迁移边界不能被当作一次真实的奖励下降、counter reset、告警恢复或新异常。
- 迁移与 Provider 写入、daily/monthly 重建协调，阻止旧一代任务回填已经清理的旧数据。验证依赖与清理范围后再执行；失败不能以半新半旧状态继续正常服务。这里规定迁移安全结果，不新增具体 migration 编号或执行脚本。
- 本地历史从切换后重新积累，先前已删历史不承诺恢复。PlatScan 再次返回的 lifetime blocks/rewards 仍是链上累计值，不是归零计数；当前选择总计可能暂时下降/未知，不能伪造连续覆盖或完整月份。
- 这是显式一次性模型转换，不改变常规 Retention 对 Validator 日/月数据的保护，也不授予普通 Node 删除操作清除共享 Validator 历史的能力。

实现状态：§15.5 已由 issue #174 交付。Server 启动迁移（`0053_validator_model_migration.sql`）在同一 SQLite 事务内识别并删除旧手工一代（无 automatic Link 的 Validator 身份）的 Link、current insight、ranking/counter history、daily/monthly 聚合及其身份本身，保留 automatic 一代的身份与数据，并写入 `validator_model_migration` 一次性标记。失败即整体回滚并停止启动；运行时身份发现与 Provider 刷新 worker 只在迁移提交后才启动，被删身份也不再拥有可写入的外键归属，因此旧一代任务无法回填。

### 15.6 Agent Attention Acknowledgment

**目的：确认已读后移出醒目的提示区，不伪造恢复，不抹掉证据。**

- 由 Owner 对 Agent Attention Item 发起确认；Server 持久化共享结果，Overview 与 Agent Detail 的醒目提示同步移除，所有 Owner、登录会话与 Server 重启后均生效。
- 允许确认历史证据提示，也允许确认仍在持续的离线/存储故障提示。当前真实 liveness、health、各诊断维度和历史证据仍可查看；确认不改变 Alert Rule/Incident 或通知策略。
- 同一次提示确认后不再展示。普通刷新、无新增问题的报告、换浏览器或重启不重新提示；新增证据或恢复后再次发生的故障要重新提示。未知/过期输入不能假装恢复并制造“新的一次”。
- 当前 DTO 的 kind + subject 只标识问题类型/主体，不足以标识发生次数。目标需要 Server-owned 的发生/证据边界；历史 count/range 的确认不能用永远隐藏该稳定 ID 代替，也不能只用会随普通 Host 更新漂移的时间戳。计数回落、Epoch/Boot 变化与状态恢复不能使新证据被旧确认误吞。
- Overview 提供 Agent 提示逐条确认；Agent Detail 提供逐条及“确认当前全部提示”。批量只覆盖 Owner 本次明确看到的 Agent 提示及其证据边界，不包括随后新增内容，不连带确认所属 Node。
- 原始证据留在可主动展开的诊断区域，确认记录包含谁在何时确认什么范围。失败不隐藏提示；并发确认可以安全重试，但不能确认客户端未看到的新证据。返回成功后重取权威投影，不用浏览器永久隐藏列表。
- 本次不增加 Node 提示确认、永久关闭某类提示、Silence/Maintenance 页面或完整 Incident 历史页面。

### 15.7 恢复、确认、主体删除与通知

| 动作/事实 | 提示与 Incident | 数据/通知 |
|---|---|---|
| 当前故障恢复 | 当前提示条件消失；Incident 仅在已知恢复持续满足规则后 resolved | 恢复不自动删除 Incident；历史型提示可能仍需确认 |
| Owner 确认提示 | 同次提示退出醒目区域，持续故障的真实状态不变 | 证据与 Incident 保留；不暂停通知、不发虚假恢复 |
| Agent/Node 删除 | 主体退出当前待处理问题和后续告警评估；保留 Incident 原有事实并标注主体已删除 | 保留既有 Incident/必要审计；取消尚未发送通知，不因删除发恢复通知 |

Incident 保留证据不等于继续把它当作当前待处理故障；删除主体不把 open Incident 改成声称已知恢复的 resolved。当前 SPA 没有 Incident 管理页面，本节不承诺新增。常规 Retention 保护 Incident 历史；Notification Event 有独立保留策略，不能把二者混淆。

取消通知按主体覆盖所有未发送项，不能只通过 incident_id 找关联：当前恢复 Notification Event 可以没有 incident_id。实现需协调 worker claim/发送前检查；已发送或已交给外部通道且无法撤回的发送不能宣称已撤回。保留已有交付事实，不影响其他 Agent/Node 或共享 Validator 主体的通知策略。

### 15.8 待实现验收与技术核实

以下是目标验收，不代表测试已存在或本次已执行：

| 场景 | 必须验证的结果 |
|---|---|
| Agent Enrollment 与资料 | Token 本身不建 Agent；实际接入后可编辑名称/备注，ID/Host/liveness 不可伪改 |
| Agent Removal | 列明子 Node、阻止未处理 Transfer、全凭证失效、子 Node 数据清理、历史审计保留；不远程停进程 |
| Node Purge 与重报 | Active/Retired 均可显式删除；后续同 revision 的合法 Inventory、更新 revision、重试、重启和迟到任务都不能重建已删 ID；其他 Node 正常接收 |
| 数据所有权 | Node 专属监控历史/Links 清理；共享 Agent/Network/Validator 历史及 Incident 证据不误删 |
| 自动身份 | 正确 Network+完整公钥识别、缺少键/错误 Network 不猜测；换键关闭旧区间且不拼接累计值 |
| Validator 当前状态 | 候选/特殊状态/退出/未知按证据区分；失败保留 stale last-good，不能 false 或 Healthy |
| Validator 一次迁移 | 清掉旧 Link/历史/衍生基线，不清 Node 历史；旧后台写入不回填、不伪造连续历史或 counter reset |
| 确认与复发 | 同次确认后持久消失；新证据/真实恢复后再发重新提示；当前健康和 Incident 不变 |
| 批量/多 Owner | 仅确认看到的 Agent 证据，新增并发项与 Node 提示不被吞；Overview/详情/其他 Owner 一致 |
| 删除与通知 | 已删主体停止当前评估，不伪造恢复；无 incident_id 的待发送通知同样取消；已发送事实保留 |
| 权限、故障与响应式 | Owner-only、CSRF/Origin、Audit、失败/冲突重取、不可逆确认；Public 无管理数据泄漏；固定移动/桌面项目可用 |

具体 DTO、端点、schema、迁移顺序、Receipt disposition 编码和 PlatScan 状态证据尚需在实现工单中落实；本次未生成 API、未开展新的实时 PlatScan 验证，也未运行 Rust/WebUI 测试或执行清理。不得据本节把旧的导出但未注册页面当作新能力已上线。

### 15.9 Node Inventory 声明纪律与拒收可观测（已实现）

本节来自 issue #177 的 `/grill-with-docs` 访谈共识，是目标设计。目标是让「Node Inventory 内容变了但 `inventory_revision` 没 bump」在 Agent 声明之前就可见、可操作，并让「整份 Inventory 被拒收」在 Admin 可见，而不是等当前投影冻住、且只能直连 SQLite 排查。

实现前的事实（issue #177 报告的现象）：

- Server 对「同 revision、内容哈希不一致」的整份 Inventory 拒收（`inventory_revision_conflict`），revision 回退使用同一个 code；`inventory_sha256` 只存在于 Server（`agents.inventory_sha256`），Agent 不计算 Inventory 哈希。
- Agent 每次构建报告都重新读取 `agent.toml`，配置改动无需重启即生效；因此「启动时自检」既不覆盖运行中改配置，也不能在 Operator bump 后自愈。
- Agent 把拒收原因写入本地 `delivery_diagnostics.last_error` 并随 `host.spool.last_delivery_error` 上报，但该字段只在整份报告被接受时落库，因此在这条「每份报告都被整份拒收」的路径上永不更新；Admin 侧也不读取 `agent_report_receipts`。

已确认设计：

1. **Inventory Declaration Record**：Agent 持久化「最后一次已生效 Node Inventory 的 revision 与内容哈希」（术语见 CONTEXT.md）。只在 Report Receipt 的 Inventory disposition 为 accepted/unchanged 时，于 receipt 应用事务内写入；记录缺失（首次运行或存量升级）时静默采用，不报错。
2. **共享哈希口径**：canonical 哈希实现放在 `platpulse-core`，由 Server 的比较/记录与 Agent 自检共用。哈希对象是已发布 `NodeInventory` 的序列化（含 revision 与 nodes），因此 `data_directory`、`collection_interval_seconds` 等本地字段不参与；`display_name` 属于 wire 字段，改它而不 bump 同样应被拒绝。
3. **Agent 自检**：声明前比较本地配置与记录——同 revision 而内容不同 → 拒绝声明；revision 低于记录 → 拒绝声明。声明的入口（`run`、`collect-report`、`persist-report`）拒绝启动并以非 0 退出，且在 `recover_previous_boot` 之前判定，避免产出注定被拒的 Closing 报告；长期运行中出现的配置漂移不结束进程，只拒绝声明并以去重日志给出可操作诊断，Operator bump 后下一个采集周期自愈。`shutdown` 与 `recover` 不受守卫，保证仍能收尾与排障；`validate-config` 以只读方式检查 Agent Store 并报错。
4. **复现诊断**：整份 terminal 拒收在 Agent 控制台按 `(code, reason)` 去重打印并带计数。现有实现只在 transport `Err` 时打印，拒收走「receipt 已应用」路径因而静默。
5. **Server 侧证据**：`agent_report_receipts` 增加可空列记录拒收 code 与本次上报的 Inventory revision/哈希。不改 wire `RejectionCode`、不改 `ReportReceipt`，也不为该表新增保留策略。（2026-09-23 增补：该结论不变——不新增**行**保留；身份行永久保留，只有 `receipt_body` 在固定 30 天窗口后瘦身为紧凑 receipt，见 [ADR 0009](../adr/0009-report-receipt-body-slimming.md)。）
6. **Admin 可观测**：新增 Agent Attention kind `agent_inventory_rejected`（critical，可按 §15.6 确认），条件是「该 Agent 最新一行 receipt 是整份 Inventory 拒收」；evidence 边界取「原因 + 已接受 revision/哈希 + 上报 revision/哈希」，因此内容不变而每周期重报不会让确认失效，只有内容或已接受状态变化才重新提示。Admin Agent DTO 增加嵌套的 Inventory 诊断（已接受 revision/哈希 + 最近一次拒收证据），WebUI 在 Inventory 与 Diagnostics 面板展示；拒收提交后向 Admin realtime 发 `agent` invalidation。
7. **不改**：wire 契约、接受路径的同 revision 哈希判定、以及「整份 Inventory 才生效」的语义都不放宽。

实现状态：已实现（issue #181），§8.4.2 的 kind 与 severity 清单已同步。本节保留当前 v1 的哈希、配置、自检与诊断规则，不随词汇表的目标语义更新而自动改变。方向 3（Server 托管 revision）的设计已由 issue #182 访谈确认，见 [ADR 0007](../adr/0007-server-managed-inventory-revision.md) 与下节；协议、Server 分配与新确认记录语义已由 issue #186 实现（仅限隔离的新部署链路）；存量迁移的准备、协调检查点（issue #189、#190）与离线基线转换（issue #191），以及协调生产切换、v1 仅重放及回退边界（issue #192）已实现。

<a id="server-managed-inventory-revision-target"></a>

### 15.10 Server 托管 Inventory Revision（已确认，协议、分配、故障语义、离线基线转换与协调生产切换已实现）

#### 15.10.1 状态与边界

本节记录 [issue #182](https://github.com/mowind/PlatPulse/issues/182) 的 Q1–Q12 访谈及最终确认，以及 [issue #186](https://github.com/mowind/PlatPulse/issues/186) 的首个实现切片。长期取舍见 [ADR 0007](../adr/0007-server-managed-inventory-revision.md)。v2 协议 major、Server 端事务内版本分配、v2 Receipt 与 Agent 确认记录已实现；省略 `inventory_revision` 的新 Agent 配置选择 Server 托管路径。issue #187 补齐了重试、乱序、并发与事务失败下的确认一致性：真实 Agent→Server→Agent 链路上的离线积压、精确 Receipt 重放、身份冲突、顺序屏障、编号耗尽、Boot 轮换与 Agent Recovery 连续性，以及受控 SQLite 故障注入下的整事务回滚，见 §15.10.6。issue #188 将 v2 准入贯通 Node 生命周期：规范化指纹覆盖仍被声明的已 Purge Node ID，准入判断独立于内容比较，内容不变时每份新报告仍逐 Node 重新执行 Purge、归属、Transfer 与 Network 校验；Admin Inventory 诊断与 `agent_inventory_rejected` Attention 改用 Server 记录的协议、接受版本/指纹与实际拒收证据解释 v2，不再伪造已移除的 Agent 版本号，且从未接受过声明时保持 Unknown 而非 revision 0。issue #189 已交付 v1 升级准备桥接：由 Operator 入口停止普通采集、继续投递既有不可变积压、完成最终 Closing，并在任何排序或规范化之前按原 v1 表示核对最终声明与 Server 最后接受的 revision/hash；成功结果写入有界单行迁移证据，并保留 Closing 后的新 Boot、sequence 与 drained_pending 衔接，不自动恢复采集。issue #190 已交付协调检查点与隔离恢复/验证：分别以既有进程所有权/离线互斥保护 Server 与 Agent 两侧，用精确 SQLite 快照（含有效 WAL 内容、不脱敏）保留 Server 数据库、Agent Store、原配置、身份/凭证、删除身份与迁移声明证据，并在隔离目录复验恢复后仍是同一个已关闭旧 Boot、待新 Boot 衔接的检查点，且不启动采集、接收、管理写入或外部通知工作器。issue #191 已交付离线基线转换、配置迁移与离线校验：`platpulse-server checkpoint convert` 从已核验的协调检查点重新计算 v2 规范化指纹、保留原接受 revision、转换 Server 基线与 Agent Inventory Declaration Record 并写出 v2 agent.toml，`checkpoint verify-conversion` 在不启动任何 worker 的情况下复验转换结果；Agent 在采集或业务写入前拒绝转换后残留的 `inventory_revision`。issue #192 已交付协调生产切换、运行期门禁与 v1 仅重放：platpulse-server cutover resume 是唯一显式切换门槛，它重新核验前置准备证据（经协调检查点）、离线转换结果与转换后 Server/Agent 两侧基线的一致性，任一条件缺失、状态被改动或参与者不一致即拒绝，并在通过后才写入持久切换标记；切换标记缺失时，转换部署以只读诊断模式运行——拒绝普通 collection/ingestion、Admin mutation 与外部副作用 worker，且 readiness 报 cutover_not_resumed；切换后普通新报告只走 v2，冻结 v1 路由仅在既有鉴权/归属/输入限制下精确重放已保留 Receipt，同身份异字节冲突，未知报告明确不支持且不创建 Receipt、不推进 Boot、不分配 revision、不更新投影，重放不绕过 Agent Removal；恢复业务写入前用 cutover rollback 从同一协调检查点整体恢复旧二进制、Server/Agent 状态与配置，写入恢复后该动作被拒绝并指引前向修复。

Agent 继续拥有完整 Node Inventory 和连接配置；Server 只接管接受版本的编号，不下发 Endpoint 或其他采集配置。不可变 Agent Report、事务性 Receipt、Network Registry 校验、last-good、每 Node 隔离及 §15.3 永久删除屏障保持不变。

#### 15.10.2 声明内容、编号与顺序

- v2 声明只提供内容，不提供 Agent 分配的 Inventory revision。对校验后的完整声明计算独立、明确版本的规范化指纹：排除 revision，按 Node ID 排序，保留所有声明字段（含 bootstrap display_name、network_key、rpc_endpoint、process），不包含 Agent-only 配置。不推断额外 URL、路径或字符串等价关系；可选字段按冻结的新协议表示规范确定性编码。
- 此指纹不是 Report 原始字节哈希，也不是准入后的 Node 投影哈希。v1 含 revision、依赖 Node 数组顺序的旧哈希规则不变。
- Inventory Revision 是每个 Agent 的已接受声明变更序号。连续内容相同沿用编号；A → B → A 得到连续的新版本而不复用历史编号。整体 Inventory 拒绝不分配新编号；Node Purge 本身不递增。
- 复用既有 Epoch、Boot lifecycle 和 Report Sequence 防回退约束，不新增声明 CAS。不以网络到达顺序替代合法报告顺序；较新声明接受后，迟到的旧报告不得仅凭不同内容获得新版本。合法的新报告主动恢复旧内容则是新变更。
- 编号属于 Agent 身份，不属于 Boot、Epoch 或协议版本。普通重启与 Agent Recovery 不重置；Recovery 后内容确实改变才递增。新 Agent 身份独立编号。从未接受 Inventory 时无有效版本，首次接受为 1；实现须区分未初始化状态与已接受的空 Inventory。
- 编号限定在正的 signed 64-bit 存储范围内，检查递增。耗尽时拒绝需要递增的变更并保留既有状态，不回绕、不归零；相同内容不需要虚增。

#### 15.10.3 Receipt、Agent 确认与准入

v2 Receipt 的 Inventory 接受结果绑定该报告声明的 revision 与规范化指纹；unchanged 返回原编号，整体拒绝不携带表示该声明已接受的编号。Report 顶层 partially_accepted 不能代替 Inventory 自己的接受结果。编号、指纹、适用的投影变化与精确 Receipt 必须同事务提交；回滚不得留下已分配版本。重放返回原 Receipt，不能换成 Server 此刻最新编号。

Agent 的 Inventory Declaration Record 保留为有界单条确认状态，不再分配编号或作为上报前置守卫。应用 Receipt 时，先验证它与原始不可变 Report 及声明指纹相符，再按下表处理；所有结果与原 Report 出队、其他 Receipt effects 同事务提交，保持 ADR 0001 的 Applied Receipt Record 边界。

| 已验证的确认 | 本地记录处理 |
|---|---|
| 无记录，或确认 revision 更大 | 采用接受的 revision 与指纹 |
| revision 相同且指纹相同 | 幂等，不变 |
| 确认 revision 更小 | 可完成旧 Report 的确认，但不回退记录 |
| revision 相同但指纹不同 | 冲突，失败关闭并保留待调查证据，不覆盖、不悄悄出队 |
| Inventory 整体拒绝 | 不推进声明确认；其他终态处理依合法 Receipt 执行 |

缺失确认、延迟 Receipt 或 Server 离线均不阻止生成新的不可变 Report；不把确认编号填回下一份声明，也不在 Agent 自增。

内容指纹覆盖完整声明，包括仍被 Agent 声明的 Purged Node ID。每份适用的新报告仍独立执行归属、Transfer、Network 与 Purge 准入校验，不能以内容未变跳过。有效声明即使全部 Node 已被 Purge，也可以是 Inventory accepted/unchanged、per-Node 全拒绝；合法兄弟 Node 不被阻断。最新有效完整声明仍决定退休关系，但永不绕过永久删除身份。不得修改旧 Receipt、删除去重证据或远程修改配置。

#### 15.10.4 协调停机、可验证基线与配置迁移

**不支持新旧协议混用运行，不承诺新 Agent 自动降级连接旧 Server。** 冻结 v1 不以可选字段扩展；使用新协议 major 与新 fixtures。以下是后续必须实现的升级准备契约，不是当前可直接执行的命令：

1. 在旧 Agent/Server 下停止普通新 Report 生成，继续交付已有不可变 Report，直至完成所有待交付结果的事务性确认。不得改写旧字节、清空 Spool 或重新 Enrollment 绕过。
2. 完成最终 Closing Report 并应用其 accepted/partially_accepted Receipt；Rejected Closing 不能过关，队列为空本身也不足以升级。捕获该 Closing 对应的完整 Inventory 声明作为有限迁移证据，而非新增永久报告归档。
3. 在改排序或规范化之前，用原 v1 表示验证该声明的旧哈希及 revision 与 Server 最后接受的值一致。当前本地配置、Node 投影、哈希本身均不能代替证据：Server-managed 名称、已 Purge 的 Node 和未保存的完整报告使投影不可逆。缺证据、不匹配或关闭失败，停止升级并在旧版本排障；不静默采用首次 v2 声明作为不可验证的新基线。
4. 在相关写入及有外部副作用的工作器均暂停后，于旧协议关闭完成的检查点备份 Server、Agent 状态与旧配置。离线验证不得启动正常 collectors、ingestion、管理写入或通知等外部交付工作器。
5. 从验证通过的声明计算新指纹，同步转换 Server 基线与 Agent 确认记录。保留 agents.last_inventory_revision 的数值，例如 57 保持 57，下一次实际变化才是 58。agents.inventory_sha256 的旧摘要不能直接转换或从当前 Node 行重算，必须由验证后的声明重建新指纹并明确算法/协议解释；无历史接受记录的 Agent 保持未初始化。
6. 保留 Closing 应用后产生的新 Boot、sequence 状态、previous_boot_id 与 drained_pending 衔接。首份 v2 Report 完成既有 DrainedPrevious，不重置身份、不伪造独立 Continuing。Server 与 Agent 的任一侧转换不一致都不能恢复正常运行。
7. v2 移除本地 inventory_revision 配置项；残留时在恢复采集前明确报“Server 已分配版本，请移除此项”的迁移错误，不静默忽略。旧配置保留于回退检查点。离线验证通过后再恢复业务写入。

迁移命令、物理列、版本标记、fixture 编码与 schema 编号由实现规格确定，但必须实现上述已确定的语义，不把这些落地细节当作允许重置身份或放宽检查的空间。

实现状态（issue #189）：第 1、2、3 条的准备与核验已经交付为 Agent 操作者入口（`prepare-upgrade`）——停止普通报告生成、投递全部待交付 Report 并完成最终 Closing（Rejected Closing 不推进 Boot）、捕获最终 Closing 的完整 v1 声明，并通过 Server 托管的只读基线核对 revision 与原 v1 哈希；证据缺失、不匹配、Closing 未成功、状态改变或归属不符时给出可操作失败且不写入可迁移结果。第 4 条的协调检查点与隔离恢复/验证由 issue #190 交付：`platpulse-agent checkpoint create` 与 `platpulse-server checkpoint create` 各自在打开 SQLite 前取得既有进程所有权/离线互斥，Server 侧复核 Agent 证据与其自身已接受的基线及 Closing Report Receipt 一致后，用精确 SQLite 快照（含有效 WAL 内容、不脱敏）保留 Server 数据库、Agent Store、原配置、身份/凭证、删除身份与迁移声明证据；`checkpoint verify` 与 `checkpoint restore` 在隔离目录证明恢复后仍是同一个已关闭旧 Boot、待新 Boot 衔接的检查点，不使用会脱敏改写的 Server Backup，也不启动采集、接收、管理写入或外部通知工作器。第 5、6 条的基线转换与配置迁移由 issue #191 交付：`platpulse-server checkpoint convert` 在重新核验协调检查点后，从已验证的完整声明计算 v2 规范化指纹，保留 `agents.last_inventory_revision` 的数值（例如 57 保持 57），同步转换 Server 基线与 Agent Inventory Declaration Record，并写出移除 `inventory_revision` 的 v2 agent.toml；`checkpoint verify-conversion` 以只读方式离线复验两侧基线与 Boot 衔接，不启动采集、接收、管理写入或外部通知工作器；隔离 E2E 证明首份 v2 Report 延续既有 DrainedPrevious 而非重置身份。第 7 条“离线验证通过后再恢复业务写入”的运行期门禁、显式切换、v1 仅重放与回退边界由 issue #192 交付：platpulse-server cutover resume 在持有独占所有权、重新核验协调检查点与离线转换并证明转换后基线与 Boot 衔接后才写入 inventory_cutover 标记；标记前转换部署拒绝普通 collection/ingestion、Admin mutation 与外部副作用 worker 且 readiness 不 ready；标记后普通报告仅走 v2，冻结 v1 路由仅精确重放已保留 Receipt，未见过报告明确不支持且无副作用，重放不绕过 Agent Removal；恢复业务写入前 cutover rollback 从同一检查点整体恢复旧部署，写入恢复后拒绝作为软件回滚。

#### 15.10.5 回退与旧 Receipt 重放

- 仅在恢复业务写入之前承诺从协调检查点整体回退旧二进制、Server/Agent 状态与配置；不是只替换二进制继续使用新数据库。
- 恢复业务写入后不承诺无损降级，优先前向修复。恢复旧备份是单独灾难恢复操作，可能丢失升级后的声明、Purge 屏障或其他写入，不能包装为安全的软件回滚。
- 切换后保留正常鉴权、Agent 归属与输入安全限制下的 v1 **仅重放**路径：已保留 Report 身份且原始字节哈希一致，原样返回原 v1 Receipt；同身份异字节冲突。不能将旧 Receipt 改成 v2 或填入最新版本。
- 未见过或已不在保留边界内的 v1 Report 均不能进入新接收流程：明确不支持，不创建新接受 Receipt、不推进 Boot、不分配 revision、不更新投影。凭证无效依然拒绝，重放不绕过 Agent Removal。
- 沿用既有 Receipt retention，不引入无限期 v1 档案。仅重放不是混用运行支持，更不能替代升级前的 drain/Closing 门槛。

#### 15.10.6 后续实现验收矩阵

以下为验收矩阵。首次、相同内容、仅重排、字段变化、A → B → A、空 Inventory 与拒绝路径已由 issue #186 的 core 契约测试与 `backlog_recovery_tests` 端到端测试覆盖。issue #187 已交付：晚到/旧 Epoch/竞争 Boot 的顺序屏障、并发接收的单事务比较与分配、编号上限的 checked 递增、精确 Receipt 重放与同身份异字节冲突、Server 分配/指纹/投影/Receipt 失败的全量回滚与无 post-commit invalidation、Agent Receipt effects 与确认记录出队的原子性、同版本异指纹的失败关闭、较小确认不回溯、无确认下的离线继续声明，以及重启、Boot 轮换与 Agent Recovery 不重置编号。issue #188 已交付：内容指纹不变时仍逐报告执行 Purge/归属/Transfer/Network 准入、全部 Node 已被 Purge 仍返回 Inventory accepted/unchanged 加逐 Node 全拒绝、从最新合法声明移除的 Node 按既有 Retired 语义处理、Server 重启与迟到写入不重建已 Purge Node，以及 v2 的 Admin Inventory 诊断与拒收 Attention（见 `crates/platpulse-server/tests/node_purge.rs` 的 v2 场景）。issue #189 已交付 v1 升级准备桥接：停止普通采集、投递既有不可变积压、完成最终 Closing 并应用其 accepted/partially_accepted Receipt、捕获完整 v1 声明并在规范化前核对 Server 最后接受的 revision 与 inventory hash，以及证据缺失/不匹配/Rejected Closing/状态改变时的可操作失败与安全重试（见 `crates/platpulse-agent/src/preparation.rs`）。issue #190 已交付协调检查点与隔离恢复/验证：两侧进程所有权/离线互斥、精确 SQLite 快照（含有效 WAL、不脱敏）、Agent 证据与 Server 已接受基线及 Closing Report Receipt 的一致绑定、缺失/损坏产物的拒绝，以及隔离恢复后仍是同一个已关闭旧 Boot、待新 Boot 衔接的检查点（见 `crates/platpulse-server/src/checkpoint.rs` 与 `crates/platpulse-server/tests/checkpoint.rs`）。基线转换、配置迁移与离线校验相关行已由 issue #191 交付（见 `crates/platpulse-server/src/migration.rs` 与 `crates/platpulse-server/tests/migration.rs`）：协调检查点重新核验、规范指纹转换、revision 保留、未初始化与已接受空声明区分、配置迁移与残留拒绝、两端不一致停止、57→57→58 与首报 DrainedPrevious 衔接；生产切换与恢复业务写入后的回退相关行已由 issue #192 交付（见 crates/platpulse-server/src/cutover.rs、迁移 0058 与 crates/platpulse-server/tests/migration.rs 的 cutover 场景）：

| 场景 | 必须验证的结果 |
|---|---|
| 仅重排 Node / 改 Agent-only 设置 | v2 声明指纹及编号不变；Report bytes hash 仍按各自原始内容验证 |
| 声明字段变更 / A → B → A | 覆盖 name/Endpoint/process/Network/成员变化；每次合法内容变更递增，返回 A 不复用旧版本 |
| 冻结 v1 与新 canonical fixtures | v1 原哈希及严格解码不变；新表示跨 Agent/Server 一致，不擅自归一化字符串 |
| 旧报告晚到 / 竞争 Boot / 旧 Epoch | 原顺序屏障有效，不以到达顺序分配版本；合法新报告恢复旧内容仍可推进 |
| 并发接收 / 事务失败 | 指纹比较与分配串行化于权威事务；无重复分配、无部分投影、无回滚残留或 invalidation |
| Receipt 丢失与重复发送 | 相同身份/字节得到精确原 Receipt，不重复递增；同身份异字节冲突 |
| Receipt 延迟或矛盾 | 较小版本可完成确认但不回退；同版本异指纹失败关闭，出队与记录不能半提交 |
| Inventory 与 per-Node 结果不同 | 以 Inventory disposition 决定编号/确认；partial report 不是半份 Inventory 接受 |
| Purge 后同内容、全 Purged、合法兄弟 | 指纹包含全部声明；已删 ID 不复活、兄弟可接收、全 Purged 不伪装成结构拒绝 |
| 从声明移除 Node / Transfer / Recovery | 生命周期与归属检查不跳过；恢复不重置编号，也不解除 Purge/Removal 屏障 |
| 首次 / 空 Inventory / 编号上限 | 未初始化不同于已接受空集合；首次为 1；上限处只拒绝需要递增的变化，不回绕 |
| 旧积压与 Closing 失败 | 不允许升级，不改 Report、不丢积压；恢复旧版本完成确认后才能继续（issue #189：全部待交付 Report 完成事务性确认、最终 Closing 必须 accepted/partially_accepted，Rejected Closing 与发送超时均阻止准备并保留原字节） |
| 快照缺失、错误顺序或 hash/revision 不匹配 | 预检停止；禁止用当前投影、未经核验配置或新首报猜基线（issue #189：最终声明缺失/不符、Server 基线 hash、revision、协议、Boot 或 Closing 归属不一致时拒绝颁发可迁移结果） |
| 迁移基线与 Boot | 旧数值保留、两端指纹同步转换；DrainedPrevious 延续，旧 Receipt/去重不变（issue #191：`checkpoint convert` 保留 57→57，隔离首份 v2 Report 执行 DrainedPrevious） |
| 配置残留 / 任一转换失败 | 在采集和业务写入恢复前报可操作错误，不能以半迁移状态运行（issue #191：转换后 Agent 拒绝残留 `inventory_revision`；失败转换不留下半份产物，隔离校验覆盖两端不一致） |
| 离线回退 / 已恢复写入 | 前者在写入恢复前从同一协调检查点整体恢复旧 Server/Agent 状态与配置（issue #192：cutover rollback 拒绝只换旧二进制读取新数据库）；后者不宣称无损降级，不自动倒退删除屏障 |
| v1 仅重放与鉴权/保留边界 | 精确重放已有 Receipt；异字节冲突、无权限拒绝、未知/已过期 v1 不进入新接收（issue #192：切换后仅重放经真实 HTTP 鉴权链路验证，未知 v1 无副作用拒绝且不绕过 Agent Removal） |

后续实现需同时更新相关 Inventory 诊断及 Attention 的版本/指纹解释，使其适配“声明不再携带 Agent revision”的新协议；不得伪造一个 Agent 上报编号或删除拒收证据来适配界面。§15.9 的现有 v1 行为在本次文档变更后仍保持原样。
