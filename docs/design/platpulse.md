# PlatPulse 产品与技术设计（MVP）

## 1. 文档状态

- 状态：MVP 设计基线（目标文档）。
- 适用范围：`platpulse-core`、`platpulse-agent`、`platpulse-server`、`platpulse-web`。
- 领域术语：以仓库根目录 `CONTEXT.md` 为准；本文件只描述术语在 MVP 范围内的使用方式，不重复定义。
- 规范性用词：
  - 必须：实现和验收不可省略；
  - 不允许：违反即破坏边界或领域不变量；
  - 可以：允许实现，但不是 MVP 必需能力；
  - 后续阶段：不进入 MVP，不阻塞 MVP 发布。
- 本文件描述目标设计，不表示当前仓库代码已经符合本文。当前实现中超出目标的部分（Validator、Geo、Alert、Notification、Backup/Restore、Retention、Operation、Node Transfer、Recovery/Rotation 等）默认视为后续阶段，本次不做实现迁移；代码注释引用的旧版章节号（如 §17.x）将在实现迁移时统一对齐。

---

## 2. 产品形态

产品分离参照 Komari（<https://github.com/komari-monitor/komari>），但监控对象是 PlatON Node 而不是服务器：

- Home：只读、以 Node 为中心的监控面。Network → Node → Node Detail，展示 Node 当前状态与近期 Block History。Site Access Mode 为 Public 时所有人可读，为 Private 时需登录。
- Admin：认证后的系统概览与配置面。覆盖 Agent/Node/Network 配置、全局历史窗口、Site Access Mode、Sessions 与 Audit；不复刻 Home 的完整 Node Detail。
- 同一个 WebUI 承载 `/` 与 `/admin` 两组路由，使用不同的 DTO、查询缓存、权限和导航。
- 站点级 Site Access Mode（Public/Private）：由 Owner 配置，变更记 Audit；MVP 默认 Private。所有 Active Node 一律出现在 Home，没有按 Node 的可见性开关。

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

## 3. MVP 范围

### 3.1 必须支持

1. 一个 Server、每个 Host 一个 Agent、一个 WebUI；一个 Agent 监控本 Host 上的多个 PlatON Node。
2. 多个 Agent 可以向同一个 Server 上报。
3. 每个 Node 的观察和错误状态完全独立；一个 Node 失败不阻塞同一 Agent 的其他 Node。
4. Home 展示 Node 当前状态与近期 Block History；Admin 提供系统概览并配置 Agent/Node/Network、全局历史窗口与访问控制。
5. 采集、上报或接收失败不会静默覆盖最后成功值，也不会重复写历史。
6. Server 重启后保留 Agent、Node、当前投影和 Block History。
7. WebUI 在 360px 手机宽度、平板和桌面宽度下可用。
8. 初始部署保持 Linux-first、单租户、单 Server、SQLite。

### 3.2 后续阶段（不进入 MVP）

Validator（Provider/Analytics/Ranking）、Geo、Peer Snapshot 与 Peer Presence Interval、Typed Alert/Incident/Silence/Maintenance、Notification、Node Transfer、Agent Recovery/Credential Rotation、Backup/Restore、Retention、Doctor、Operation、完整交易 Body、Block Explorer/Archive、RPC Endpoint failover、远程控制、多租户/HA/PostgreSQL/集群、SSO/OIDC/TOTP/WebAuthn、非 Linux Agent。

这些能力保留在 `CONTEXT.md` 术语表中作为未来领域概念，但不进入 MVP 的数据流、协议、数据库表或页面。

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
- Agent Credential：由 Owner 预先配置/下发（Server 配置或 CLI 的 provisioning 动作）；MVP 没有 Enrollment、Recovery、Rotation 工作流；重新下发凭证会推进 Agent Epoch，使旧凭证与旧报告失效；
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
- Node Transfer / Pending Transfer：后续阶段，MVP 不实现。

---

## 5. 观测与采集

### 5.1 观测维度

每个 Component Observation 同时表达三个独立维度：

