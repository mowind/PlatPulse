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
- 本文件区分三种状态：已实现的 Server/Agent/API 能力、当前 `platpulse-web` 实际注册的路由，以及明确未实现的产品边界。Validator、Geo、Peer、Alert、Notification、Backup/Restore、Retention、Operation、Node Transfer、Recovery/Rotation 等不再统一视为“未来功能”：其中 Server/API 和后台工作器已有实现，部分扩展已注册 SPA 页面（例如 Settings 中的 Geo provider、Retention 策略与绑定影响预览页），其余仍只有 API；详见 §3、§8、§9 和 `docs/design/webui.md`。

---

## 2. 产品形态

产品分离参照 Komari（<https://github.com/komari-monitor/komari>），但监控对象是 PlatON Node 而不是服务器：

- Home：只读、以 Node 为中心的监控面。根路由 `/` 以 All Networks 展示 Active Node 卡片，并进入 Node Detail 展开公共投影。Node Detail 由最近两个连续 Block Summary 推导出块间隔，但不展示 Bounded Block History 列表。Site Access Mode 为 Public 时匿名 Guest 可读选定 Public GET/SSE 路径；为 Private 时 Owner 或 Viewer 登录后可读。
- Admin：认证后的 Owner-only 系统概览与配置面。当前 SPA 路由覆盖 Overview、Agents、Nodes、Networks、Settings、Sessions 与 Audit；Settings 现在包含 Geo provider 选择（Disabled / Local MMDB / IPinfo / GeoJS）。Server/Admin API 另外提供 Validator、Alert、Notification、Operation、Retention、Backup/Restore、Doctor、Transfer、People、Enrollment/Recovery/Rotation 等能力，但尚未全部注册为页面（Operation 台账、Task 详情、Doctor、Backup artifact 面与 Retention 策略/影响预览面已注册，见 WebUI §15.11–§15.13）。
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
7. 追加合法的 Block History，记录 coverage/divergence/gap 状态（受全局窗口约束；低空间保护生效时，被跳过的可选 metric history 也在此事务内累加到 `capacity_skipped_series`，见 §11.5）；
8. 计算 Inventory、per-Node 和 per-sample dispositions，写入完整 Report Receipt；
9. 在同一事务中评估 Alert/Notification side effects；
10. 提交事务（Geo 国家解析不在事务内，提交后只唤醒后台解析路径）；
11. 事务提交后才发布受影响资源的 Admin/Public SSE invalidation。

`partially_accepted` 表示同一个事务中部分 Node/sample 被接受、其余被拒绝，不表示半提交。任一步骤失败都回滚投影、历史、Receipt 和告警副作用；回滚不会发布 invalidation。Post-commit invalidation 是通知层行为，客户端必须用 REST 重新读取权威 DTO。低空间保护不改变上述事务边界：被跳过的可选 metric history 样本与它的缺口记账在同一次提交内完成，任一步失败同样整体回滚（§11.5）。

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

这些 family 表示当前实际 route 集合；每个 operation 的 GET/POST/PUT/PATCH/DELETE 方法、参数和响应以源 handler/OpenAPI operation 为准。备份创建与恢复是**离线操作**（[ADR 0008](../adr/0008-offline-server-backup.md)）：服务进程不创建 backup artifact，因此 `/api/admin/v1/backups` 只保留列表/详情/verify，`POST /api/admin/v1/backups` 随 `backup_create` Operation 一起移除。Session 撤销是 `POST /api/admin/v1/sessions/{session_id}/revoke`，不是 DELETE。运行时 handlers 还可能返回 OpenAPI 未列出的 typed `ApiErrorBody`（特别是 503/500），客户端必须把任何非 2xx 当作错误处理。 规则编辑与 Network/Node 继承覆盖的保存是版本安全的（issue #204）：`PUT /api/admin/v1/alerts/rules/{rule_key}` 与 `PUT /api/admin/v1/alerts/rules/{rule_key}/overrides` 都要求请求体携带 `expectedVersion`；与当前 composed version 不匹配时返回 `409` 加 `alert_rule_version_conflict` 的 `ApiErrorBody`，并在同一事务中不写入任何规则或覆盖变更。composed version 是整个规则聚合（baseline 加全部 Network/Node 覆盖）的并发令牌：baseline 编辑与覆盖 upsert 成功时都会追加一条不可变的历史记录并把 `alert_rules.version` 推进到新版本；`DELETE /api/admin/v1/alerts/rules/{rule_key}/overrides/{scope_kind}/{scope_value}` 只删除显式、可重建的覆盖、不带版本前置条件，但删除同样推进 composed version，持有旧版本的写入者必须重载后重审。

通知测试与命令对账由 issue #206 交付：`POST /api/admin/v1/notifications/channels/{channel_id}/test` 与 `POST /api/admin/v1/notifications/deliveries/{delivery_id}/retry` 的请求体都必须携带 `requestId`（1..128 字符，空白或超长返回 400 `notification_request_id_invalid`），`GET /api/admin/v1/notifications/requests/{request_id}` 返回该 request id 的持久 Server 命令结果。Server 用 `notification_requests` 台账做**请求级**去重：同一 request id 加同一意图重放时返回已记录的结果且不重发（`deduplicated: true`），同一 id 换意图返回 409 `notification_request_id_conflict`；被拒绝的命令（冷却、未配置、禁用等）不写台账，因此查不到结果。测试命令还受 `[notifications] test_cooldown_seconds`（默认 30，范围 1..3600）约束，命中时返回 429 `test_cooldown_active` 并带 `Retry-After`；台账保留期由 `dedup_retention_seconds`（默认 86400，范围 60..604800，且不得小于冷却）决定，过期行在读写时按不存在处理。这一层只保证 Server 命令不重复，**不等于**外部通道的 exactly-once 投递：delivery 仍是 at-least-once，重放只保证「同一条命令得到同一个 Server 结果」。retry 只接受 `retry_scheduled`/`failed`/`dead_letter`，`pending`/`in_flight` 返回 409 `delivery_already_queued`，终态或 suppressed 返回 409 `delivery_not_retryable`。