~~~text
Collection State: Starting | Ok | Error | Disabled | Unsupported
Value State:      Current | LastGood | AuthoritativeEmpty | None
Freshness State:  Fresh | Stale | Unknown
~~~

规则：

- 采集失败只更新状态与错误，绝不覆盖 LastGood；
- Unknown、Stale、从未观测、Disabled、Unsupported 绝不渲染为 `0`、`false` 或 Healthy；
- 采集成功且结果为空是 AuthoritativeEmpty，不是 Unknown；
- 一个 Component 失败不阻塞同一 Node 的其他 Component；一个 Node 失败不阻塞同一 Agent 的其他 Node；
- Host Observation 每 Agent 采集一次，被 Node 视图引用而不重复计入。

### 5.2 Host Observation

每 Agent 采集一次：CPU、Load、Memory、Disk/Mount、Network Throughput、Agent 与 Server 时间偏差（Clock Unreliable 诊断）、Agent Store/Spool 状态。

### 5.3 Node Process Observation

每个 Node 独立采集：进程是否存在、PID 或 PID 文件、进程身份校验、进程 CPU/Memory、启动时间或运行时长、进程错误。MVP 只观察，不重启、不停止、不升级、不执行命令。

### 5.4 Node RPC Observation

每个 Node 独立采集：RPC 可连接性、Client Version、RPC Namespace、RPC Capability（必须实际探测，不能只凭 Client Version 推断）、NodeInfo 中的 Network Identity、当前 RPC 错误与延迟。

### 5.5 Node Chain Observation

每个 Node 独立采集：

- 当前 Head；
- Sync 状态；
- Consensus 状态；
- Peer Count Observation（当前连接 Peer 数量，见 §5.7）；
- 最新 Block Summary（见 §6）；
- 区块交易数量；
- 观察时间与 freshness。

MVP 不保存完整交易内容，不采集 Peer 集合（Peer Snapshot 是未来概念）。

### 5.6 Consensus 字段

Consensus 表示 Node 当前的协议状态，不是 Validator 管理。暂定字段：

- `epoch`
- `view_number`
- `highest_qc_block`
- `highest_lock_block`
- `highest_commit_block`

MVP 不包含 Validator 成员管理。

### 5.7 Peer Count Observation

当前 Peer 只采集数量：成功采集可以为权威的 0；采集失败保留最后成功的数量及其年龄。完整 Peer 集合（Peer Snapshot）与 Peer Presence 是未来概念。

---

## 6. Block Summary 与 Block History

### 6.1 Block Summary

保留完整的当前 Block Summary 字段，包括 Block Production Attribution（Coinbase、Seal Signer Match、Protocol Proposer 三者区分，不得坍缩成单一推断标志）。**（暂定，Q40：这是唯一保留的显式复杂度权衡，未来可能重议。）**

最新区块通过每 Node 独立的 Head Subscription 触发，按 header hash 完成 Block Resolution（获取并校验区块）。订阅只是采集触发器；断线、启动或订阅跳跃期间错过的区块不追溯。

### 6.2 归属与放置

- Agent 把最新 Block Summary 放进 Node Observation（AgentReport 的 nodes[] 字段）。
- Agent 端只保留有界的 Block Summary 与显式 History Gap 待上报项，不拥有 Server 的历史高水位或网络级历史。
- Server 从 accepted reports 追加近期 Block History，并以 Node 为范围维护 Historical High-Water Mark、可恢复 Gap coverage 和 resync replay 诊断。
- History 是 best-effort：Agent 或网络故障期间错过的区块不追溯；Agent 声明的显式 History Gap 可被 Server 有界记录并去重。

### 6.3 全局历史窗口

- Server 保留一个全局可配置的时间窗口，Admin 可动态修改。
- 窗口变更立即生效；缩短窗口时异步删除过期历史；延长窗口不能恢复已删除或已错过的数据。
- 必须提供安全的 min/max/default 边界；边界外的值被拒绝。
- 每次变更把 old/new 值和操作者写入 Audit Event。

### 6.4 明确排除

MVP 没有 Block Explorer，也不保存完整交易 Body。这些能力保留在 `CONTEXT.md` 作为未来领域概念；History Gap、Gap Backfill、Historical High-Water Mark 与 Resync Episode 仅限于本节定义的 Agent 有界待上报项和 Server 有界 ingestion 处理。

---

## 7. Agent 设计

### 7.1 配置与 CLI

CLI：`validate-config`、`collect-report`、`run`、`shutdown`。没有 enroll 命令；凭证由 Admin 预先配置。

~~~toml
server_url = "https://monitor.example.com"
credential_file = "/var/lib/platpulse-agent/credential"
state_db = "/var/lib/platpulse-agent/agent.db"
collection_interval_seconds = 5

[[nodes]]
node_id = "..."
network_key = "platon-mainnet"
rpc_endpoint = "ipc:///var/lib/platon/data/platon.ipc"

[nodes.process]
pid_file = "/var/run/platon.pid"
~~~

Server 不下发或修改 `nodes.rpc_endpoint`。

### 7.2 Agent Store 与 Durable Spool

流程：

~~~text
每 Node 长连接 Head Subscription → 解析并立即持久化待上报 Block Summary
每 5 秒采集当前观测 → 生成完整 AgentReport → 校验 → 写入 Agent Store
独立 1 秒发送循环 → 最老报告优先 → 校验 Receipt → 事务性删除已确认报告
~~~

`collection_interval_seconds` 可配置范围为 1–300 秒，默认 5 秒。区块订阅、当前观测采集、报告组装与发送相互解耦；线上仍只使用完整、不可变的 AgentReport，不按数据类型拆分 wire protocol。

要求：

- 报告 bytes 不可变；重试使用相同 `report_id` 与 bytes；
- 投递失败保留原报告；最老报告优先投递；
- Spool 有明确的大小和年龄上限；
- 溢出时丢弃最老的未确认历史报告、记录诊断日志，并保留当前状态的采集与投递；
- Spool 不是用户可见历史；
- Agent Store 损坏时 fail-closed：停止继续伪造报告，等待人工处理。

### 7.3 AgentReport

~~~text
AgentReport
├── protocol_version
├── report_id
├── agent_id
├── agent_epoch
├── report_sequence
├── generated_at
├── inventory_revision
├── host            # 一次 Host Observation
└── nodes[]         # 每个 Node 的完整当前视图（含最新 Block Summary）
~~~

- 报告不可变、报告级原子；
- 不允许 Agent-level chain state；不允许 null、缺失字段和零值混淆；
- Server 接收后重新校验所有身份、时间、数量和边界。

### 7.4 Report Receipt

Receipt 是简单的报告级确认，不是归档，也没有按 Node 的部分成功矩阵：

~~~text
ReportReceipt
├── report_id
├── report_body_sha256
├── disposition: accepted | rejected
├── server_time
└── rejection_code   # 稳定的拒绝码
~~~

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
- 人类登录（仅 Owner）与 Audit；Site Access Mode 决定 Home 匿名可读性；
- 同源托管 WebUI 静态资源。

Server 不连接 Node RPC、不远程控制、不根据 Agent 输入自动创建 Network、不用 Agent 级视图合并多 Node 链状态。

### 8.2 最小数据模型

至少包含：

~~~text
users
sessions
agents
agent_credentials
networks
nodes
node_current_state     # 当前投影（含各 Component 状态）
report_receipts
block_history          # 受全局窗口约束
server_settings        # 全局历史窗口、Site Access Mode 等
audit_events
~~~

不提前创建后续阶段的表族（alert_*、notification_*、validator_*、geo_cache、operations、node_transfers、retention_policies、backup_artifacts）。

### 8.3 Report Ingestion 事务