Delivery 的显式安全重试由 issue #207 交付（[WebUI §15.10](webui.md#1510-explicit-safe-retry-of-a-retryable-delivery-issue-207-delivered)）。重试只接受 `retry_scheduled`/`failed`/`dead_letter`；`pending`/`in_flight` 返回 409 `delivery_already_queued`，`succeeded`/`suppressed`/`cancelled` 等终态返回 409 `delivery_not_retryable`，两种拒绝都不改动该 Delivery。成功重试只把同一行 `notification_deliveries` 重新武装为 `pending`（清空 `next_attempt_at`），不动 `attempt_count`，也不改写 `delivery_attempts`：历史尝试、provider 结果与 last error 原样保留，下一次 worker 轮次按 `attempt_number = attempt_count + 1` 追加新尝试，并且不会新建 Notification Event、Incident 或业务状态迁移。重试后仍在 handoff 前重新检查 Silence、Maintenance Window 与主体删除：期间生效的抑制或删除会抑制或取消这次发送（`suppressed_by_silence:<id>` / `cancelled_subject_deleted`）且不记录新的 provider 尝试，不会盲发。retry 命令与测试命令共用同一 request id 台账：并发重复折叠为一条命令与一条 Audit，换 Delivery 使用同一 id 属于换意图返回 409，进程重启后仍可按 id 查到已记录结果。
运维任务可见性与 Doctor 诊断由 issue #208 交付（[WebUI §15.11](webui.md#1511-operations-task-visibility-and-doctor-diagnostics-issue-208-delivered)）。`GET /api/admin/v1/operations` 按 `created_at DESC, operation_id DESC` 返回最新一页（`limit` 钳制到 1..200，默认 50），`status` 与 `kind` 分别按 `OPERATION_STATUSES` 与 `OPERATION_KINDS` 校验；`GET /api/admin/v1/operations/{operation_id}` 缺失时返回 404 `operation_not_found`。`POST /api/admin/v1/operations/{operation_id}/cancel` 只记录取消请求：缺失返回 404 `operation_not_found`，非 `queued`/`running` 返回 409 `operation_not_cancellable`，`queued` 直接转为 `cancelled` 并写入 `finished_at`，`running` 记录 `cancel_requested = 1` 但保持 `running` 与空 `finished_at`，直到 worker 观察到；因此「已请求取消」不等于「已经完成」。`POST /api/admin/v1/doctor/run` 只入队一条 `doctor_run` Operation 并返回其 id，`GET /api/admin/v1/doctor` 返回最后一份报告及其 `currentRun`；Doctor 只诊断、不修复。这些是读与命令操作，不施加删除确认。

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
- Node Detail：用户批准的统一展示目标以 [WebUI Node Detail 契约](webui.md#node-detail-composition-page-home-node) 为准，不声明实现或验收完成，并取代 Validator 四 KPI／普通节点六 KPI 的分叉布局。保留 1280px shell 与 pinned Emerald：共用无卡片 Header（Node Health 圆点、名称、Sync、经证据确认的 Role、适用 Activity、inline 相对 Last report；下方完整 Node ID 与复制），六 KPI（Head、Sync、Peers、Uptime、Block interval、Transactions / block），同一 Chain state／Process／Host resources、条件 Validator performance、Latest 60 seconds 六图。桌面 KPI 三列两行，与地图约 5:6 分栏；地图沿用 Home 的轨道（低于 xl 保持 2:1 比例、达到 xl 用上游 22rem 固定带），KPI 网格拉伸填满该轨道高度、与地图上下齐平，地图不被压到 KPI 堆叠高度；手机 KPI 两列且地图在其后，角色不改变几何或顺序。仅 currentValidatorStatus=validator 展示紧凑单卡 3×2 performance；stale positive 保留且明确警告；仅 current not_validator 显示 Observer；unknown／无 Link／stale negative 不推断 Role、不展示 performance，身份原因在受影响数值旁就地可见。Process 显示 Collection Successful／Error／Disabled／Unsupported／Starting／Unknown，不从采集结果推断 Running／Stopped；Started 为绝对 UTC，Uptime 仅在 KPI，缺失可选行隐藏。Head lag 仅由当前有效且 high-confidence 输入比较：0 success、1–10 warning、>10 error，负值显示 ahead 而不归零；保留标注的 last-good 绝对高度。Host 三条细进度条和单行上下行速率保持共享 Host 口径。六图同序、同高、右上实时值、底部 legend、原 tooltip 与真实样本；CPU／Memory 保留 5／10／25／50／100% 动态轴。保留完整 ID 复制／展开与失败回退、Validator diagnostics、精确奖励值、独立 stale／error 解释和仅改变显示年龄的 Last report 时钟；不填充大量 Unknown、不扩展 Public API、不修改 Home／Admin。固定视口 360／390／768／1280／1440 验收角色切换与移动端，无 Bounded Block History 或历史导出。

Home 顶部为六卡片紧凑概览，顺序为 Active Nodes、Healthy Nodes、Cumulative blocks、Attention、Networks、Cumulative rewards。保留当前四项统计的 Network 筛选口径：Active 为选中 Active Node 数，Healthy 为其中 Healthy 数，Attention 为其余 Node 数（含 Unknown），Networks 为选中 Public Network 分组数；排序只改变 Node 顺序。累计两卡片是 Cross-Network Validator Overview：对当前选择内 Server 已按 Network 去重的累计出块/奖励精确数值相加，不重新累加 Node、不跨 Network 合并身份；奖励只是各 Network 原生单位数字相加，不表示同一资产余额或法币估值。每张累计卡片提供可访问的 Breakdown 对话框，保留精确值、逐 Network 独立的 eligible/known/stale 分母与计数、未关联 Node 和缺失/部分/last-good 语义，替代独立 totals 区域。整个页面仍以 1280px 为上限；桌面 >=1024px 概览左右约 5:6，地图占较大一侧，使六张卡片保持原四卡 2×2 布局的紧凑高度（44px 标题行内放标题与指标图标或 Breakdown 控件，数值在其下，不设页脚行），左侧三列两行，六卡在地图带内贴底对齐，使概览到下方网络分组的间距与网络分组到节点卡片的一致（16px）；右侧地图 >=1280px（xl，页面达到上限、列宽不再变化）改用上游 22rem 固定带，1280px 以下保持 2:1 比例轨道；两个图层都填满轨道盒（left/top/width/height 100%），不再按世界自身比例定高——2:1 轨道比该比例更扁，定高会溢出并切掉两极——因此任何宽度都不裁切。不沿用 37:61/232px 固定带。更窄屏幕沿用上游构图：地图在先、统计在后（DOM 仍把统计放在前以利读屏，靠 CSS order 在 lg 以下还原该视觉顺序），>=640px 三列、手机两列。Node grid 保留 auto-fill/minmax(300px,1fr) 列规则；Node 卡片自然高度，不因相邻卡片拉伸。统计与地图共用浅绿渐变与淡网格，Network 筛选同步改变概览、下方 Node 列表和地图范围。地图只使用 Server 提供的国家计数与国家代表点：国家按 Peer 记录逐 Node 计数（不按 IP 去重），未知国家不绘制，无法绘制或缺少代表点的国家保留可访问文字统计，不使用 `[0, 0]` 或随机点回退，也不表示受监控 Node 的部署位置。底图为固定版本、本地托管的世界国家几何（Natural Earth 1:110m Admin 0 Countries，公有领域），由 `platpulse-web/scripts/build-world-geometry.mjs` 离线生成并随 WebUI 静态资源同源托管；运行时不访问地图 CDN 或在线瓦片。Geo 停用、底图加载失败或地图渲染失败只在概览局部降级，不影响统计、筛选、排序与 Node 卡片。

Home 的列表过滤与搜索全部在 Web 端、基于同一份 Public Projection 完成：`GET /api/public/v1/networks` 不接受任何查询参数、一次返回全部 Active Node，因此不新增 Server 搜索端点、不分页、不发起第二次读取。搜索是对三个公开字段的大小写不敏感子串匹配——Node 显示名、Node ID、Network 显示名（无显示名时仍可由 Node ID 找到）——Linked Validator 标识与名称、Host/Agent 名称、Health 与身份理由文本都不参与搜索；搜索与 Health、Validator status 按 AND 组合：Health 取 `all|healthy|unhealthy|unknown`，Validator status 直接使用 Server 的 `validator.currentValidatorStatus`（`all|validator|not_validator|unknown`），缺失 Link 记为 unknown 而不写成 not_validator，绝不从 Activity、Consensus 成员或缺失区块推断。只有 Network 选择会改变六张概览卡、Attention、累计卡片与 Peer 国家地图，搜索/Health/Validator status 只收窄 Node 列表、排序只改变顺序，列表同时给出匹配数与作用域内总数（`Showing <matching> of <in-scope> Active Nodes`）以及「概览与地图覆盖整个 Network 选择」的说明，空态区分「该 Network 选择没有 Active Node」与「没有 Node 匹配这些过滤条件」，两者都不渲染成故障、不编造 0。Home 提供紧凑卡片与列表两种视图（`view=card|list`，默认 card 且不写入 URL）：列表是一张语义化表格，含 Node／Network／Health／Current Head／Peers／Process CPU／Process memory 七列，列宽固定、窄屏只在列表内部横向滚动而不造成页面水平溢出，Node 名称与 Health 固定在列表左边缘，滚动到任何位置都能读到身份与健康，且手机宽度下 Node／Network／Health 三列在未滚动时即全部落在列表可视带内；被保留（last good）的 Peer 计数保留数值并在其下方就地标注快照口径与不再当前的维度（例如 last good (collection error, freshness stale)），完全当前与从未观测到的计数不标注；手机宽度下工具栏首行改为换行排布，Network pill 行独占整行宽度、排序与视图控件落到下一行——视觉排布变化，DOM 顺序、阅读顺序与 Tab 走查顺序（Network → 排序 → 视图 → 搜索 → Health → Validator status → 首个卡片）均不变，已交付的「pill 行只在自己内部横向滚动」口径也不受影响；排序键为 Health／Name／Current Head／Peers／Process CPU／Process memory 六种，四种数值排序把「从未观测到」的值排到末尾并保持真实 0（0 与 Unknown 绝不互相代替），Health 排序用 Health 自己的三个词按同一条「未经 Server 确认的取值排末尾」口径分级（unhealthy 在先、healthy 随后、Home 只能读作 Unknown 的 health 排最后，即 attention 仍领先但未经确认的 health 绝不排在已确认的 health 之前），所有排序都以 Node 身份作稳定 tie-break，重取后顺序不变。`network`/`q`/`health`/`validator`/`sort`/`view` 六个查询参数按固定顺序读写、默认值不写入 URL，刷新、书签、前进后退或直接导航都恢复链接所载内容；该 URL 明确不是受保护的分享链接、也不是授权凭据，不授予任何权限。本部署无法识别的取值就地回退为默认并显式告知读者（点名参数与取值、说明回退到默认、其余部分照常生效），载入时不改写 URL，只有读者操作才写回，避免前进/后退抖动；一次读者操作只写一条历史——Network pill 在获得焦点与按下时各上报一次取值，重复请求地址栏已有的或正在写入的同一 URL 会被丢弃，使「后退一步」恰好撤销一步；Network key 是否过时只在 Projection 可用后判定（`networks === null` 表示尚未加载，选择保持临时有效、不判为过时）。空值一律视为「未提供」而非拒绝，Home 不拥有的参数（`to`）原样保留。

Home 不展示：凭证、RPC Endpoint 原文、内部错误堆栈、Agent/Host 拓扑、任何操作入口。已退休/已删除/未知 Node 使用不泄漏信息的 unavailable 文案。Site Access Mode 为 Private 时 Home 路由要求已认证 Owner 或 Viewer；为 Public 时允许匿名 Guest 读取允许的 Public projection 路径。

### 9.2 Admin

认证后使用，但 Admin 路由由 Owner role guard 保护，Viewer 只能访问 Home：页面组为：

1. Overview；
2. Agents；
3. Nodes；
4. Networks；
5. Settings（按顺序包含 History Window 与 Site Access Mode）；
6. Sessions 与 Audit。

Server Admin API 另有 People、Restore、Transfer 和 Agent credential operations；当前 SPA 没有对应注册路由。Validator 的自动身份覆盖与当前状态已由 issue #218 注册为 `/admin/validators` 与 `/admin/validators/:validatorId`（见 WebUI §15.20）：`GET /api/admin/v1/validator-identities` 与 Node 详情内嵌的 `validator_identity` 来自同一投影，识别状态（Network + 完整 P2P 公钥）与当前质押状态分开呈现，未识别不写成「非 Validator」。Retention 策略读取/允许编辑/影响预览已由 issue #210 注册为 `/admin/retention`（见 WebUI §15.13）：Server 权威 preview 绑定 policy version、scope 与固定 cutoff，执行只按 preview id 排队，过时 preview 被拒绝且不排队；计数是估计而非冻结行集。Alert/Incident/Rule/Silence/Maintenance 与 Notification 的测试、Event/Delivery 历史和请求对账页面已分别由 issue #203、#204、#205、#206 路由（见 WebUI §15.6–§15.9），Operation 台账、Task 详情与 Doctor 页面由 issue #208 路由（见 WebUI §15.11）；Backup artifact 列表、详情与只读校验请求由 issue #209 路由（见 WebUI §15.12），创建与恢复仍是离线命令（ADR 0008）。

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
Metric History       Node 与共享 Host metric history/export（原始样本按 raw_metric_sample family，默认 24 小时；两个聚合层把可回答窗口延伸到 30 天，见 §11.6）
Sync/Consensus State Node 的同步/共识状态变更日志（`node_state_observations` 与它的账本 `node_state_series_state`，按 observation_state family 默认 30 天；同一窗口的五个区块高度数值走 Metric History，见 §11.7）
Operations/Audit     Alert/Notification/Operation/Backup/Restore/Doctor 与审计记录
~~~

Server 仍不会用零值填充缺失区间；Retention 按 data family 分别配置，Block History Window 是其中独立的一项。

每个 family 的保留契约由 Server 的 retention catalog 声明（`crates/platpulse-server/src/retention.rs`）：策略类别（raw / investigation / contract）给出安全下限（raw 至少 24h，investigation 与必要的 aggregate/state 至少 30d），family 自有 min/max/default 只能在此之上收紧或放宽，Server 拒绝任何低于下限的值；未启用或未支持的 family 不声明任何 cleanup target，因此不会暗示已保存其未实现的历史。支持的 family 声明其 cleanup target（固定 SQL、有界批量、恰好一个绑定参数），执行时按 preview 冻结的 cutoff 逐批运行；估计计数是 Server 给出的上界而不是冻结行集，也不是执行配额：某批返回的行数少于批量上限即证明该 cutoff 之后已无过期行，视为该目标完成；某批满额则继续；实际释放行数可以超过估计值，结果按 Server 记录的真实释放数报告。plan 在排队瞬间冻结（scope、policy version、每个 family 的 cutoff），执行期间不再读取实时策略，因此运行中的策略变更既不会加长也不会扩大正在执行的范围。preview 的 policy version 由该 family 记录的值与时间戳摘要而成，并使重预览判定同时比较 retention_days 本身，因此任何会改变计划内容的编辑都被判为过时；preview 校验与入队是两条语句，排队瞬间的校验存在极窄窗口，但已冻结的 plan 使该窗口无法扩大或加长将要释放的行（把校验并入同一事务留待后续票据）。已记录的历史在更长保留下不会被自动缩短。

按批准计划执行、命令去重与诚实的取消（issue #211）：每次清理执行都必须绑定一个仍然有效的 preview，Server 在执行前重新校验 policy version、scope 与每个 family 的 cutoff，任何一项变化都返回 `409 retention_preview_stale` 且不入队。Owner 的一次确认是一个持久化的命令身份（请求体中的 `requestId`，与中间件生成的 HTTP 关联 id 不同）：同一 `requestId` 的重复提交（双击、第二个标签页、响应丢失后的重试）不会产生第二次清理，Server 按 `operation_requests` 的 `(kind, intent_fingerprint)` 唯一索引回放已记录的结果，Retention 的 intent fingerprint 是 `retention_run:<previewId>`；同一 `requestId` 用于不同意图返回 `409 operation_request_id_conflict`，而不是静默执行一个新动作。同一时刻只允许一个 queued/running 的 `retention_run`（部分唯一索引），因此第二次确认要么回放已记录的任务，要么被明确拒绝（`409 retention_run_in_progress`）；命令记录的存活期与 preview 一致（24h）。取消是显式记录而不是回滚：queued 的清理在取消当刻由 Server 写入终态与结果（已释放 0 行、未完成目标数、按 family 明细与说明），running 的清理在下一个安全检查点停止并记录已释放行数与未完成目标数；已释放的数据永不回滚。重启后被中断的清理按已记录的 plan 汇总已释放行数与未完成目标数，而不是一条通用错误。释放数据与其记账在同一次提交的同一事务内完成，因此不存在“行已删除而记录仍称从未触碰”的状态。从未开始且没有计划工作的任务不编造结果（queued 的 Doctor 诊断取消后 `result_json` 保持为空，`doctor::last_run` 因此不会把一次取消当成一次诊断报告）。

### 11.5 低空间保护（issue #212）

Server 启动时解析可选的 `[capacity]` 段（crates/platpulse-server/src/config.rs），由 crates/platpulse-server/src/capacity.rs 按 hysteresis 采样挂载点可用空间：

- `enabled = true` 时必须同时给出 `pause_below_bytes` 与 `resume_above_bytes`，不设默认阈值；契约是 `resume_above_bytes >= pause_below_bytes > 0`，采样间隔限 5..86400 秒（默认 60）。缺少阈值时启动直接失败并说明原因，而不是替部署方猜一个 GiB 数字。
- 进入保护时向 `capacity_protection_intervals` 写入一行，用当时的真实容量填 `opened_total_bytes`/`opened_available_bytes`，`started_reason` 为 `low_space`，并以部分唯一索引保证同一时刻最多一个未结束区间；只有恢复到 `resume_above_bytes` 以上才结束该区间（`ended_reason` 为 `resumed`），保护被配置关闭时以 `protection_disabled` 结束，两条路径都保留原始开区间证据供审计。
- 保护生效期间 Report 仍完整校验、当前投影与 Report Receipt 照常写入，但可选的 metric history（Host/Node series）不再追加：每次被跳过的样本在同一 ingestion 事务内累加到 `capacity_skipped_series`（`scope_kind`/`scope_key`/`metric`/`dimension` 加 `skipped_count` 与首个/最近跳过时间戳，`dimension` 是 Host 存储系列的挂载路径、其余序列为空串，见 §11.6 的 issue #215），因此样本未落库与缺口已记账要么一起提交、要么一起回滚；记账失败会回滚整个 Report 并返回可重试的 503，历史因此不会静默丢失，也不会为了腾空间删除已有数据或降低精度。
- 跳过只作用于可选 metric history 的追加。Block History、Receipt、LastGood 与 current projection 的语义不变，Receipt 的 samples/disposition 也不把跳过的可选样本算作已接受；可见缺口由 `capacity_skipped_series` 与 Admin capacity 面表达（见 docs/design/webui.md 第 15.11 节）。
- 采样失败是 fail-open：保留当前 state 并记录 `sampling_error`，不因读不到 statvfs 就伪造缺口。重启时 `reconcile` 接管已打开的区间：仍低于 pause 阈值就继续保护，否则以 `resumed` 或 `protection_disabled` 结束。
- Admin 通过 `GET /api/admin/v1/capacity`（Owner-only）看到带年龄的容量状态、最近区间与被跳过的序列；Doctor 的 storage 检查在保护生效时报 FAIL、未配置时报 NOT_CONFIGURED、采样异常报 WARNING；`/metrics`（独立内部监听器，主监听器按设计把该路径当作 not-found）暴露 `platpulse_capacity_total_bytes`、`platpulse_capacity_available_bytes`（未知时省略而不是写 0）与 `platpulse_capacity_paused`。

### 11.6 可信的指标历史：24 小时原始样本、30 天聚合层与共享 Host 系列（issue #213、#214、#215、#216）

24 小时原始历史的可信度取决于两件事：保留窗口由策略而不是上报节奏决定，以及「这个序列被观察到什么」本身有记录。issue #213 因此做了三处改动（issue #215 后来把同一个引擎用在第二个 scope 上：Agent 采集一次、由它所有 Node 共享的 Host 系列，见本节末）：

- **删除每序列硬裁剪。** ingestion 里原有的 `NODE_METRIC_SAMPLES_PER_SERIES = 64` 会每序列只保留最后 64 个样本，于是「能看到多久」变成上报频率的函数（默认 5s 节奏下约 5 分钟），任何更长的曲线都只是幸存者。现在 raw 样本的存活期完全由 retention family 决定。
- **新 family `raw_metric_sample`**（label `Raw Metric Samples`，策略类别 raw）：default 1 天（即 24 小时），min 1 天（等于 raw 类别下限，因此 24 小时窗口无法被配置取消），max 30 天，声明两个 cleanup target：`node_metric_samples` 与 `host_metric_samples`（均按 `observed_at < cutoff`）。Host series 一并纳入，否则删掉旧裁剪后 Host 样本会无界增长；issue #215 后来确认 Host 不需要自己的 family：它与 Node 共用同一个 24 小时窗口，另开一个 family 只会再重建一遍 `retention_policies` 的 CHECK 列表。行数估计是 Server 给出的上界而不是冻结行集，也不是执行配额。 新增 family 必须同时重建 `retention_policies` 的 CHECK 列表（0066 migration 沿用 0022/0027/0029/0035/0059 的做法）：CHECK 不认识的值在 seed 时会被静默跳过，策略既不出现在 Owner 面，其 cleanup 也永不运行。Node 表每 Report 的清理批次用 `NODE_METRIC_CLEANUP_BATCH = 4096`（编译期断言它必须覆盖一份最大 Report 的 256 × 10 行；issue #217 把三个共识高度系列并入 Node 侧后，这个批次由 2048 提到 4096）；Host 表自 #215 起有自己的批次 `HOST_METRIC_CLEANUP_BATCH = 512`，并有编译期断言（crates/platpulse-server/src/retention.rs:224）要求它覆盖一份最大 Host Report 的 `HOST_METRIC_SERIES.len() + 2 × MAX_HOST_MOUNTS`（= 10 + 2 × 128 = 266）；聚合批次 `AGGREGATE_CLEANUP_BATCH = 2048` 也补了一条同样形状的断言（crates/platpulse-server/src/retention.rs:250），因为 Host 现在每份 Report 都会给每个 tier 各加一个桶。一份 Report 现在会陈述它采到的每个 Host 量，两个按挂载点区分的系列就足以让 Node 的那个批次跟不上一个挂满挂载点的 Host。批次小于一份 Report 自身写入量时，过期速率会落后于写入速率，backlog 会一直增长，直到低空间保护暂停这个 family 本来要保住的历史。
- **新表 `node_metric_series_state`**：`(node_id, metric)` 主键，外加 `first_observed_at`、`last_observed_at`、`last_received_at`、`observation_count`、`replayed_count`、`corrected_count`。它不保存任何数值，是序列的账本而不是第二份历史；样本被 retention 释放后账本仍在，因此「首次被观察」与计数不会随样本一起消失。0066 migration 从现有 `node_metric_samples` 回填 `MIN/MAX/COUNT`，升级不会把已经存在的历史说成从未发生；同时新增 `(observed_at, node_id, metric)` 与 `(observed_at, agent_id, metric)` 索引支撑按时间范围的读取，以及 `capacity_skipped_series (scope_kind, scope_key, metric, last_skipped_at)` 索引：0065 的主键以 `interval_id` 打头，而这个 series 谓词在每次 Owner 读取时都要执行。

计数规则同时满足「重放不得抬高观测数」与「携带的 last-good 不是新观测」：写样本前在同一 ingestion 事务里读取 `(node_id, metric, observed_at)` 已存值，并按固定顺序判断（crates/platpulse-server/src/metric_history.rs:887 `classify_delivery` 返回 crates/platpulse-server/src/metric_history.rs:840 `Delivery`）：① 该时刻在序列 high-water mark（`last_observed_at`）之后 → `Observed`（canonical 时刻按文本比较，因此这里就是字符串比较）；② 该时刻已有样本 → 值相同为 `Replay`，值不同为 `Correction`；③ 没有样本但该时刻仍在 raw 窗口内（crates/platpulse-server/src/metric_history.rs:961 `outside_retained_window`）→ `Observed`，即乱序到达或填补空档的读数，因此保留行数与账本始终相等；④ 没有样本且该时刻已早于 cutoff → 视为 `Replay` 且永不计数：已过期的重放与「旧到无法保留的观测」无法区分，而把它计入正是唯一能让携带的 last-good 抬高生命周期计数的路径（代价是这类投递一律不计，计数只会少报、不会虚高）。携带的 last-good 保留其原始 `latest_observed_at`，因此永远不落在 high-water mark 之后的分支里：只有真正的新观测会推进观测数与覆盖时长。`ON CONFLICT` 只更新 value，所以一行保留「首次写入它的那次投递」的 receipt 时间；`last_observed_at` 与 `last_received_at` 由同一条 CASE 一起移动，报告的 delay 因此永远属于同一个真实样本。

缺口在读取时推导，不落库（低空间保护造成的暂停已经由 #212 的 `capacity_skipped_series` 记账）：Server 从不被告知 Agent 的采样间隔，因此用观测到的节奏（窗口内相邻已存样本的最小正间隔）而不是假设值，阈值 = `max(3 × cadence, 120s)`，其中 cadence 先被限制在 `MAX_OBSERVED_CADENCE_SECONDS = 300`（Agent 的 `collection_interval_seconds` 上限就是 300，见 crates/platpulse-agent/src/config.rs:164），因此阈值上限 900s——一个节奏较慢的序列不会显得永远断裂。相邻已存样本间隔达到阈值即构一个缺口，`kind` 为 `collection_gap`（该区间没人观察）或 `protection_pause`（保护区间覆盖它），`reason` 用 Server 自己的措辞；窗口末尾仍在暂停中时补一个 paused-tail 缺口，并裁剪到本次请求窗口的上界，因此窗口之外不会被报告。缺口用「夹住它的两条已存样本」定界，`skipped_count` 只在所报区间完整覆盖整个暂停区间时才附带（`capacity_skipped_series` 每个区间只有一个计数、没有逐次跳过的时刻，被裁剪的时段不能认领这些丢失）。`coverage_seconds` 只累加被相邻样本证明的时段，绝不跨缺口，也绝不假设缺口里的值：一对样本间隔小于阈值但已知暂停落在其中时，暂停时长会从覆盖里扣除。两个同为聚合桶的相邻点还要额外满足「它们各自代表的窗口真的相邻」这一条（crates/platpulse-server/src/metric_history.rs:594 `windows_abut`）：桶只写在实际有观测到达的窗口上，因此中间整整少一个桶窗口时，那段就是没人观察过的时段，无论节奏与窗口宽度的宽限怎么算都算缺口——宽限覆盖的是「两个真的被计数过的窗口之间」那一段，而不是一个从未被写入的窗口，否则相隔十分钟的两个 5m 桶会跨过中间那个五分钟窗口被连成一条线。跨 tier 与跨粒度的交接按构造就相邻（粗桶答完自己的窗口，细 tier 从它的结束边界接着答），一对「桶 + raw」的点则只保留节奏规则。聚合桶只证明它的两个端点，不证明两点之间连续（review F1）：写入时增量维护 `max_gap_seconds`（该桶两个相邻被计数观测之间最宽的间隔，crates/platpulse-server/src/metric_history.rs:1059 `AGGREGATE_UPSERT_SQL` 里取 `MAX`，判据来自 crates/platpulse-server/src/metric_history.rs:1093 `candidate_gap_seconds`），读取时只有当它小于本次答案判据节奏所要求的阈值时才把 `span = last_observed_at - first_observed_at` 计入覆盖，否则这个桶对覆盖的贡献是 0——它的计数、极值与跨度照常返回，空洞由 `maxGapSeconds` 交给界面在那里断线。因此覆盖绝不用「桶宽 × 桶数」这种下界冒充精确值；判据节奏取该序列在答案 raw 窗口里观测到的节奏，桶内没有 raw 证据时退化为 `grain_seconds / sample_count`（crates/platpulse-server/src/metric_history.rs:622 `continuity_from`）。每段覆盖都裁剪到请求窗口之内。delay 与时钟可疑同样在读取时从保留的 `(observed_at, received_at)` 对推导：`delay = received_at - observed_at`，当观测时刻比接收时刻还晚超过 300s 时标记 `clock_suspect` 并给出说明——不保存任何可能漂移的标志位。

**30 天聚合层（issue #214）。** 原始样本之外的可回答窗口由两个聚合层承担：grain 60 秒（`1m`，7 天）与 grain 300 秒（`5m`，30 天）。累加发生在 ingestion 的同一 receipt 事务内、与 raw 写入同进同出，且不取决于 raw 那一行是否真的落库：每个被计入的观测（`Observed`）都同时累加两个 tier，因此一个「被计数但已按 raw 策略 release」的观测仍然留下一个带计数与极值的桶，而 `Replay` 从不重复累加（crates/platpulse-server/src/http/report_ingestion.rs:893 起按 `Delivery` 分派：`Observed` 累加、`Correction` 重述、`Replay` 不动）。桶按确定性 UTC 边界对齐（crates/platpulse-server/src/metric_history.rs:200 `aligned_bucket_start`：对 Unix epoch 按 grain 整除再格式化回 canonical 时刻），一个桶保留 `sample_count`、`min_value`、`max_value`、`last_value`、`first_observed_at`、`last_observed_at`、`last_received_at`、`max_gap_seconds` 与 `updated_at`；真正没有观测的桶绝不被 0 填充，空就是空。`max_gap_seconds` 是「桶内已测到的空洞」的确凿证据：它由两个相邻被计数观测的时刻差算出并在 ingestion 时取 `MAX`，不需要任何阈值假设，不会因为后来 release 掉 raw 行而消失，一次修正也只能把它抬高、不能压低；因此读取侧只能在它小于阈值时才敢声称覆盖，而不能反过来用一个 0 冒充「已测过且连续」。

**重述只覆盖它真有证据的范围。** `Correction` 到来时（crates/platpulse-server/src/metric_history.rs:1211 `recompute_aggregates`）按「桶里还剩多少证据」分三种情形：桶内没有任何已存 raw 行则整桶跳过，桶从不被发明、也从不被清空；仍在 raw 窗口内的该桶保留行数等于桶里记下的 `sample_count` 时，这些行就是全部证据，因此用它们重算整个包络（min/max/count/last_value/first_observed_at/last_observed_at/last_received_at）；保留行数不等于 `sample_count` 时走携带路径（crates/platpulse-server/src/metric_history.rs:1190 `BUCKET_CARRY_SQL`，review F5）：极值用 `MIN`/`MAX` 无条件拓宽——被修正的那个观测即使不是最新读数也贡献过它的极值，绝不能因为重述而把已证成的尖峰收窄——`last_value`/`last_observed_at`/`last_received_at` 三者一起、且只在修正不早于该桶最新观测时刻时才一起前移——一个桶说「最新读数是 X」，就是在说 X 的观测时刻是 `last_observed_at`，而 Admin 契约正是把这个 `(value, last_observed_at)` 对读成「点的值」与「它的 delay」，因此值与它被观测到的时刻必须同进同退（review round two：只让值前移会把修正后的读数挂到另一个观测的 instant 上，那个 instant 自己的读数从未改变，报告出来的 delay 也随之属于一对并不存在的观测）。携带到的那个时刻本身就是该桶窗口内一次真实的观测，所以这一对始终落在有证据的范围内：`until`（即 `last_observed_at`）只参与静默长度的计算、不参与窗口相邻的判定（crates/platpulse-server/src/metric_history.rs:594 `windows_abut`），而桶是否证明空洞仍由不动的 `max_gap_seconds` 单独裁断。`first_observed_at`/`sample_count`/`max_gap_seconds` 不动。计数永不因重述而改变，陈旧但真实的极值也不会因为证据不全而被抹掉。

**迁移与保留。** 新表 `node_metric_aggregates`（0067 migration）以 `(node_id, metric, grain_seconds, bucket_start)` 为主键，`grain_seconds` 只允许 60/300，`sample_count >= 1` 且 `max_value >= min_value`，另有 `node_metric_aggregates_expiry_idx (grain_seconds, bucket_start)` 支撑到期扫描；`SERVER_SCHEMA_VERSION` 升到 67，node purge 一并拥有该表。保留侧新增两个 family：`one_minute_aggregate`（7/7/7，策略类别 contract——一个 tier 的可服务窗口本身就是它的契约）与 `five_minute_aggregate`（30/30/30，策略类别 investigation，即注册的 30 天 investigation 下限）；0067 沿用 0022/0027/0029/0035/0059/0066 的做法重建 `retention_policies` 的 CHECK 列表，并把 `one_minute_aggregate` 归一化到 7/7/7 且 `supported = 1`（review F4）：升级前这一行可能是 #213 时代留下的占位值，也可能是 Owner 手工改过的 30 天窗口，而 1m tier 实际只服务 `[now - 7d, now - 24h]`，把窗口留在 30 天只会让 cleanup 多留 23 天读取侧永不取用的桶。同一原则在 seed 与执行两处重复落实：`ensure_seeded`（crates/platpulse-server/src/retention.rs:840）对固定窗口 family 用 `ON CONFLICT DO UPDATE` 把 catalog 元组写回（只改 `retention_days`/`min_days`/`max_days`/`supported`，`enabled`、`updated_at`、`updated_by` 保留，Owner 原本的编辑仍留在 `audit_events` 里可查），`aggregate_retention_days`（crates/platpulse-server/src/retention.rs:775）对这两个 family 直接返回 catalog 的窗口而不是读库里的值。一个 tier 的可服务窗口就是它的契约，这句话因此在迁移、seed 与 cleanup 三处都成立，而不是只在 catalog 里成立。`cleanup_expired_metric_aggregates` 在 Server 启动时与每次 Report ingestion 之后各运行一次，固定 SQL、有界批量 `AGGREGATE_CLEANUP_BATCH = 2048`，并带编译期断言「一批必须覆盖一份最大 Report」（256 × 5 行）。两个 family 各按自己的窗口释放：过期判定以 `grain_seconds` 分开，因此一个 10 天前的桶不会被 1 分钟 tier 的清理带走——只有粗 tier 还在回答它。到期判定走 `release_cutoff`（crates/platpulse-server/src/retention.rs:632）：对这两个 family 把 cutoff 再往前推一个桶宽，因为一个桶只有在自己的整个宽度都落在窗口之外时才不含任何窗口内的观测；代价是每个 tier 多留一个桶，换来的是「读取侧仍可能问到的那一个跨界桶永不被提前释放」（review F2）。preview 的行数估计与实际删除用同一个 cutoff，因此计划与执行不会各说一套。规模上每条 Node series 在 30 天里约 18.7k 行聚合（7 天 1m 的 10080 行 + 30 天 5m 的 8640 行），五个 series 的一个 Node 约 93k 行，guard 按这个量级而不是按 raw 窗口估算。

**边界的所有权：tier 的分界一定落在桶边界上。** 读取时（crates/platpulse-server/src/metric_history.rs:1350 `load_range`）先把交接点推到真实桶边界再读任何东西，因为 raw 区间自己的下界就是其中之一（crates/platpulse-server/src/metric_history.rs:223 `aligned_bucket_end`）：raw 从「cutoff 之后（含）的第一个分钟边界」开始（前提是 1m tier 真的持有那一分钟，见下）。请求起点落在 raw 窗口之内（`from >= raw_cutoff`）时，raw 区间就从请求起点开始，聚合 tier 一律不被咨询：跨界的那一分钟整个落在请求窗口之外，窗口内没有任何观测需要它的桶，因此 1 小时/6 小时/24 小时这类答案就是一段 raw 样本，不会多出一条「被查询但没有证据」的 tier。请求越过 raw cutoff 时（`from < raw_cutoff`），raw 才从「cutoff 之后（含）的第一个分钟边界」开始（`raw_from = ceil_minute(raw_cutoff)`），1m 的上界正是这条边界，5m 的上界是 `ceil_5m(now - 7d)`、raw 边界与答案上界三者中最早的那个，而下界一侧 `from` 落在某个桶内部时向下对齐到该桶起点。两条规则各有理由：tier 的 floor 落在桶内部时，那个桶同时装着 horizon 之内与之外的观测，raw 行 release 之后它就是那段区间仅存的证据，整桶回答好过把已证成的观测丢掉；tier 交接处则向上取，因为跨界的粗桶在写入时已经把它整个窗口的观测累加过，它整桶回答、细 tier 从它的结束边界接着回答。因此没有任何时刻被两层各答一次，也没有一段已被服务区间里的观测因为「落在错误的桶里」而被丢掉（review F2）。这条交接还有一项前提（review N2，判据在 review round two 收紧为计数）：只有当 1m tier 真的把那个跨界分钟「数全」了，才把 raw 下界推到它的结束边界——`handed_over` 成立的条件是该分钟有桶、且桶的 `sample_count` 不小于该分钟仍然存留的 raw 行数（crates/platpulse-server/src/metric_history.rs:1418 用 `straddling_minute_ledger` 一次读回这两个数）。聚合层是事后引入的、0067 也从不回填，因此 #214 之前就已存在的序列手里只有保留窗口内的 raw 行、一个桶都没有；此时 raw 保留跨界那一分钟本身，因为这些样本是那段区间仅存的证据，而 tier 里没有任何桶会把同一分钟答第二次——「每条观测只被答一次」因此照旧成立，只是回答它的是唯一持有它的那一层。探测问的不再是「桶是否存在」，而是「这个桶是否数过这一分钟里还留着的每一条观测」：0067 从不回填，升级之前就存在的原始行没有任何桶数过，升级之后的第一条投递于是开出一个只覆盖它自己的桶，这一分钟的存留行数因此多于桶的 `sample_count`；此时整分钟改由它自己的 raw 行回答，1m tier 的上界也就止于这一分钟的起点（raw 仍然持有它），而不是把更早那一条存留行留在承诺的 24 小时窗口里无人回答。这条判据在生产里不会误判：raw 行只在它自己的聚合 family 到期时才被释放（那是年），因此在 24 小时 raw 窗口之内，只要 tier 真的数过这一分钟，桶的计数就不会小于该分钟还留着的行数；一个空想的桶也绝不会被用来占位。代价是两处已披露的粒度让步：刚进 24 小时窗口的最多 1 分钟由 1m 桶（而不是 raw 样本）回答，刚进 7 天窗口的最多 5 分钟由 5m 桶回答——那些观测在桶里，只是以桶的粒度交出。与请求窗口相交的桶一律整桶回答，因此一个点的坐标可以早于 `from`：`coverage_seconds` 会被裁剪到请求窗口内，桶自己的计数与极值保持完整。空区间被跳过而不是用一个空答案占位。聚合层是事后引入的，因此 #214 之前的时段不保留任何聚合证据，缺就是缺，绝不从相邻 tier 反推。

**一份预算，从最新的 tier 开始花。** 一次答案只花一份预算：raw 先花，接着 1m，最后 5m；第一个花不起但仍持有证据的 tier 由 `EXISTS` 探测确认（crates/platpulse-server/src/metric_history.rs:1697 `BUCKET_EVIDENCE_EXISTS_SQL`），答案据此置 `truncated = true` 而不是悄悄丢掉一整层；一旦在某一层停下，更旧的 tier 就不再读取，因此被省略的时段永远是一道缝（seam），绝不在中间留洞。`continuation` 是最旧被返回的坐标并作为排他游标，下一页既不重复也不跳过任何点。

**Host 证据属于 Agent，不属于它的任何一个 Node（issue #215）。** 一份 Report 对每个 Host 量只陈述一次，Server 也就只存一次：六个 Node 站在两台 Host 上，计费与计数的是两套系列，没有任何 Node 拥有自己 Host 总量的副本（crates/platpulse-server/src/http/report_ingestion.rs:807 `host_series_samples`；该函数只列出 Report 真正携带的量，缺失的读数不是 0）。因此同一条共享系列有两个问法：`GET /api/admin/v1/agents/{agent_id}/metric-history`（回答里 `nodeId` 为 `null`）与 `GET /api/admin/v1/nodes/{node_id}/host-metric-history`（`nodeId` 为该 Node，`scope_kind = "host"`、`scope_key` 是 `agent_id`）；既有的 `GET /api/admin/v1/nodes/{node_id}/metric-history` 仍然只答 Node Process 系列（`scope_kind = "node"`）。两个 kind 互相拒绝对方的 metric，回答 400 `invalid_metric`；而一个从未被观察到的 `dimension` 不是错误，它是「从未观测过的系列」：200、`items` 为空、`series.observed = false`，因为「这条挂载路径没人上报过磁盘」本身就是关于这条系列的事实。回答里点名系列属于哪个 Agent 不是修饰：在 Node 页面上，这条历史本就是它背后那台机器的历史。

**一个引擎，两个 scope：泛化而不是平行的 Host 模块。** 同一份 ingestion 写入、读取、tier 与缺口推导对两个 scope 只差三件事：写哪几张表、哪一列指出所有者（Node 的 `node_id` 或 Agent 的 `agent_id`）、以及序列身份是否带 dimension（crates/platpulse-server/src/metric_history.rs:145 `HistorySchema`，`NODE_HISTORY` 与 `HOST_HISTORY` 两份常量分别是 crates/platpulse-server/src/metric_history.rs:160、:171）。表名与占位由模板渲染（crates/platpulse-server/src/metric_history.rs:190 `HistorySchema::sql` 替换 `{raw}`/`{agg}`/`{ledger}`/`{scope}`/`{dim_column}`/`{dim_bind}`/`{dim_predicate}`，无 dimension 的 scope 直接丢掉这几个片段），序列身份由 crates/platpulse-server/src/metric_history.rs:229 `SeriesScope`（crates/platpulse-server/src/metric_history.rs:237 `SeriesScope::node`、crates/platpulse-server/src/metric_history.rs:250 `SeriesScope::host`）表达。Node 侧仍是 5 条系列，Host 侧是 10 条（crates/platpulse-server/src/metric_history.rs:116 `HOST_METRIC_SERIES`）：`cpu_percent`、`memory_used_bytes`、`memory_total_bytes`、`load1`、`load5`、`load15`、`network_rx_bytes_per_sec`、`network_tx_bytes_per_sec`、`disk_used_bytes` 与 `disk_total_bytes`。刻意不写一套平行的 Host 模块：两套实现会让 tier 边界、缺口判据与账本语义各自漂移，而它们必须在两个 scope 上永远一致。

**存储系列按挂载路径命名，不按设备身份。** `disk_used_bytes`/`disk_total_bytes` 的 `dimension` 是 Agent 报来的挂载路径，其余系列为空串，因此 `/data` 移到另一个文件系统时开始的是另一条系列，而不是同一条系列的延续——Story 52 明确拒绝声称可靠的物理设备身份。dimension 同时进入序列主键与容量账本：`host_metric_samples` 的主键是 `(agent_id, metric, dimension, observed_at)`，`capacity_skipped_series` 的主键是 `(interval_id, scope_kind, scope_key, metric, dimension)`（0068 migration）。不进主键，同一份 Report 里两个挂载点会在同一时刻互相覆盖；不进账本，两个挂载点被跳过的样本会合并成一个说不清来源的计数。挂载路径是逐字比较的身份：Server 原样保存 Agent 报来的路径（crates/platpulse-core/src/envelope.rs:496 只校验长度，不裁剪、不归一化），查询时也逐字比较，因此只差首尾空白的两个路径是两条不同系列（review S2/P1）。生产面板把 Operator 输入的内容原样送出，不做 `trim()`：`/mnt/win ` 与 `/mnt/win` 在 POSIX 上是两个不同的目录，裁剪会把两个真实挂载点错并成一条系列，并让另一个挂载点的证据冒充这条系列。这条边界的代价如实记下：带首尾空白的路径只能在带上空白时读到，面板不为它猜一个更接近的名字；而一条存储系列在挂载路径给出之前根本不发请求（空 dimension 不是一个可回答的问题，面板不发出去再用空答案遮住）。

**存下来的是采集到的量，不是就地烤好的百分比。** Host 系列保存 Agent 采到的字节、百分比、load 与每秒字节，`memory_total_bytes` 与 `disk_total_bytes` 各自成系列，因此任一时刻的比例都还能从同一时刻的证据重新导出（0068 migration 的注释），而不是把某个时刻的除法结果固化成历史。这一票没有引入 Swap，也没有扩展磁盘 IO 系列。

**Host 的迁移与保留复用既有家族。** 0068 migration 重建 `host_metric_samples`（旧行以空 `dimension` 迁入，旧表随后删除），新建 `host_metric_series_state`（与 `node_metric_series_state` 同形，按序列记账并以 `MIN/MAX/COUNT` 从现有样本回填）与 `host_metric_aggregates`（同一对 60/300 秒 tier，主键 `(agent_id, metric, dimension, grain_seconds, bucket_start)`），`SERVER_SCHEMA_VERSION` 升到 68 并在 crates/platpulse-server/src/database.rs:92 的必需表清单里登记两张新表。保留侧不新增 family：既有 `raw_metric_sample` family 增加 `host_metric_samples` 目标与 `HOST_METRIC_CLEANUP_BATCH = 512`，两个聚合 family 的清理同时覆盖 Host 表（crates/platpulse-server/src/retention.rs:210、:218、:246）。与 Node 一样，两个序列账本刻意都不是 cleanup target：样本被释放之后，「这个序列观察过什么」仍然可知。

**Purge 的边界：Node 的清除不碰共享证据。** 一个 Node 的 Purge 只删 `node_metric_samples`/`node_metric_series_state`/`node_metric_aggregates`（crates/platpulse-server/src/node_purge.rs:345-347）与该 Node 的 node scope 容量账本行（crates/platpulse-server/src/node_purge.rs:259 `NODE_SCOPED_ROWS`、:355-359），Host 系列与容量区间属于该 Agent 的共享证据而保留；Host 侧的表只随 Agent 级联消失（`ON DELETE CASCADE`，0068 migration）。

**WebUI 表面是生产路由，不是示意页（细节见 docs/design/webui.md 第 15.17 与第 15.19 节）。** Node 页现在有三个历史面板：原来的 `Metric history`（Node Process，`data-surface="node-process"`）、新增的 `Sync and consensus history`（`data-surface="node-state"`，`data-slot="state-history-panel"`，读的是 §11.7 的状态变更日志：一条记录一行，Server 没记到的状态显示为缺失而不是某个值）与 `Host metric history`（`data-surface="node-host"`，存储系列要先给出挂载路径，并点明这是哪个 Agent 的机器）；Agent 页新增第 06 节 `Host resource history`（`data-surface="agent-host"`），其后的 Diagnostics/Audit/Danger zone 编号顺延。两个 metric 面板共用同一个 `MetricHistoryBody`（platpulse-web/src/pages/metricHistoryPanel.tsx），只有标题、主体名、引言与定义表按 scope 不同；`Sync and consensus history` 不是这条曲线，它由自己的 `StateHistoryBody`（platpulse-web/src/pages/stateHistoryPanel.tsx）渲染 Server 记录的状态本身，不把高度推断成状态。

观测接口是 Owner-only `GET /api/admin/v1/nodes/{node_id}/metric-history`，参数 `metric`/`from`/`to`/`before`/`limit`（默认窗口 24 小时，limit 默认 5000、上限 20000；`before` 是排他的翻页游标，必须严格落在 `(from, to]` 之内，否则以 `invalid_history_range` 拒绝）。响应包含每个点自己的时间证据（observed/received/delay/clock）与它由哪个 tier 交出（每点带 `grain`/`source`/`min_value`/`max_value`/`sample_count`/`first_observed_at`/`last_observed_at`/`max_gap_seconds`：`observed_at` 是坐标——raw 的观测时刻或桶的对齐起点——而 `first_observed_at` 是这个点真正持有的最早观测，界面报「这个点覆盖多久」必须用首末两个观测时刻而不是桶的对齐起点，`max_gap_seconds` 让界面在服务端已证成的空洞处断线）、推导出的缺口、序列账本（含 `window_seconds` 与本次实际携带的 `sampled_count`）、`coverage_seconds`、`window_seconds`、`truncated`、`requested_from`、`raw_retention_days`（只表达存储策略，不决定粒度）、`history_horizon_days`（30）与 `aggregate_supported = true`，以及按区间边界的 `segments`（每个 segment 给出 `from`/`to`/`grain`/`source`/`point_count`/`truncated`）与排他的 `continuation`。答案顶层的 `grain` 由返回的点推导（有点是 raw 即 `raw`，否则 1m，否则 5m，都没有则 `none`），而不是由被查询的 segment 决定，因此一个被问到但没有任何证据的 tier 不会把整个答案标成它自己的粒度。raw 与聚合的分界是契约而不是配置（review F3）：处理函数用 `crate::metric_history::raw_window_cutoff`（crates/platpulse-server/src/metric_history.rs:136，即 `now - 24h`）算 cutoff，与 `raw_metric_sample` 配成 1 天还是 7 天无关，因此把 raw 策略放宽到 7 天不会让三天前的区间改由桶回答，收紧到下限也不会让昨天的区间改由 raw 回答；`raw_retention_days` 继续如实报告存储策略本身。`from`/`to` 必须是 canonical（crates/platpulse-server/src/metric_history.rs:937 `canonical_instant`）：由于已存时刻按文本比较，任何「同一时刻的其它写法」（小数秒、`+08:00` 偏移、其它精度）都会静默排除行，因此一律以 `invalid_history_range` 拒绝，连 query 本身无法解析时也回答 `invalid_query`；成功响应包在 `no_store` 里，Owner-only 的历史永不被缓存；每样本的 delay 与时钟可疑取本次答案最新一行的 `(observed_at, received_at)`，而不是账本的 `last_received_at`（后者可能属于同一时刻更晚的一次重述）。可用性如实表达，判据是 30 天的 investigation horizon 而不是 raw 窗口：请求起点仍在 horizon 之内为 `null`；起点早于 horizon 为 `partial` 并保留原始 `requested_from`，回答只覆盖 `[max(from, horizon), to]`；终点早于或等于 horizon 为 `unavailable`——任何情况下都不编造样本，账本照常回答。整个请求窗口都早于 horizon 时还有一个必须堵住的出口（review round two 的 F3）：处理函数把请求下界 clamp 到 horizon，因此一个「终点在 horizon 之前一点点」的请求会让 `from > to` 地进入读取侧，而 tier 的下界是向下对齐到桶边界的，横跨 horizon 的那个桶于是被整桶答出来——答案标着 `unavailable`，却带着一个坐标在 `to` 之前、证据落在 `to` 之后的点。读取侧因此先判「下界高于自己的上界」，这种窗口直接空答、不咨询任何 tier（crates/platpulse-server/src/metric_history.rs:1384，位于任何 tier 边界对齐之前）：`items`/`segments`/`gaps` 为空、`coverage_seconds` 为 0、没有 `continuation`，而序列账本照常返回——一个序列观察过什么是关于这个序列的事实，不是关于这一段窗口的事实。超过 limit 时返回的是最新点并置 `truncated = true`，同时给出排他的 `continuation`，因此最旧一端不会被误读成缺口，翻页也不重复、不跳过；从未上报的序列以 `observed = false` 与零/空字段表达，Admin 面显示「从未上报」而不是 0。一条已披露的边界：`capacity_skipped_series` 没有 retention target，`capacity_protection_intervals` 也从不删除，因此暂停账本按「每个区间每个序列一行」永久增长；它极小，为它设置生命周期属于审计证据的独立决定。

**按挂载路径调查存储系列：清单本身就是证据（issue #216）。** 一条存储系列的名字只有 Agent 报来的挂载路径一个（Story 52），因此在 Operator 能读到它之前，必须先知道这个 Agent 报过哪些路径；#216 把这件事做成一次 Owner-only 的读取：`GET /api/admin/v1/agents/{agent_id}/storage-mounts`（路由声明 crates/platpulse-server/src/http/admin.rs:4800，处理函数 crates/platpulse-server/src/http/admin.rs:4815，注册 crates/platpulse-server/src/http/admin.rs:7409），回答 `AdminAgentStorageMountsResponse`（crates/platpulse-server/src/http/admin.rs:4607）——`agentId`、`answeredAt`、`cadenceSeconds`、`silenceThresholdSeconds`、`usedMetric`、`capacityMetric`、`mountLimit`、`truncated`、`collectionPaused`，以及每条路径一个 `AdminStorageMount`（crates/platpulse-server/src/http/admin.rs:4576）：`mountPath`、`observationState`、`silentSeconds` 与 `used`/`capacity` 两份 `AdminStorageSeries` 证据（crates/platpulse-server/src/http/admin.rs:4544）。清单的主表是序列账本而不是样本表：`HOST_COVERAGE_SQL`（crates/platpulse-server/src/metric_history.rs:2286）以 `host_metric_series_state` 为主表左连 `host_metric_samples`，所以「这条路径被观察过」在样本被 release 之后仍然可读；账本里没有行的路径以 `observed = false` 与全空字段表达（crates/platpulse-server/src/http/admin.rs:4953 `storage_series`），界面因此能把「有过这条路径但读数已不在保留窗口内」与「从未有过这条路径」分开，两者都不是 0。清单属于 Agent 而不是它的任何一个 Node，行数不随 Node 数增长（Story 51）；路径逐字比较、从不归一化（crates/platpulse-server/src/http/admin.rs:4576 的字段注释），因此同一文件系统的两种拼写是两条系列、同一路径背后换了设备也永不宣称可辨认，本票不新增任何稳定 device ID——这正是本票的主要风险（把 path 当稳定 device identity）被明确拒绝的地方。

**三态说的是「还在不在报」，不是「有没有值」。** `observationState` 取 `reported`、`silent` 或 `unknown`，由 `(cadence_seconds, silent_seconds)` 一次判出：节奏为 0 或静默秒数缺失即为 `unknown`，静默秒数大于阈值才是 `silent`，其余为 `reported`（crates/platpulse-server/src/http/admin.rs:4815 的处理函数）。`cadenceSeconds == 0` 的语义是「节奏测不出」而不是零节奏，而且测的是**现在**的节奏：`current_cadence_seconds` 只采信最新的间隔，且仅当上一间隔在 ×2 以内与它一致时才采信，否则答 0（crates/platpulse-server/src/metric_history.rs 的 `current_cadence_seconds`/`load_observed_cadence`），因此把 `collection_interval` 从 5 秒改成 300 秒之后，旧的高速间隔不会被继续当成今天的节奏，仍在 300 秒上报的路径不会被误判成静默，此时 `silenceThresholdSeconds` 也是 0、`silentSeconds` 根本不给出（crates/platpulse-server/src/http/admin.rs:4576 的字段注释），慢采样因此永远不会被伪装成静默，年龄仍通过每条系列自己的 `latestObservedAt`/`latestDelaySeconds` 可见。阈值与本节前面的缺口判据同源：`gap_threshold_seconds(cadence)` 即 `max(3 × cadence, 120s)`，cadence 先 clamp 到 `MAX_OBSERVED_CADENCE_SECONDS = 300`（crates/platpulse-server/src/metric_history.rs:734-737、:358），因此阈值上限为 900 秒。Story 59 要求的四条边界在这份清单上各有出处：pre-enablement 的路径以 `observed = false` 出现；低空空间保护生效期间被跳过的样本计入 `capacity_skipped_series`（dimension 就是挂载路径），而已经观测到的路径及其读数既不被删除也不被算成静默——处理函数读 `CapacityProtection::status().protected`，暂停期间对每条路径一律答 `unknown`、不给 `silentSeconds`，并把 `collectionPaused` 置真，让界面说出「是 Server 按住了采集」而不是把仍在照常上报的路径叫成静默；Purge 一个 Node 不删这份清单，因为它与其余 Host 证据一样是 Agent 级共享系列（与本节前面 Purge 的边界同一条，crates/platpulse-server/src/node_purge.rs:345-347、:259、:355-359）；Retention 释放掉最新读数时该系列答 `unknown` 并给出 `releasedBefore`，与「从未观测」区分。

**覆盖上限、排序与索引：截断是答案的一部分。** 每次读取最多答 `MOUNT_COVERAGE_LIMIT = 2 × MAX_HOST_MOUNTS = 256` 条路径（crates/platpulse-server/src/metric_history.rs:2221、:83），恰好容纳一份 Report 的挂载契约：路径已经整体换过一遍的 Host 答最新的一批并置 `truncated`，而不是把最旧的静默丢掉（crates/platpulse-server/src/http/admin.rs:4607 的字段注释）；`load_host_metric_coverage` 多读一行来判断是否截断（crates/platpulse-server/src/metric_history.rs:2293）。读取按 `last_observed_at DESC, dimension ASC` 排序并在 SQL 里完成（`HOST_COVERAGE_SQL`，crates/platpulse-server/src/metric_history.rs:2286），由迁移 0069 的索引 `host_metric_series_state_mount_idx (agent_id, metric, last_observed_at DESC, dimension ASC)` 支撑（crates/platpulse-server/migrations/0069_host_mount_coverage.sql），`SERVER_SCHEMA_VERSION` 因此升到 69（crates/platpulse-server/src/database.rs:24）；两次覆盖读取与节奏读取发生在同一个事务里（`load_mount_coverage`，crates/platpulse-server/src/metric_history.rs），所以两侧看到的是账本的同一个快照；处理函数再把同一条路径的两侧按路径合并（`merge_mount_coverage`），按同一顺序（newest desc + path asc）重排后在**合并之后**截到 `MOUNT_COVERAGE_LIMIT`，因此同一条路径不会答两次、答案也不会超过它声明的上限，`truncated` 为真当且仅当任一侧读到了更多行或合并后的路径数超过上限。

**这是一次可共享的取证复用，不是新的采集面。** 挂载系列属于本节开头的共享 Host 家族：Host 数据按 Agent 采集一次并被它的所有 Node 共享（Story 51），这份清单只是把已经存下来的证据重新读一遍，没有新增采样、没有新增保留 family；所以它的容量与性能报告仍然是已采集的那一套 Host 序列（Story 46、Story 50）。

**WebUI 表面（细节见 docs/design/webui.md 第 15.18 节）。** Agent 详情页第 06 节 `Host resource history` 现在以 `Storage by mount path` 卡片开场（platpulse-web/src/pages/StorageMountsPanel.tsx:69-78），表格给出 Mount path、Observation、Used、Capacity 与一列逐行读取的按钮（platpulse-web/src/pages/StorageMountsPanel.tsx:129-192），点某一行就把下面的 Host 历史面板切到该路径当前所选的那个存储 metric（默认 `disk_used_bytes`，切到 `disk_total_bytes` 则读容量系列；platpulse-web/src/pages/AdminAgents.tsx:1618-1633）；Node 页不给清单，仍然只接受逐字键入的挂载路径（platpulse-web/src/pages/metricHistoryPanel.tsx:750-773 的注释：清单只提供已经存有证据的路径，服务端还没有存的路径仍按它的原样拼写询问）。卡片有三处文案与 Server 一致：清单取自存储下来的证据「而不是设备身份」（`rather than from a device identity`，platpulse-web/src/pages/StorageMountsPanel.tsx:25-26）、节奏测不出时不声明静默、`truncated` 时显式说明最旧的路径不在本次答案里。

### 11.7 同步与共识的证据：五个数值系列与一份状态变更日志（issue #217）

同一个 Report 里关于同步与共识的证据有两类，不能被同一个引擎处理。区块高度是量：`sync_current_block`、`sync_highest_block`、`consensus_highest_qc_block`、`consensus_highest_lock_block`、`consensus_highest_commit_block` 可以取极值、可以按 tier 聚合，因此它们属于 §11.6 的指标历史引擎。而「这次采集成功了没有」「屏幕上那个值还是不是上次的好值」「Agent 有没有说自己在同步」是关于一次观测的事实，不是量：一段 Error 或 Unknown 一旦被压进一个桶就被抹掉，而 Server 只是漏收一份 Report 时更会被读成一次状态变化。issue #217（父票 #202 的 Story 53）因此把两者分开：高度走既有引擎，状态走一份只追加的状态日志，各有自己的账本与保留 family。两者都不从 Block history 或 Incident 反推：一个 Node 的 key height 只有它自己的 Report 说过才算，状态条目也只因一份 Report 真的携带了 sync 或 consensus 组件而存在（crates/platpulse-server/migrations/0070_sync_consensus_history.sql 的头部注释）。

**五个新系列骑在既有 raw/聚合 tiers 上。** `NODE_METRIC_SERIES` 由 5 条扩到 10 条（crates/platpulse-server/src/metric_history.rs:98）：新增的五条从 crates/platpulse-server/src/metric_history.rs:104 起，`is_node_metric`（crates/platpulse-server/src/metric_history.rs:112）一并接受它们，写入与其余 Node 指标走同一条路径（crates/platpulse-server/src/http/report_ingestion.rs:1533 起）。因此 24 小时 raw 窗口、1m/5m 两个聚合层、`windows_abut` 与 `max_gap_seconds` 的缺口判据、`classify_delivery` 的重放与修正账本、`raw_metric_sample` family 的清理全部照旧适用（这些 tier 与缺口判据的完整说明见 §11.6），没有新 family、没有新 tier。五条分开而不是合成一条 height：同一个 epoch 的 highest QC block 与 commit block 是两个不同的事实，一个前移而另一个停在原处必须看得见。一次失败的链上探测不写任何样本，失败本身由状态日志记下（crates/platpulse-server/src/metric_history.rs:98 起的注释），因此数值侧永远不会用 0 或插值掩盖一次没采到的读数。高度是整数而样本是实数：任何真实链可达的高度都远低于 2^53，样本因此就是那个高度本身、历史保留的是测量时的精度（crates/platpulse-server/src/http/report_ingestion.rs:1526 起）；超过这个界的整数会被实数取整，规范 fixture 里那个 2^53+1 的高度证明的是 JSON 这一层不丢精度，不是样本也存得下它。

**为什么重建 raw 表而聚合表不动。** `node_metric_samples` 的 metric 列是 CHECK 白名单（0041 migration 建表时即如此，0070 重建为十项，见 crates/platpulse-server/migrations/0070_sync_consensus_history.sql:44），新增序列因此必须重建这张表：0070 把旧表改名为 `node_metric_samples_old`、重建、把已有样本全量搬过来、再删旧表（crates/platpulse-server/migrations/0070_sync_consensus_history.sql:40、:62-66），两个索引也一并重建。已有样本是真实证据，迁移只搬不改。`node_metric_aggregates` 与 `node_metric_series_state` 的 metric 列只校验长度、不认识任何具体序列名（crates/platpulse-server/migrations/0067_node_metric_aggregates.sql:45、crates/platpulse-server/migrations/0066_node_metric_history.sql:44），五条新系列不加迁移就能落进两个聚合层，账本也继续按 `(node_id, metric)` 从原来计数；0070 的头部注释把这一点写成「reuses node_metric_aggregates and node_metric_series_state unchanged」。

**状态不是量：`node_state_observations` 与 `node_state_series_state`。** 状态向量是 `(collection_state, value_source, error_code, syncing)`（crates/platpulse-server/src/state_history.rs:133 `StateVector`，由 crates/platpulse-server/src/state_history.rs:146 `StateObservation` 从组件观测导出），其中任一字段变化就是一次状态变化，这也是失败与成功不会被合并成一行、更不会被拿去平均的原因。日志只追加：每个被承认的状态一行不可变记录，外加状态不变时的周期性锚点（crates/platpulse-server/migrations/0070_sync_consensus_history.sql:117 建表，主键 `(node_id, component, observed_at)`，`component` 只允许 `sync` 与 `consensus`，即 crates/platpulse-server/src/state_history.rs:54 `STATE_COMPONENTS`）。同名账本 `node_state_series_state`（crates/platpulse-server/migrations/0070_sync_consensus_history.sql:159，与 `node_metric_series_state` 同形）除「最近一次状态」之外不保存任何状态值，因此它是账本而不是第二份历史：日志行被保留策略释放之后，「这条序列最后一次说的是什么」仍然可读。`observed_at` 是 Agent 自报的尝试时刻，组件从未尝试过（disabled、unsupported）时退化为这份 Report 的生成时刻（crates/platpulse-server/src/state_history.rs:124 `component_instant`），因此每一行都是一个 Node 真的被听到过的时刻。

**`value_source` 在写入时判定，`syncing` 只在采集成功时存。** `StateObservation::of_component` 按组件自己的证据判 `current`/`last_good`/`none`：这份 Report 带了值且状态为 Ok 即 `current`；带了值但状态不是 Ok 则 `last_good`（`value_observed_at` 就是那个值的观测时刻，因此 last-good 的年龄从证据读出，不是重新算出来的）；这份 Report 没带值而 Server 的 `component_status` 里仍留着值也是 `last_good`（crates/platpulse-server/src/state_history.rs:221 `retained_value_at`，在本次投递写入之后读，读到的正是 Owner 看得见的那一行）；两处都没有才是 `none`。`error_code` 同样是 Report 说的那个错误码，不由 Server 编造。`syncing` 只在 `collection_state = 'ok'` 时入行（crates/platpulse-server/src/state_history.rs:146 的判定与 0070 的列注释）：一次失败的重试根本没有观测到这个标志，在那里写 0 等于替一个同步状态未知的 Node 声称「不在同步」，而这正是 §5.1 的「Unknown 不是 false」。写入顺序也是这个道理（crates/platpulse-server/src/http/report_ingestion.rs:1279-1325）：先写组件投影，再回读它还持有哪个值，然后才写状态日志。

**锚点让「没变化」也可证；没到达的 Report 什么都不写。** 状态不变时日志按小时留一个锚点（crates/platpulse-server/src/state_history.rs:91 `STATE_ANCHOR_SECONDS = 3600`；crates/platpulse-server/src/state_history.rs:442 `anchor_due` 在它前面那一行（没有行时用账本的 `last_entry_at`）已经过去一小时后判真，没有可用的时刻时立刻判真），因此相邻两行之间的一段是这条序列真的证明过的时段，而不是「大概一直如此」；没有锚点，读取侧只能说「很久以前某一刻它是 X」，然后必须在「之后一直没变」与「之后不知道」之间猜。反过来说，一次从未到达的 Report 不写任何东西：日志只由投递推进，于是停报留下的是一个洞，读取侧必须把它报成「这段没有被证明」，而不是凭空造一次状态变化（0070 的头部注释：「a Node that stopped reporting writes nothing at all」）。`entry_kind` 因此只有 `change` 与 `anchor` 两种（crates/platpulse-server/src/state_history.rs:413 `entry_kind`）。

**投递分类与账本计数。** 写日志前在同一 ingestion 事务里读账本与该时刻已存的行，按与指标样本同一套顺序判定（crates/platpulse-server/src/state_history.rs:470 `classify_state`，连同该时刻之前那一行的证据一起判定，crates/platpulse-server/src/state_history.rs:570 `fetch_predecessor`）：晚于账本 `last_delivery_at`（crates/platpulse-server/src/state_history.rs:809：这条序列已经记账过的最近一次投递时刻）的观测一律是 `Observed`；在该时刻已有行时，向量相同是 `Replay`、不同是 `Correction`；在该时刻没有行时看它前面最近的一行：向量不同就是这次迟到投递所证明的那次转变，记为 `Observed` 并写一行 `change`，向量相同才是无害的 `Replay`；而一个落在已记账投递时刻上、却与当前持有的那一行不符的投递按 `Correction` 处理（只改写该时刻那一行，不重复计入一次转变）；其余是 `Replay`（一次已释放的重放与「旧到无法保留的观测」无法区分，一并按重放计，宁可少计也不虚高）。账本逐项记账 `entry_count`、`change_count`、`anchor_count`、`replayed_count`、`corrected_count`（crates/platpulse-server/migrations/0070_sync_consensus_history.sql:159-194 的列与 CHECK，`last_delivery_at` 见 :177-184），其中 `entry_count` 统计的是被承认的投递，包括状态没变、没有写行的那些，因此 crates/platpulse-server/src/state_history.rs:842 `StateLedger::delivery_cadence_seconds` 得到的是这条序列的上报节奏而不是日志行的锚点节奏，取不到时答 0（未知，不是「没有间隔」）。一次修正只改写该时刻那一行，并把这一行整套换成这次投递的说法：`ON CONFLICT` 的每个状态列都取 `excluded` 的值——`collection_state`、`value_source`、`value_observed_at`、`error_code`、`syncing` 一律跟随这次投递（crates/platpulse-server/src/state_history.rs:612 `store_entry`，由 crates/platpulse-server/src/state_history.rs:262 `record_state` 在同一事务里调用），因此行里不会残留上一次的说法与这一次混成的第三种状态；证据并不因此消失，只是这一时刻的证词以最新一次投递为准，而这次投递本身仍在账本的 `corrected_count` 里留下痕迹。 账本计数也止于日志能指认的范围：只有日志持有行的时刻与已记账的最新时刻能被保证只计一次，更旧而没有落行的时刻无法与从未见过的投递区分，一次与它前面那一行不符的重复在那里会作为新证据计数（与它前面那一行相同的重复仍是无害的 `Replay`）。

**迟到投递把已知的最新状态落到行上。** 一条迟到（时刻早于账本已记账的最新时刻）、且与它前面那一行不同的投递，证明的是日志原本没有的一次转变，因此会写一行 `change`；但这会让「这条序列最后一次说的是什么」只留在账本里而没有对应的行，读取侧于是把迟到的那个旧状态当成最新状态。写入因此在同一次调用里把账本已知的最新状态补成一行：`observed_at` 取账本最新已记账时刻，内容取账本保存的最新向量，`entry_kind` 按该时刻之前那一行的证据判定（crates/platpulse-server/src/state_history.rs:1022 `repair_late_insert`）。这一行不是一次投递——它是账本早已承认过的证据，所以不动任何计数，正如一次 `Correction` 改写的那一行不动计数；若改为等未来某次投递来「接上」这个返回状态，就会把它记到错误的时刻，而且可能永远等不到。

**状态侧的沉默阈值跟着这条序列的实测节奏。** 状态行不像指标样本那样每隔一段固定时间就落一条：状态不变时只有锚点落行，所以相邻两行的距离天然可以到「锚点间隔 + 一次真实投递节奏」。因此状态点在连续性判定里带一个锚点间隔的窗口、并按这条序列实测的上报节奏放宽（crates/platpulse-server/src/state_history.rs:976），引擎再把节奏（上限 300 秒）加到窗口上（crates/platpulse-server/src/metric_history.rs:877-880），沉默阈值即 `max(gap_threshold, 锚点窗口 + 实测节奏 + clamp(实测节奏, 1, 300))`。没有这条放宽，一条一直在上报、状态却一小时才落一行的序列会被自己的正常节奏判成 `collection_gap`；真正超出这个距离的沉默仍然报成缺口，短于这个距离的沉默无法从日志证明，因此不做任何声称。

**低空间保护暂停的是状态日志本身，记账仍在既有账本上。** 保护生效期间状态日志不追加，但每次被跳过的投递都记进 §11.5 已有的 `capacity_skipped_series`：`metric` 就是组件名（`sync`/`consensus`），`dimension` 为空串，没有为状态新增任何容量表或列（crates/platpulse-server/src/state_history.rs:262 `record_state` 在 `HistoryGate::Paused` 且投递不是 `Observed` 时调用 `capacity::record_skipped_series`）。因此暂停在读取侧是一道 `protection_pause` 缺口并带上被计入的跳过数，而不是一段「正常」或一次状态变化：Story 59 要的正是「一次暂停必须作为披露出来的边界出现」。与指标样本一样，记账与写入同进同出，因此不存在「样本没落库而缺口也没记账」的状态。

**保留家族 `observation_state`。** 状态日志有自己的 family（crates/platpulse-server/src/retention.rs:56 `FAMILY_OBSERVATION_STATE = "observation_state"`，label `Synchronization State History`，策略类别 investigation，30/30/30——min 等于 max，因此普通编辑既不能缩短读者仍然需要的状态，也不能承诺设计没有的时段；crates/platpulse-server/src/retention.rs:610-622）。0070 因此又把 `retention_policies` 的 family CHECK 从 15 项重建为 16 项（crates/platpulse-server/migrations/0070_sync_consensus_history.sql:199-244），并只搬运 Operator 已选的值：CHECK 不认识的值在 seed 时会被静默跳过，于是这个 family 既不出现在 Owner 面、其清理也永不运行，状态日志会无界增长——这正是 30 天调查下限本身要防的结果。清理批次 `STATE_CLEANUP_BATCH = 2048`（crates/platpulse-server/src/retention.rs:343）带编译期断言：它必须覆盖一份最大 Report 的 `MAX_NODE_OBSERVATIONS × STATE_COMPONENTS`（crates/platpulse-server/src/retention.rs:346），即其余 raw 家族同样的余量。`cleanup_expired_observation_state`（crates/platpulse-server/src/retention.rs:928）先按同一个 cutoff 给每条序列盖 `released_before`（crates/platpulse-server/src/retention.rs:358 `STATE_RELEASED_BEFORE_SQL`，与旧值比较且只往前走），再按有界批量删除过期行（crates/platpulse-server/src/retention.rs:367 `TARGET_OBSERVATION_STATE`，固定 SQL、恰好一个绑定参数）。盖章的理由与 raw 家族相同：删掉一行就毁掉了 Server 判断「这个时刻是否已经记过」的依据，`released_before` 因此让一次策略放宽之后重放的投递仍被认成重放、永远不会被计第二次（issue #213）。账本刻意不是 cleanup target：行的证据可以释放，「这条序列最后一次说的是什么」必须留下来。窗口小于等于 0（保留永久）时没有 cutoff，直接答 0 行，不可能过期任何东西。

**Purge 的所有权。** 同步与共识状态描述的是某个 Node 自己对链的看法，不是共享证据：Node 的 Purge 计数并删除 `node_state_observations` 与 `node_state_series_state`（crates/platpulse-server/src/node_purge.rs:239-240 计数、:366-367 删除），该 Node 的 node scope `capacity_skipped_series` 行同样删除，Host 系列与容量区间仍属于 Agent 而保留（§11.6）。两张新表同时登记在必需表清单里（crates/platpulse-server/src/database.rs:94-95），因此一个缺表的数据库不会被当成可用的。

**读取面：`GET /api/admin/v1/nodes/{node_id}/state-history`。** Owner-only，参数 `component`（`sync` 或 `consensus`）、`from`/`to`/`before`/`limit`（crates/platpulse-server/src/http/admin.rs:4740 `AdminStateHistoryQuery`、:4917 处理函数、路由注册见 crates/platpulse-server/src/http/admin.rs:7840）。组件不在记录之列时回答 400 `invalid_component`，与未知 metric 同样的错误体，而不是给一个看起来像「从没人报过」的空序列；query 本身不可解析与区间非法分别回答 `invalid_query` 与 `invalid_history_range`，`before` 必须严格落在 `(from, to]` 之内（一个落在请求区间之外的游标只会答一段没人问过的时段）。状态没有聚合层，因此 family 的保留窗口就是它的全部地平线：起点仍在地平线之内时 `availability` 为 `null`，起点早于地平线为 `partial` 并保留原始 `requested_from`，终点早于或等于地平线为 `unavailable`；默认窗口 24 小时、`limit` 默认 2000、上限 20000（crates/platpulse-server/src/state_history.rs:94、:97）。条目按时间正序回答（最旧在前），因为连续性判定就是先判最旧的一对（crates/platpulse-server/src/state_history.rs:868 `load_range`，先按 `observed_at DESC` 多读一行判断截断，再反转成时间序）。`coverage_seconds` 只累加两条已记录状态之间的时段，`gaps` 给出 `collection_gap`（没人观察）与 `protection_pause`（保护区间覆盖它）两种缺口及各自的原因，`skipped_count` 只在 `protection_pause` 且所报时段完整覆盖整个暂停区间时附带；`series.released_before` 让「早期行已被释放」与「这条序列一开始就很安静」分得开；`truncated` 为真时 `continuation` 给出最旧被返回的坐标，作为排他游标取下一页，既不重复也不跳过。窗口最新一条之后的沉默不算缺口：状态不是一直伸到下一次上报为止的常量，因此答案绝不为它补一个洞，也绝不补一个状态。成功响应包在 `no_store` 里——Owner-only 的 Node 历史是逐会话的数据，不经过中间层或浏览器缓存（§12.4）。状态条目永远是这个 Node 自己的历史，不是上报它的那台 Agent 的 Agent/Host 历史。

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
- 历史窗口与 Site Access Mode 变更记录 old/new 值与操作者，边界外的值被拒绝；
- Node 指标历史的两个聚合层（grain 60s/300s）在同一 ingestion 事务内累加，重放不重复累加、修正不抬高计数；raw/1m/5m 的可回答区间在任意时刻都不重叠（tier 分界对齐到桶边界，跨界的那一个桶由更粗的 tier 整桶回答），桶只在自己证成的跨度上声明覆盖（`max_gap_seconds` 暴露桶内空洞，`coverage_seconds` 既不跨缺口也不假设缺口里的值；两个相邻桶之间缺了整整一个桶窗口时一律是缺口，绝不因节奏宽限而被桥接，而 tier 手里没有那个跨界桶时 raw 保留自己跨界的那一分钟，升级缝隙里已存的 24 小时证据因此不会被丢掉），请求区间超出 30 天 investigation horizon 的部分如实报 `partial`/`unavailable`，从不编造样本；
- Host 共享系列（issue #215）只有一条写入路径：一份 Report 的每个 Host 量只落一次、属于采集它的 Agent，任何读法（Agent 路由或 Node 路由）都读同一份并点名 `scope_kind`/`scope_key`；`node` 与 `host` 两个 kind 互相拒绝对方的 metric（400 `invalid_metric`），一个从未被观察到的 `dimension` 以 `observed = false` 如实上报而不是 404 或空报错；一个 Node 的 Purge 不删它所属 Agent 的 Host 系列与共享容量区间（crates/platpulse-server/src/node_purge.rs:355-359）。

### 13.4 WebUI

- Home 从 Network 列表进入 Node Detail；站点 Private 时 Home 要求 Owner/Viewer 登录，Public 时允许匿名 Guest 读取；
- Home 概览为六项当前选择统计 + 透明 Peer 国家地图，保留 1280px 页面上限与现有 Node grid 列规则；Network 筛选同步作用于四项原统计、累计出块/奖励数值概览、Node 列表与地图。桌面 >=1024px 统计/地图约 5:6（地图占较大一侧，统计保持原四卡时的紧凑高度），统计三列两行；地图 >=1280px 用上游 22rem 固定带，低于该宽度保持 2:1 比例轨道（两层填满轨道盒，世界不被裁切）；低于该宽度统计在地图上方，>=640px 三列、手机两列；逐 Network 精确值、coverage/stale、未关联 Node 通过累计卡片的 Breakdown 访问；
- Home 的 Node 列表过滤在 Web 端完成：搜索匹配 Node 显示名、Node ID、Network 显示名三个公开字段，并与 Health、Validator status（直接用 Server 的 currentValidatorStatus 原值，缺 Link 为 unknown 而非 not_validator）按 AND 组合；只有 Network 选择改变六张概览卡与 Peer 国家地图，列表同时显示匹配数与作用域内总数并给出该口径说明，空态区分「选择内无 Active Node」与「无匹配」且不编造 0；network/q/health/validator/sort/view 由普通 Home URL 承载、刷新与直接导航即恢复，本部署无法识别的取值就地回退并显示通知而不改写链接（载入不改写、仅读者操作写回）；Home 提供卡片与列表两种视图（`view`，默认 card），列表含七列且窄屏只在列表内部横向滚动、Node 名称与 Health 固定可读，六种排序中数值键把从未观测到的值排末尾、真实 0 保持为 0、同值以 Node 身份稳定 tie-break；该 URL 不是分享凭据也不授予权限，未知/陈旧绝不渲染为 0/false/Healthy；
- Node Detail 按 §9.1 和 WebUI 统一契约验收：Validator 与非 Validator 共用 Header、六 KPI＋地图、Chain state／Process／Host resources 和相同六图；只条件增加紧凑 Validator performance，无四 KPI 分支、Details／Network tabs 或空白占位。身份、Collection 与 Head lag 遵循已批准的数据证据边界；角色切换不改变地图／KPI 尺寸或顺序，缺失不伪造零／Healthy；图表动态轴、tooltip、ID copy、diagnostics 与移动端均保持。最近两个连续 Block Summary 仅用于区块间隔，无 Bounded Block History 或历史导出；
- Admin 的 Node 页面不复制 Home 的完整 Node Detail；
- SSE 断开显示 `Live updates paused`；invalidation 后通过 REST 重取；
- 360px 无水平溢出；Unknown、Stale、Error 不被渲染成 Healthy；
- Admin Node 页的 `Metric history` 面板提供 1 小时/6 小时/24 小时/7 天/30 天预设，把聚合桶画成方块并在图例里与圆点（一条已存观测）区分，列出 `Tiers in this answer`（含被查询但没有证据的 tier）与 `Investigation horizon`，并支持 `Load older points` 与 `Return to the newest points`；
- Admin Node 页同时有三个历史面板：`Metric history`（Node Process，`data-surface="node-process"`）、`Sync and consensus history`（`data-surface="node-state"`，`data-slot="state-history-panel"`，读 §11.7 记录的同步与共识状态以及每一次静默）与 `Host metric history`（共享 Host 系列，`data-surface="node-host"`：存储系列先要求一个挂载路径，并点明该系列属于哪个 Agent）；Admin Agent 页第 06 节 `Host resource history`（`data-surface="agent-host"`）读同一份共享系列，其后的 Diagnostics/Audit/Danger zone 顺延为 07/08。两个 metric 面板共用同一个 `MetricHistoryBody`，展示定义分别取自 `NODE_METRIC_SERIES` 与 `HOST_METRIC_SERIES`；`Sync and consensus history` 由 `StateHistoryBody`（platpulse-web/src/pages/stateHistoryPanel.tsx）渲染 Server 记录的状态日志本身。

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

本节来自已完成的 `/grill-with-docs` 访谈（Q1–Q22）及 Owner 的最终共识确认，是目标设计。后续实现规格按仓库约定进入 GitHub Issues，本节不代替具体接口设计或实现工单。实现状态：§15.2 第 1、2 项（Agent 接入引导、显示名称/备注）已由 issue #169 交付；§15.3 Node Purge 已单独交付；§15.2 第 3–5 项 Agent Removal 已由 issue #171 交付（含所属 Node 权威列表、未处理 Transfer 阻止、全凭证撤销、子 Node 级联清理与删除身份边界）；§15.6 Agent Attention Acknowledgment 已由 issue #172 交付（Server-owned occurrence/evidence 边界、共享持久确认、Overview 与 Agent Detail 的逐条与批量操作、失败可重试与审计）；§15.4 自动 Validator 身份与 Current Validator Status 已由 issue #173 交付（Server 从已校验 Network 与观测到的完整 P2P 公钥自动建立 Node Validator Link、换键关闭旧区间、Public/Node 视图消费 Server 投影，不再展示手工 role）。手工注册/绑定/角色写入端点已退役并返回明确的 GONE 状态。一次性 Validator 模型迁移（§15.5）已由 issue #174 交付：Server 启动迁移在同一 SQLite 事务内识别无 automatic Link 的旧手工一代，删除其 Link、current insight、ranking/counter history、daily/monthly 聚合与 Validator 身份本身，并写入 validator_model_migration 一次性标记；automatic 一代身份与数据保留，旧分类/变化基线同步失效，Node 监控历史、既有 Incident 与必要审计保留；§15.7 删除主体的通知取消与 Incident 标注已由 issue #175 交付（被删 Node/Agent/Host 退出当前评估并标注 subject_deleted_at 而不伪造恢复，未发送通知——含无 incident_id 的恢复事件——被取消，已发送事实保留，其他主体与共享 Validator 通知策略不受影响）；§15.7 的 Incident 列表、详情与持久确认已由 issue #203 交付（Owner 仅可对一次 occurrence 做共享、不可撤回、不可覆盖的确认，记录确认者与时间且不改健康/恢复/通知策略，恢复后复发为新的未确认 occurrence，主体删除与恢复相互区分；列表支持精确 `subject_key` 过滤，Node/Agent 详情提供跳转到该 subject 的 Incident 历史快捷入口，父 issue #202 Story 2）。

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

实现状态：§15.4 的自动身份识别、当前状态投影与 Admin 呈现已由 issue #218 交付（见 WebUI §15.20）。Server 侧由 `validator.rs` 承担唯一投影（`project_activity`、`automatic_identity_reason`、`last_good_age_seconds`、`list_node_validator_identities`），Public 与 Admin 因此共用同一套判定，不会各自漂移；Admin 新增 `GET /api/admin/v1/validator-identities`、`/admin/validators`、`/admin/validators/:validatorId`，并在 Node 详情内嵌只含识别状态与该身份的公开关联是否投影的 `validator_identity`（不含所有权或角色）。Provider 未配置或不可读时一律为 `unknown`/`Not configured`/`Unknown` 并保留 stale last-good，绝不产生 `not_validator` 或新鲜的 0；缺键、Network Identity 不匹配时只给出未知状态与理由。链上密钥变化结束旧 Link 区间并按边界新开区间，而「无证据」状态（缺公钥、缺 Network Identity）不结束区间；Node Purge 只清该 Node 的识别状态与 Link：被 Purged Node 自己的区间随该 Node 删除而不可重建（读回为无关联），`linkCount` 相应减少，仅当被删 Node 是该身份唯一关联时才为 0；共享 Validator 身份、其保留证据以及其他 Node 的关联历史（含已结束区间）保留。2026-10-06 的 #202 修复将确认结论与详情指标分开持久化：`last_good_verdict_outcome`（`success`/`empty`）及 `last_good_verdict_received_at` 是当前状态/Activity 的唯一 last-good 依据，Public 与 Admin 共用 `project_verdict`。只有携带明确 Activity 的成功观测或严格校验的权威缺席更新该依据；指标-only 成功不覆盖或续期结论。`active → empty → error` 与首次 `empty → error` 均保留最新缺席并标 Stale，缺席也会在无刷新时自然陈旧；无可证明的确认时间则 Unknown，不从旧 Activity 猜测结论。`activityReceivedAt` 单独披露确认时间，`empty` 不续期旧质押/奖励/计数器的 `last_good_received_at`，也不生成指标快照或新鲜的 0。迁移 72 只为 `source = platscan` 的当前成功回填结论：已交付的 PlatScan 规范化器在每次成功中都携带明确 Activity；当前权威缺席则使用其自身 attempt receipt。其他来源的成功可能只有指标而沿用旧 Activity，旧失败记录也可能遗失中间缺席，因此这两类记录均保持 Unknown，等待真实确认。Admin 同时独立披露 ranking 的 outcome、freshness、attempt/last-good 时间和脱敏诊断，详情新鲜不能替排名背书。修复与实际回归/门禁结果见 [#202 Validator evidence repair](../research/issue-202-validator-evidence-fix.md)；locked/exiting/verifying 的权威否定条件仍需主源证据。

### 15.4.1 Validator 日/月快照趋势与配置日历边界（已由 issue #219 交付）

- 趋势以「配置日历的一日」为单位，而不是以 UTC 日为单位：`from`/`to` 用 UTC 瞬时表达，但归属到该瞬时所在的配置 IANA 本地日，`to` 含其落入的本地日，因此 N 天窗口对应 N+1 个配置本地日。日/月边界由 `local_day_bounds`（validator.rs:2243）、`local_period_at`（:2273）、`local_midnight`（:2205）计算；DST 抹掉的午夜（例如 America/Santiago 的 00:00 不存在）取该本地日最早的合法瞬时，桶仍属于真实的本地日。配置时区来自 `[validator_provider] timezone`：配置加载时若不能解析成 IANA 名即 `ConfigError::InvalidValidatorTimezone` 启动失败，读取时若仍不能解析则返回 `InvalidTimezone` 400——两处都不回退成 UTC。
- 查询有界且披露完整：窗口上限 730 天（`TREND_MAX_WINDOW_DAYS`，超出即钳制并在响应里标 `clamped`），`limit` 默认 90、上限 366（`TREND_MAX_LIMIT`）；分页游标 `before` 是排他游标（严格更旧的配置本地日），响应给出 `continuation`；截断只丢最旧的日，且 `expectedDays`/`observedDays`/`missingDays` 只统计「已应答的那一段」，应答段之外的日绝不算作静默（`coverage` 因此区分 `empty` 与 `unavailable`）。
- 每个点披露真实的取样时间与来源：`sampleTime` 为 `provider` 或 `receipt`（provider_timestamp 优先、否则 receipt，与 §15.5 的 `analytics_period` 同一规则），并同时给出三个具名瞬时（sample_at、provider_timestamp、received_at），因此 provider 优先的取样时间绝不会脱离它旁边的 receipt 单独出现；`delaySeconds` 在任一时间不可用时为 null（绝不写 0），`clockSuspect` 标记 Provider 时间早于接收时间；行里没有的指标就是 Unknown，绝不呈现为 0。
- `coverage=partial` 有两种诚实读法，由应答自身的数字决定：已应答段内确有缺行（`missingDays > 0`）是「静默之外的缺失」，而 `missingDays == 0` 只是窗口未答完（truncated/clamped），此时绝不能说已答段内有缺失。
- 累计语义写死在契约里：`counterSemantics` 固定为 `"cumulative"`，reward/block 是累计值，WebUI 也标注为累计，绝不差分出周期收益或净收益；stake 是该次观测的余额快照，不是累计计数器，因此文案不得把 stake 归入累计措辞。
- 其他时区已存在的行既不合并、也不静默按 UTC 重桶：响应把它们计为 `foreignRows`/`foreignTimezones` 并明确披露，配置日历上的那几天仍如实显示为静默。
- Story 68：Purge 删除该 Node 的 `node_validator_links`，因此趋势里的关联列表只列出仍能解析到存活 Node 的区间，并给出 `deletedNodes` 与 `associationHistoryPartial=true`，说明已删 Node 的关联是「这里不可用」而不是「从未存在」，也绝不把它们重新挂到别的存活 Node 上；共享 Validator 身份与其已存快照日保留，趋势仍可读取。
- rank 归属由同一观测周期保证（复审 F1）：`record_daily_snapshot` 返回它真正落盘的那一天（`StoredDay { local_date, month_key }`），`apply_provider_result` 以 `AppliedDetail.stored_day` 把它交给 `record_snapshot_rank`（validator.rs:2620），后者只更新那一天的行（权威缺席写 NULL），不再按 receipt 瞬时重新推算一个可能无行或错日的日期。因此 provider 时间戳落在前一个配置本地日时，rank 随那次观测写到那一天而不丢失；detail 失败（`stored_day: None`）或 ranking 失败的周期不写任何 rank，保留该日 last-good——这一行只有唯一的 sample time，rank 必须与它所属的观测同周期写入，否则会假借旧观测的时间戳。由于 `rebuild_monthly_aggregate` 没有零行保护，只有确实更新到该日行时才重建该月聚合。
- 表面：Admin 只读端点 `GET /api/admin/v1/validators/{validator_id}/trend`（Owner 守卫沿用 `http/mod.rs` 的 `/api/admin/v1` 嵌套，见 WebUI §15.21）。

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
- 本次（§15.6 Agent Attention Acknowledgment，issue #172）不增加 Node 提示确认、永久关闭某类提示或 Silence/Maintenance 页面；当时也尚未提供完整 Incident 历史页面，该页面已由后续 issue #203 交付，见 §15.7 与 [WebUI §15](webui.md#accepted-management-ui-target)。

### 15.7 恢复、确认、主体删除与通知

| 动作/事实 | 提示与 Incident | 数据/通知 |
|---|---|---|
| 当前故障恢复 | 当前提示条件消失；Incident 仅在已知恢复持续满足规则后 resolved | 恢复不自动删除 Incident；历史型提示可能仍需确认 |
| Owner 确认提示 | 同次提示退出醒目区域，持续故障的真实状态不变 | 证据与 Incident 保留；不暂停通知、不发虚假恢复 |
| Agent/Node 删除 | 主体退出当前待处理问题和后续告警评估；保留 Incident 原有事实并标注主体已删除 | 保留既有 Incident/必要审计；取消尚未发送通知，不因删除发恢复通知 |

Incident 保留证据不等于继续把它当作当前待处理故障；删除主体不把 open Incident 改成声称已知恢复的 resolved。Incident 列表、详情与持久确认页面已由 issue #203 交付（Agent 提示确认与 Incident 确认是两个独立边界，见 §15.6）。常规 Retention 保护 Incident 历史；Notification Event 有独立保留策略，不能把二者混淆。 版本安全的 Alert Rule 管理与 Network/Node 继承覆盖页面已由 issue #204 交付（[WebUI §15.7](webui.md#157-alert-rule-management-and-inherited-overrides-issue-204-delivered)）：typed catalog 只能读取/编辑既有 key，保存携带 composed `expectedVersion`（覆盖 upsert/删除同样推进该版本），陈旧保存被 Server 以 `alert_rule_version_conflict` 拒绝后必须重载当前配置并重新复核；preview 不提交、不创建/解决/确认 Incident，也不承诺立即生成 Incident；Rule 编辑或禁用绝不改写 Incident 的 opening rule version 与 opened evidence，也不清除确认，Incident 详情另行投影当前 effective 配置，无法解析时按 unknown 处理而不是认为 Rule 已禁用或主体已恢复。

通知侧的测试命令、请求级去重台账、测试冷却、Delivery 重试与 request id 对账页面已由 issue #206 交付（见 WebUI §15.9）：HTTP 超时或网络错误必须呈现为结果未知并对账原 request id，绝不换一个身份自动重发；Channel 状态与 Destination 一律掩码，bot token 不出 Server 主机。 可重试 Delivery 的显式安全重试由 issue #207 交付（见 WebUI §15.10）：只有 Server 认可的状态接受重试，终态一律拒绝；重试追加新尝试而不覆盖旧尝试与失败证据，不新建 Event 或业务迁移；重试后仍受 handoff 前的抑制与删除检查约束。
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