1. 验证 Agent Credential；
2. 验证 Agent ID、Epoch、Report ID、sequence 与 body hash；
3. 验证 Node Inventory 与 Node 归属；
4. 验证 Network Identity；
5. 幂等检查：重复 report 返回同一 Receipt，不重复写投影与历史；
6. 更新 Agent 与各 Node 当前投影；
7. 追加合法的 Block History（受全局窗口约束）；
8. 写入 Report Receipt；
9. 发布受影响资源的 SSE invalidation；
10. 提交事务。

任一步骤失败都返回明确结果，绝不产生半份投影。

### 8.4 HTTP API

Agent API：

~~~text
GET  /api/agent/v1/time
POST /api/agent/v1/reports
~~~

Public API：

~~~text
GET  /api/public/v1/access
GET  /api/public/v1/networks
GET  /api/public/v1/networks/{network_key}
GET  /api/public/v1/nodes/{node_id}
GET  /api/public/v1/nodes/{node_id}/history
GET  /api/public/v1/events
POST /api/public/v1/login
POST /api/public/v1/logout
GET  /api/public/v1/session
~~~

Admin API：

~~~text
GET  /api/admin/v1/overview
GET  /api/admin/v1/agents
GET  /api/admin/v1/agents/{agent_id}
GET  /api/admin/v1/nodes
GET  /api/admin/v1/nodes/{node_id}
PUT  /api/admin/v1/nodes/{node_id}/metadata
GET  /api/admin/v1/networks
POST /api/admin/v1/networks
PUT  /api/admin/v1/networks/{network_key}
GET  /api/admin/v1/history-window
PUT  /api/admin/v1/history-window
GET  /api/admin/v1/access-mode
PUT  /api/admin/v1/access-mode
GET  /api/admin/v1/sessions
DELETE /api/admin/v1/sessions/{session_id}
GET  /api/admin/v1/audit
~~~

MVP 不为后续能力预留空路由。

---

## 9. WebUI

页面与交互契约的权威文档是 `docs/design/webui.md`；此处只列边界。

### 9.1 Home

只读 Public Projection，Network → Node → Node Detail：

- Network 列表；
- Network 概览（Active Node 列表/卡片）；
- Node Detail：Node Health Summary、freshness、当前 Head、Sync、Consensus、近期 Block History（受全局窗口约束）、Peer Count、Process 摘要、脱敏的 Host 百分比。

Home 不展示：凭证、RPC Endpoint 原文、内部错误堆栈、Agent/Host 拓扑、任何操作入口。已退休/已删除/未知 Node 使用不泄漏信息的 unavailable 文案。Site Access Mode 为 Private 时 Home 路由要求登录，为 Public 时匿名可读。

### 9.2 Admin

认证（Owner）后使用，页面组：

1. Overview；
2. Agents；
3. Nodes；
4. Networks；
5. History Window；
6. Site Access、Sessions 与 Audit。

Admin 的 Node 页面聚焦配置与诊断（显示名、RPC Endpoint 诊断、Node Inventory/生命周期、freshness 摘要），不复刻 Home 的完整 Node Detail。Admin 不执行远程 Node 操作。

### 9.3 REST / SSE / 响应式

- REST 是唯一权威数据来源；SSE 只发 invalidation/reset 信号，不携带完整业务 DTO；
- 收到 invalidation 后 WebUI 重新读取对应 REST 资源；
- Home 与 Admin 查询缓存隔离；权限变化先清敏感缓存再重新验证；
- 必须在 360×800、390×844、768×1024、1280×800 可用：无水平溢出、表格降级为卡片、键盘可完成登录/导航/主要操作、状态不只靠颜色、Reduced Motion 下不依赖动画表达状态。

---

## 10. 身份与安全

### 10.1 Agent 身份

- Agent Credential 由 Admin 预先配置，只能访问 Agent API；
- Server 不保存 Credential 明文；凭据文件 Agent 用户可读、其他用户不可读；
- 重新下发凭证（provisioning）推进 Agent Epoch，旧凭证与旧报告失效；MVP 不提供 WebUI 轮换流程；
- 不静默创建相同身份的第二个 Agent；
- Human Session 不能访问 Agent API。

### 10.2 Human 身份

MVP 只支持一种人类主体：Owner（初始化时创建）。支持：Session Cookie；Argon2id 密码哈希；登录限流；Session 撤销；基础 CSRF/Origin 校验。Home 的匿名可读性由 Site Access Mode 决定（默认 Private）。

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
- WebUI SSE 断开显示 `Live updates paused`，通过 REST 重新获取。

### 11.4 数据边界

MVP 的观测/历史数据只有两种（Receipt、设置、Audit 等支撑数据见 §8.2）：

~~~text
Current Projection   最新已接受状态（含 LastGood）
Block History        全局窗口约束的近期 Block Summary（best-effort）
~~~

Server 可以保留有界、去重的 History Gap 记录表达缺失区间；缺失区间不做零值填充。

---

## 12. 部署

- Agent：Linux x86_64/aarch64，建议 systemd；每 Host 一个 Agent；独立 state directory；credential 与 SQLite 严格权限；只需访问本地 RPC Endpoint 与 Server HTTPS 地址。
- Server：单进程、单 SQLite、同源托管 WebUI；`/health/live` 判进程存活，`/health/ready` 判 SQLite、Owner 初始化与 WebUI 资源；Backup/Restore 不属于 MVP 启动流程。
- WebUI：React + Vite 构建为静态资源，由 Server 同源托管，生产环境不单独运行 Node.js。

---

## 13. MVP 验收标准

### 13.1 采集与上报

- 一个 Agent 配置两个 Node 可同时采集；
- 一个 Node RPC 失败时另一个 Node 仍能上报；
- Host Observation 只采集一次；
- Agent 重启后未投递报告不丢失；
- Server 收到重复报告不重复写投影与历史；
- Receipt 校验失败时 Agent 保留原报告。

### 13.2 区块历史

- 最新 Block Summary 随 Node Observation（AgentReport 的 nodes[] 字段）上报，含高度、Hash、时间、交易数量与 Block Production Attribution；
- Server 从 accepted reports 追加 Block History；
- 历史窗口缩短后过期历史被异步删除；显式 History Gap 记录保持有界且去重；
- Network Identity Mismatch 时历史不并入注册 Network。

### 13.3 Server 与权限

- Owner 未初始化时 Server 报告 setup required；
- Agent Credential 不能访问 Human API，Human Session 不能访问 Agent API；
- Site Access Mode 为 Private 时未登录请求不能读 Home API；为 Public 时匿名可读 Home，Admin 仍需 Owner 登录；
- 所有 mutation 有基础 CSRF/Origin 校验与 Audit；
- 历史窗口与 Site Access Mode 变更记录 old/new 值与操作者，边界外的值被拒绝。

### 13.4 WebUI

- Home 从 Network 列表进入 Node Detail；站点 Private 时 Home 要求登录，Public 时匿名可读；
- Node Detail 显示当前健康、Head、Sync、Consensus、Peer Count、Block History 与 Host 摘要；
- Admin 的 Node 页面不复制 Home 的完整 Node Detail；
- SSE 断开显示 `Live updates paused`；invalidation 后通过 REST 重取；
- 360px 无水平溢出；Unknown、Stale、Error 不被渲染成 Healthy。

---

## 14. 设计原则

PlatPulse MVP 是：

~~~text
轻量 Agent + 可靠 Report Spool
+ 一个中心 Server
+ SQLite 当前投影 + 有界 Block History
+ 一个清晰的 Web Dashboard（Home 只读 / Admin 配置）
~~~

任何新增设计都必须先回答：

1. 是否直接服务于 Node 监控主链路？
2. 是否必须进入 Agent、Server 或 WebUI 的核心边界？
3. 延后是否会阻塞 Agent → Server → WebUI 链路？
4. 是否可以作为独立扩展，而不是提前进入核心协议与数据库？

后续阶段的扩展必须满足：不改变核心边界、不向 MVP 必需协议塞字段、不创建没有真实场景支撑的抽象、有独立的设计、测试与回滚边界。
