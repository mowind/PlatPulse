# PlatPulse Server / Agent / WebUI 设计方案

## 1. 文档状态

- 状态：已确认的目标架构，尚未开始实现
- 修订日期：2026-08-12
- PlatPulse 代码基线：`b02047423083c6c4a0ccba3c9270da208208f52c`
- PlatON-Go 事实基线：官方 `develop` 分支 commit `887be9c181b0009eb6f7bfc5897c14c300cf58c7`
- ChainDash 参考基线：`../chaindash` commit `2ba9c2ff9d2b2d5e1251d88df97331af591ffecb`
- 适用范围：`platpulse-core`、`platpulse-agent`、`platpulse-server`、`platpulse-web`
- 明确不适用：TUI、ChainDash 迁移兼容、Agent Endpoint 故障转移

本文描述 PlatPulse 的绿地 Server–Agent–WebUI 架构。ChainDash 仅用于核实已经被真实使用验证过的采集模式和产品需求，不继承其进程模型、共享状态、TUI、端点故障转移或数据归属。

本文中的规范性用词：

- **必须**：实现和验收不可省略；
- **不允许**：违反即破坏安全或领域不变量；
- **默认**：Owner 可在明确边界内配置；
- **后续阶段**：属于目标架构，但不阻塞 Phase 1 首个纵向切片。

领域术语以仓库根目录 `CONTEXT.md` 为准。

---

## 2. 执行摘要

PlatPulse 是一个 Linux-first、单租户、单 Server、SQLite 的 PlatON 节点监控系统：

```text
┌──────────────────────────────────────────────┐
│ Host                                         │
│                                              │
│  PlatON Node A ─┐                            │
│  PlatON Node B ─┼─ platpulse-agent           │
│  PlatON Node C ─┘  - per-Node collectors     │
│                     - AgentStore / spool      │
└───────────────────────┬──────────────────────┘
                        │ outbound HTTPS
                        │ AgentReport v1
                        ▼
┌──────────────────────────────────────────────┐
│ platpulse-server                             │
│ - Report Ingestion                          │
│ - SQLite current projections + history      │
│ - auth / Admin / Audit                       │
│ - alerts / notification outbox              │
│ - Validator / Geo collectors                │
│ - REST + SSE + Web assets                    │
└───────────────────────┬──────────────────────┘
                        │ same-origin REST/SSE
                        ▼
┌──────────────────────────────────────────────┐
│ platpulse-web                                │
│ - Home Dashboard: Network → Node             │
│ - Admin Dashboard: operations and settings  │
│ - desktop / tablet / mobile responsive       │
└──────────────────────────────────────────────┘
```

最重要的不变量：

1. 一台 Host 运行一个 Agent；一个 Agent 监控本 Host 上多个 PlatON Node。
2. 一个 Node 只有一个 IPC/WS/WSS RPC Endpoint，不支持 Endpoint failover。
3. Block、transaction、consensus、Peer、process freshness 和 Collector error 都按 Node 隔离，绝不合并成 Agent 级权威链状态。
4. Host Observation 每个 Agent 只采集一次；Node 页面可以引用共享 Host 百分比，但存储和汇总不能复制计数。
5. 每个 Node 独立订阅新区块头，再按 Header hash 获取区块摘要；不保存完整交易。
6. 采集失败保留最后成功值；未知、stale、disabled、unsupported 与权威空值具有不同语义。
7. Node 删除本地链数据重同步时，current head 可以下降；既有历史、高水位和累计统计不被回放数据覆盖或重复累计。
8. AgentReport 是落盘后发送的不可变完整当前视图加新样本；Server 事务提交后才确认。
9. Home 和 Admin 使用不同 DTO、route group 和权限查询；Public Projection 不是 Admin DTO 的运行时删字段版本。
10. WebUI 必须同时适配桌面、平板和移动端，并满足基本可访问性；移动端不是后续增强项。

---

## 3. 目标、约束与非目标

### 3.1 目标

1. 多个 Agent 主动向一个 Server 汇聚观测数据。
2. 一个 Agent 从第一版开始可靠监控多个独立 Node。
3. 展示 Node 的 Host、process、RPC、sync、block、transaction、consensus、Peer 和 Validator 视图。
4. 临时 Agent/Server/RPC 故障不会静默丢失最新状态或重复写入历史。
5. 当前状态、最后成功值、历史、高水位、缺口和错误拥有明确语义。
6. 默认私有的 Home Dashboard 与受认证的 Admin Dashboard 形成完整运维闭环。
7. Alert Incident、Silence、Maintenance 和通知通过 Server 持久化，重启后继续工作。
8. 单机 SQLite 部署可备份、恢复、升级和诊断。
9. 所有安全与脱敏边界由 Server 强制执行。
10. 第一阶段交付小而完整的 Agent → Server → SQLite → REST/SSE → WebUI 纵向切片。

### 3.2 基础部署约束

- v1：单租户、单 `platpulse-server`、SQLite；
- Agent 和 Server：Linux x86_64 / aarch64；
- Agent：Host 原生二进制或 systemd 服务，不作为 v1 容器运行；
- Server：原生二进制或 OCI image；
- WebUI：生产环境由 Server 同源提供，不需要 Node.js runtime；
- Agent 到 Server：仅主动出站；Server 不反向连接 Agent；
- 一个 Node 的 Endpoint：仅 `ipc://`、`ws://`、`wss://`；
- RPC 深度能力优先通过本机 IPC 获得。

### 3.3 明确非目标

v1 和本文目标架构都不包含：

- TUI 或 ChainDash TUI 兼容；
- TUI roadmap；
- Agent Endpoint failover；
- Agent 跨 Host 监控 Node；
- Server 远程修改或下发 RPC Endpoint；
- Web Terminal、任意命令或脚本执行；
- 远程重启 Agent/Node；
- 远程升级；
- Docker Socket 控制；
- 完整 transaction body、receipt、trace、input 或账户级索引；
- 区块浏览器或归档数据库；
- 无限保存逐区块历史；
- 多 Node 合并为一个 Agent chain snapshot；
- 自动 Node takeover；
- 自动修改 Network Identity；
- 前端隐藏代替服务端授权；
- URL credential；
- 自定义 Alert DSL 或脚本；
- 插件、主题和自定义脚本市场；
- 默认调用公网 Geo Provider；
- 在发行物中打包或再分发 GeoLite MMDB；
- 多租户、HA、Server cluster、PostgreSQL；
- Kubernetes Operator；
- SSO/OIDC/TOTP/WebAuthn；
- 通用 Prometheus/Grafana 替代品；
- Windows/macOS Agent 支持。

---

## 4. 身份、归属与生命周期

### 4.1 Host、Agent、Node 与 Network

```text
Host 1 ── Agent 1 ──┬── Node A ── Network Mainnet
                    ├── Node B ── Network Mainnet
                    └── Node C ── Network Testnet

Host 2 ── Agent 2 ───── Node D ── Network Mainnet
```

规则：

- 一台 Host 只运行一个 Agent；
- Agent 只监控本 Host 上的 Node；
- 一个 Agent 可同时监控不同 Network 的 Node；
- Network 属于 Node，不属于 Agent；
- 每个 Node 的采集、freshness、revision、last-good 和 error 独立；
- 一个 Node 的故障不能停止兄弟 Node 的采集、上报或 current projection 更新。

### 4.2 稳定身份

- Agent ID：Enrollment 后由 Server 签发的全局 UUID；
- Node ID：Agent 本地创建并持久化的全局 UUID；
- Network key：配置中的稳定 Registry key，例如 `platon-mainnet`；
- PlatON P2P Node ID、Validator Node ID、BLS key、RPC URL 和显示名称均不能代替 PlatPulse Node ID。

修改 Node 的显示名称、Endpoint 或 owning Agent 不创建新 Node。历史、Alert 和 Home URL 继续使用原 Node ID。RPC Endpoint、Node ID 和 Network key 由 Agent 本地配置声明；display name、label、visibility 和 Alert policy 由 Server/Admin 管理。

### 4.3 Node Inventory

Agent 本地 TOML 是 Node 连接配置的权威来源。Agent 在完整配置校验成功后提交完整 Node Inventory。

```text
latest valid inventory contains Node  → Active
previously present but now absent     → Retired
same Agent declares same ID again     → Active
```

- 无效或局部解析失败的配置不允许提交半份 Inventory；
- 首次出现的 Node 默认 `visibility = private`，必须由 Owner 显式公开；
- Agent offline 不会将 Node 变为 Retired；
- Retired Node 保留身份、历史和 Audit，但不再产生 live offline/RPC failure Alert；
- 永久清除是单独的 Node Purge，只允许明确 Owner/CLI 流程；
- 删除 Agent 不自动删除其 Node 历史。

### 4.4 Node Transfer

Node Transfer 使用 Owner 预授权的两阶段流程：

1. Owner 指定 Node 和目标 Agent，创建可取消、会过期的 Pending Transfer；
2. 源 Agent 在切换前仍是唯一权威来源；
3. 运维人员在目标 Agent 本地配置中声明原 Node ID；
4. 目标 Agent 提交有效 Inventory，且 Network Identity 校验通过；
5. Server 在一个事务中切换 ownership；
6. 原 Agent 后续提交该 Node 时，只拒绝该 Node 条目并产生安全事件；
7. 目标 Agent 未按期声明则转移失效，原归属不变。

Transfer 不允许改变 genesis/chain/P2P network identity；另一条链必须创建新 Node。

### 4.5 Agent Enrollment、Recovery 与冲突实例

- Enrollment Token：短期、单次使用，只能换取 Agent ID 和 Agent Credential；
- Agent Credential：至少 256-bit 随机 secret，只能访问 Agent API；
- Server 只保存 keyed digest；
- 凭据丢失时，Owner 为既有 Agent 创建一次性 Recovery Token；
- Recovery 轮换 credential 并推进 Agent Epoch，不创建重复 Agent；
- 状态目录丢失后必须 Recovery/Reset，不能从 sequence 1 静默覆盖；
- 复制状态目录或并发运行相同 identity 时，Server 拒绝冲突实例并记录安全事件。

---

## 5. Observation 模型

### 5.1 三层 Observation

#### Host Observation

每个 Agent 采集一次：

- CPU、memory、load；
- disk usage；
- Host network throughput；
- Agent clock skew；
- Agent spool 状态。

#### Node Process Observation

每个 Node 可选采集：

- process running state；
- PID identity；
- start time / uptime；
- process CPU/memory；
- process selector 状态。

每个 Node 最多配置一个显式 selector：

```toml
[nodes.process]
systemd_unit = "platon-validator-a.service"
```

或：

```toml
[nodes.process]
pid_file = "/run/platon-validator-a.pid"
```

不允许按 process name、command line 或 RPC port 猜测。Agent 要验证 PID start time 和 executable，避免 PID reuse。未配置时状态为 `Disabled`，RPC/chain 采集继续。

#### Node Chain Observation

每个 Node 独立采集：

- RPC reachability 和 capability；
- Network Identity；
- current head 和 sync；
- block/transaction summaries；
- consensus current state；
- Peer Snapshot；
- Node software/static metadata；
- Block Production Attribution。

### 5.2 Observation Envelope

每个独立 component 使用统一状态：

```text
Starting | Ok | Error | Disabled | Unsupported
```

并保存：

```text
attempted_at
latest.observed_at
received_at
state_revision
value_revision
latest
error
```

语义：

- collection failure 只更新状态和 error，不用空值覆盖 last-good；
- Home/Admin 同时显示 last-good value、stale age 和当前错误；
- 成功采集到空 Peer 集合等结果是权威空值，必须清除旧集合；
- missing、unknown、never observed 或 stale 不得显示为 `0`、`false` 或 Healthy；
- 一个 component 失败不阻止同 Node 或同 Agent 的其他 component 更新；
- Server 重新验证 Agent 上报字段，不能把 Agent 当作信任边界内输入。

### 5.3 健康维度

权威事实保持独立：

- Agent liveness；
- Inventory lifecycle；
- Node process；
- RPC reachability；
- observation freshness；
- synchronization；
- consensus；
- Host resource pressure。

Home 可以派生：

```text
Healthy | Warning | Critical | Unknown
```

但必须附带 reason；该 summary 不是持久化权威状态，Admin 必须展示全部维度。

---

## 6. PlatON RPC 与区块采集

### 6.1 源码事实基线

PlatON RPC 行为以 `PlatONnetwork/PlatON-Go` `develop` commit `887be9c181b0009eb6f7bfc5897c14c300cf58c7` 为准，并在 Agent 启动时执行 runtime capability probe。部署方可能运行其他 commit 或私有 patch，不能仅由 version string 推断能力。

已直接核实：

- `cmd/platon/config.go` 调用 `utils.RegisterFilterAPI`；
- `cmd/utils/flags.go` 将 `filters.NewFilterAPI` 注册到 `platon` namespace；
- RPC registry 将 `eth` namespace lookup 映射到 `platon`；
- 因此 Alloy/ChainDash 使用的 `eth_subscribe("newHeads")` 可调用该订阅；
- `platon_getBlockByHash(hash, false)` 返回交易 hash 列表而非完整交易；
- `platon_*` block timestamp 是毫秒，`eth_*` alias 转换为秒；
- `net_listening` 在该基线中恒为 `true`，不能作为真实健康指标；
- `admin` 和 `debug` namespace 同时包含高权限或高成本方法，不能开放到公网。

Agent 必须记录 client version、实际 namespace 和 capability probe 结果。

### 6.2 Endpoint

一个 Node 只配置一个 PubSub-capable Endpoint：

```text
ipc:///path/to/platon.ipc
ws://127.0.0.1:6790
wss://node.example.com
```

- v1 不支持 HTTP/HTTPS Endpoint；
- 不配置第二 Endpoint；
- 不做 transport 或 URL failover；
- IPC 优先；
- `admin_nodeInfo`、`admin_peers`、`debug_consensusStatus` 仅应经 IPC，或受保护的 loopback/私网 WS 使用；
- 不为监控把完整 `admin`/`debug` namespace 暴露到远程网络；
- method absent 为 `Unsupported`；已 probe 可用后失败才为 `Error`。

### 6.3 每 Node 区块链路

每个 Active Node 独立运行：

```text
eth_subscribe("newHeads")
        │
        ▼
bounded Header queue
        │
        ▼
platon_getBlockByHash(header.hash, false)
        │
        ▼
verify number/hash/parentHash
        │
        ▼
Block Summary + Attribution
        │
        ▼
AgentStore durable sample
```

规则：

- Header subscription 是正常实时采集唯一触发源；
- Header 只作为实时触发和 block identity；Block Resolution 返回的原生 `platon_*` block timestamp 才是 Block Summary 的权威链时间；
- 不存在 Agent 级全局 block subscription；
- Resolver 按 hash 获取，不像 ChainDash 按 height 获取，避免 reorg race；
- `false` 只获取 transaction hashes，用数组长度统计 transaction count；
- 不获取完整 transaction body；
- Header reader 与 Block Resolver 解耦；单次 block RPC 失败不重建正常订阅；
- 只有连接/订阅本身失败才重连；
- 每 Node 有独立 bounded queue、backoff 和 jitter；
- 一个 Node 队列拥塞不影响其他 Node；
- 返回 block 的 hash、number、parent hash 不一致时拒绝该 sample 并记录 Collector error。

### 6.4 Gap Backfill

正常运行不轮询新区块，但以下情况允许有限回补：

- 首次订阅建立完成后查询一次 current head，覆盖连接竞态；
- Agent restart；
- subscription reconnect；
- 发现 head height jump；
- Header queue overflow。

回补规则：

- 通过 point query 解析缺失高度；
- sample source 标记为 `GapBackfill`，正常流为 `Subscription`；
- 回补有严格高度/数量/时间上限；
- 缺口过大时记录 History Gap，直接从 current head 恢复；
- subscription unsupported/error 时 Block component 明确失败，不静默变成永久 polling mode。

### 6.5 Block Summary

每个 sample 至少保存：

```text
node_id
network_identity
block_number
block_hash
parent_hash
block_timestamp_ms
observed_at
transaction_count
block_interval_ms?
source = Subscription | GapBackfill
coinbase
seal_signer_key_fingerprint?
seal_signer_match = Self | Other | Unknown
protocol_proposer = Verified(identity) | Unknown
attribution_reason
```

不保存：

- complete transaction body；
- receipt、trace、input；
- account transaction index；
- complete debug block tree/votes。

### 6.6 本 Node 区块归属

`miner`/Coinbase、Header Seal signer 和 CBFT protocol proposer 是三个不同概念：

- RPC `miner` 是 `header.Coinbase`，不是 producer proof；
- Agent 可按 pinned/fork-aware Header seal 规则恢复 ECDSA public key；
- 与 `admin_nodeInfo.enode` 中 Node key 比较，得到 `Seal Signer Match`；
- `Self` 表示 seal key 与当前有效 Node key 匹配；
- `Other` 表示 signer 已知且不匹配；
- 缺少 key、解析失败、fork 未验证、key 历史不完整时为 `Unknown`；
- Admin 展示 recovered signer fingerprint、对应 Node key fingerprint、key 有效期和 attribution reason；
- 当前标准 block/QC RPC 不提供足以证明历史 protocol proposer 的持久化 NodeIndex，因此默认 `protocol_proposer = Unknown`；
- Node key 轮换要保存有效时间范围，不能用当前 key 反推全部历史。

Home 可以显示“本节点签署”，不得显示未经证明的“本节点是协议出块者”。

### 6.7 重同步与高水位

假设历史最高为 `100000`，Node 删除数据后从 `0` 重同步：

```text
historical_high_watermark = 100000
current_head              = 0 → 1 → ...
state                     = Resyncing
```

必须区分 current state 与 append-only history：

- current head 实时下降/上升并展示；
- 普通重同步 replay 中，高度 `<= historical_high_watermark` 且不属于已登记 open gap 的区块不覆盖 Block Summary；
- 不重复累计 transaction、seal match 或历史 sample；
- RPC、sync、consensus、Host、process current state 继续更新；
- replay 阶段可合并连续 Header，只定期解析最新 block 以刷新 current state；
- 第一次超过 high-water mark 后恢复逐 Header history append；
- 同高度不同 hash 且仍有 identity evidence 时记录 Chain Divergence Observation，不覆盖原记录；
- genesis hash、chain ID 或 P2P network ID 变化则进入 Network Identity Mismatch，停止合并区块历史；
- Resyncing 使用独立 progress/stalled Alert，不应用普通 block-stalled 语义。

`historical_high_watermark` 只表示最大已接受高度，不能独自证明其下每个高度都已保存。Server 还要维护独立于 raw retention 的紧凑 coverage ledger：

```text
block_history_state
- node_id
- historical_high_watermark
- cumulative_block_count
- cumulative_transaction_count
- cumulative_self_seal_count

block_coverage_intervals
- node_id
- first_height
- last_height
- status = Covered | OpenRecoverableGap | PermanentGap

block_identity_window
- node_id
- height
- block_hash
- retained_until
```

语义：

- 正常 `Subscription` 只在 height 大于 high-water mark 时追加，并推进 coverage/high-water mark；
- `GapBackfill` 可以且只能填补显式 `OpenRecoverableGap`，即使其 height 小于 high-water mark；成功后只累计一次并收缩/关闭 gap；
- `PermanentGap` 不接受迟到样本，避免无限期重扫；
- 非 gap 的 `height <= high-water mark` 一律视为 resync replay，不写历史、不累计；
- recent identity window 用于检测同高度异 hash；超过该窗口后旧 replay 仍被忽略，但不能声称已验证无 divergence；
- raw Block Summary 删除不能删除 history state、coverage interval 或尚在 retention 内的 identity evidence。

---

## 7. Network 与 Consensus

### 7.1 Network Registry

Server 管理：

```text
network_key
display_name
genesis_hash
chain_id
p2p_network_id
address_hrp
```

Phase 1 已包含最小 Network Registry 存储与本机 CLI bootstrap，例如：

```bash
platpulse-server network create \
  --key platon-mainnet \
  --display-name "PlatON Mainnet" \
  --genesis-hash <hash> \
  --chain-id <id> \
  --p2p-network-id <id> \
  --address-hrp lat
```

该命令要求显式、完整 identity tuple，写 Audit，且不会依据首个 Agent 的自由文本或观测值自动信任/创建 Network。Phase 2 增加的是 Registry 的 Admin 管理体验和完整 lifecycle，而不是首次引入其身份存储或验证。

Agent 配置使用 `network_key`，并上报实际观察到的完整 Network Identity。Server 按 Registry 验证：

- 同 Network 的 Node 必须匹配 identity tuple；
- display name 或 `network_key` 不能代替链身份；
- mismatch 时 current diagnostics 可继续，但 block history 不再合并；
- Transfer 也必须通过同一校验；
- 未知 `network_key` 的 Inventory/current/history 使用稳定 rejection code 拒绝，不创建隐式 Network；
- Server 不自动修改 Registry。

### 7.2 Observed Network Head

```text
Observed Network Head
  = 同一 Network Identity 下 eligible fresh Active Node 的最大 current head
```

候选：

- Agent online；
- Node Active；
- Network Identity match；
- block observation fresh；
- Resyncing Node 默认不参与，除非没有其他 fresh source。

projection 记录贡献 Node、observed time 和 source count。一个来源时标记 `LowConfidence`。它只是本部署观察到的参考值，不声称是全网绝对链头。严重 sync lag Alert 不得只依赖低置信度参考值。

### 7.3 Consensus Observation

默认每 2 秒采集 `debug_consensusStatus` 中的有界字段：

- epoch；
- view number；
- current validator membership；
- highest QC block；
- highest lock block；
- highest commit block。

不持续保存：

- complete `blockTree`；
- votes；
- 高基数 view QC 内容。

`validator = true` 只表示当前 validator pool 包含本 Node，不自动创建 Validator 或证明某区块由它提出。

---

## 8. Agent 架构

### 8.1 Runtime

```text
Agent Runtime
├── Host Collector
├── Node Supervisor A
│   ├── RPC Session + Capability Probe
│   ├── Head Subscription
│   ├── Block Resolver
│   ├── Sync/RPC Collector
│   ├── Consensus Collector
│   ├── Peer Collector            # Phase 3
│   └── Process Collector
├── Node Supervisor B
│   └── ...
├── Report Planner
└── Report Sender
```

- 一个 Node 对应一个 RPC provider connection；subscription 与 RPC 调用复用它；
- Node Supervisor 拥有自己的 current state；
- Collector 通过 bounded channel 发送 typed observation；
- 不使用全局 `Arc<Mutex<Data>>`；
- panic 被 Supervisor 捕获、记录并按退避重启；
- cancellation token 控制 shutdown；
- task 不允许无限阻塞退出；
- Host Collector 与任何 Node RPC 故障解耦。

### 8.2 Agent 配置

```toml
server_url = "https://monitor.example.com"
credential_file = "/var/lib/platpulse-agent/credential"
state_db = "/var/lib/platpulse-agent/agent.db"

[[nodes]]
id = "0195..."
display_name = "Validator A"
network_key = "platon-mainnet"
rpc_endpoint = "ipc:///data/platon.ipc"

[nodes.process]
systemd_unit = "platon-validator-a.service"
```

规则：

- credential 与普通配置分离；
- `platpulse-agent validate-config` 完整校验；
- `platpulse-agent generate-node-id` 生成 UUID；
- 配置必须整体有效；
- v1 修改后重启生效，不自动 watch；
- 不支持环境变量插值、浏览器编辑、远程配置或 shell expansion；
- `display_name` 只作为首次见到该 Node 时的 bootstrap 建议或 Agent 本地诊断名称；Node 已注册后不能通过 Inventory 覆盖 Server 管理的 display name；
- credential file 和 state DB 必须只允许 Agent OS user 读取；
- RPC URL 中的 credential 必须在日志和错误中脱敏；
- 容器内运行的 PlatON Node 可通过本地 RPC 监控；若没有 Host 可验证的 PID，则 Process Observation 为 `Disabled`，不要求 Docker Socket。

### 8.3 默认 cadence

```text
Head Subscription             continuous
Block Resolution              every subscribed head
Consensus Observation         2s
RPC / Sync Observation        5s
Host Observation              5s
Node Process Observation      5s
Peer Snapshot                 30s
Node/client static metadata   5m
AgentReport flush             5s
```

- cadence 可在 Agent 本地配置，但有安全 min/max；
- 使用确定性 jitter，避免 Agent 同步上报；
- Collector failure 使用 backoff，last-good freshness 继续增长；
- Report flush 与 Collector cadence 解耦；
- 达到 body/sample 阈值可提前 flush。

### 8.4 时间模型

区分：

```text
block_timestamp      链时间
observed_at          Agent UTC wall clock
monotonic_elapsed    Agent 单次 boot 的单调耗时
received_at          Server commit AgentReport 的 UTC 时间
```

- block interval 使用相邻权威 block timestamp；
- Header arrival latency 使用 monotonic clock；
- Agent liveness 只依据 Server `received_at`；
- Server 响应返回 `server_time`，Agent 估算并上报 clock skew；
- clock skew 过大进入 Clock Unreliable，但不丢弃其他有效 observation；
- 不可靠时钟下跨 Host 亚秒传播比较为 Unknown。

### 8.5 Agent Store

Agent 使用一个深的 `AgentStore` module 隐藏 SQLite 顺序与事务：

```rust
append_block_sample(...)
plan_report(...)
next_report(...)
apply_receipt(...)
record_delivery_failure(...)
```

内部负责：

- identity/epoch/boot/sequence；
- pending Block Summary；
- immutable report planning；
- canonical body 和 body hash；
- sample 到 report 的一次性分配；
- oldest-first delivery；
- Stored Report Receipt 的分层 disposition 应用和 cleanup/requeue；
- spool capacity；
- History Gap；
- crash recovery 和 integrity check。

不变量：

- Block Summary 尽快落盘，不只停留在内存；
- report body 一旦落库不可修改；
- retry 读取完全相同的 bytes；
- sequence 与 report 创建同事务；
- sample 在 receipt 应用前不能重复分配；
- receipt 是整份 immutable report 的终局 ACK；AgentStore 必须先在一个事务中处理全部 Inventory/Node/sample disposition，之后才能删除 report row；
- current observation 是完整快照，被拒后不单独重放，下一份 report 携带最新 current；
- 被标记 retryable 的 Block Summary/History Gap 从旧 report 解绑定并重新排入后续 report；terminal rejection 进入本地 rejection ledger，并形成待上报的 `server_rejected` History Gap，不能静默丢失；
- overflow 优先丢弃最旧、非 in-flight 的中间 report/sample，并保留最新完整 current report；
- overflow 不修改任何已持久化的 immutable report；它在同一事务中记录 pending History Gap，并由下一份新 report 携带，必要时立即规划一份 gap/current report；
- report sequence 只要求单调，不要求连续；Server 接受 sequence jump、立即记录 Report Gap，并等待后续 spool diagnostics/History Gap 给出精确范围；
- 单份最小完整 current report 若仍超过 protocol hard limit，Agent 进入 `ReportTooLarge` degraded/fatal 状态，不突破上限、不循环删除，也不发送不可解析报告；
- 被删除范围形成精确 History Gap；
- Collector/Sender 不直接执行 SQL。

### 8.6 Spool

Agent SQLite spool 同时受最大 bytes 和最大 age 限制：

- report 先持久化再发送；
- 只有收到并成功应用 Stored Report Receipt 后才清理该 report；transport timeout/5xx 不构成 ACK；
- oldest-first；
- 当前唯一 in-flight report 永不被 capacity cleanup 删除；
- 永远保留最新完整 AgentReport；
- 记录 dropped report/sample count、sequence range 和时间/高度范围；
- corruption 时隔离损坏文件并进入明确 fatal/error，不静默创建空库；
- 精确默认容量在基准测试后确定。

### 8.7 Agent graceful shutdown

1. 停止建立新 subscription 和周期 RPC work；
2. 取消 Head Subscription，不再接收新 Header；
3. 在独立 drain deadline 内等待已进入 Resolver queue/正在解析的 Header；成功解析的 sample 立即落 Store；
4. drain deadline 后取消剩余 resolution，持久化未完成的高度/hash range；下次启动执行有界 Gap Backfill，超过 backfill 上限则形成 History Gap；
5. current observation 形成 immutable final report；
6. report 先持久化；
7. 在总 shutdown deadline 内尝试发送；
8. 未收到 receipt 的 report 留在 spool；
9. 关闭 SQLite。

强制终止后下次启动从 Store 恢复，不因 deadline 删除数据。测试必须覆盖 queued、in-flight resolution、RPC timeout 和 deadline exhaustion。

---

## 9. AgentReport v1

### 9.1 Route 与编码

```http
POST /api/agent/v1/reports
Content-Type: application/json
Authorization: Bearer pp_agent_<token_id>_<secret>
X-Request-ID: <uuid>
```

- 普通未压缩 JSON；
- Server 在反序列化前执行 8 MiB hard limit；
- Agent 接近 2 MiB 或 sample 阈值时提前 flush；
- v1 不接受 gzip；
- 字段 `snake_case`；
- sequence/revision 等 Rust `u64` 使用 JSON integer；
- timestamp 使用 RFC 3339 UTC，block timestamp 明确为 Unix ms；
- duration 字段以 `_ms` 结尾；
- enum 使用稳定小写 string；
- null、omitted 和 authoritative empty 分开定义。

### 9.2 Envelope

```text
protocol_version
agent_id
agent_epoch
boot_id
previous_boot_id?
boot_transition = Continuing | Closing | DrainedPrevious | RecoveredAfterStale
report_sequence
report_id
generated_at
agent_version
agent_capabilities[]
inventory + revision
host observation
per-Node current component observations
new per-Node Block Summaries
History Gaps
spool diagnostics
```

### 9.3 顺序、Boot 切换与幂等

- Agent Epoch：Server 在 Enrollment、Recovery、Reset 推进；
- Boot ID：每次正常启动生成；
- Report sequence：每 boot 从 1 单调递增；sequence 可以因本地 capacity cleanup 出现 gap，但不能回退；
- Agent 最多一个 in-flight report；backlog strictly oldest-first；
- graceful shutdown 的 final report 标记 `Closing`；Server receipt commit 后把 active boot 标记 closed；
- AgentStore 持久化当前 boot state。进程崩溃重启且旧 boot 未关闭时，新进程先进入 `recovery-drain`：暂不启动新 Collector，继续使用持久化的旧 `boot_id`，oldest-first 发送旧 backlog；
- 旧 backlog receipt 全部应用后，recovery-drain 以旧 boot 的下一个 sequence 生成一份无新链样本的 `Closing` report；其 receipt 应用后才生成新 `boot_id`；
- 新 Boot 第一份 report 携带 `previous_boot_id` + `DrainedPrevious`，Server 只在 previous boot 已 closed 时原子激活它；
- 接受新 boot 后，旧 boot 只允许重放已存在 `report_id`，任何新的旧-boot report 返回 `stale_boot`；
- copied state 的两个实例可以重放相同 immutable backlog；它们在首次提交不同的同-sequence report 或不同新 boot 时发生冲突：Server 事务中只接受一个，另一者稳定返回 `conflicting_boot` 并产生 duplicate-agent security event；
- 同一 `(agent_epoch, boot_id, report_sequence)` 出现不同 report ID/body hash 也是 duplicate-agent conflict；
- 对未知/旧 boot 使用稳定 `conflicting_boot`、`stale_boot` 或 `invalid_boot_transition` code；Recovery/Reset 推进 Epoch 后旧 Epoch 一律拒绝；
- `RecoveredAfterStale` 仅供未来显式的 Server-approved recovery 使用；v1 状态目录丢失或无法完成 recovery-drain 时必须走 Recovery/Reset，不能靠 timeout 自动抢占。
- HTTP retry 复用相同 `report_id` 和 bytes；
- duplicate `report_id` 返回同一 Stored Report Receipt；
- 相同 `report_id` 不同 body hash 是安全/协议冲突；
- 接受更高 sequence 后，旧的非重复 report 不回退 current projection；
- state loss 不能静默重置顺序。

### 9.4 Report Receipt 与 Partial Acceptance

Receipt 顶层 disposition：

```text
Accepted | PartiallyAccepted | Rejected
```

并持久化：

- report ID、body hash、committed time 和 exact response body；
- Inventory disposition；
- accepted component revisions；
- 每 Node current disposition；
- 每个 Block Summary/History Gap 或有界 range disposition；
- stable rejection code、`retryable` 和处置原因。

语义：

- `Accepted` 与 `PartiallyAccepted` 都是整份 immutable report 的终局 ACK；相同 report 重试永远返回第一次 receipt；
- Inventory 是完整集合，必须先独立、整体校验；Inventory disposition 只有整体 `accepted`、`unchanged` 或 `rejected`，不能把“合法 Node 子集”当成新 Inventory；
- Inventory rejected 时保留 Server 上一版 lifecycle/ownership，不得因某 Node field error 把它视为 absent 或 Retired；
- Inventory accepted 后，Node current/history 仍可逐 Node、逐 sample 接受或拒绝；
- retryable sample/range 由 AgentStore 在应用 receipt 时重新排入新 report；terminal rejection 进入 rejection ledger，并以 `server_rejected` History Gap/diagnostic 上报；
- current observation 不重放旧快照；被拒后由下一份完整 report 带最新状态；
- Agent 只有在 `apply_receipt` 事务成功后才删除原 report。

### 9.5 版本兼容

同时使用：

```text
/api/agent/v1/*
protocol_version = 1
agent_version
agent_capabilities[]
```

- optional、可安全忽略字段可留在 v1；
- 字段语义变化、required 删除或幂等变化必须 v2；
- unknown enum 不能默认为 Healthy/false；
- unsupported major 返回 `unsupported_protocol_version`；
- response 包含 Server version、supported majors、server time 和 rotation hint；
- 每个 major 保留固定 JSON fixtures；
- 初始不预建多版本 dispatcher。

---

## 10. Server 架构

### 10.1 Report Ingestion 深模块

Axum handler 只负责 transport、Agent authentication 和 response mapping。核心 interface：

```rust
ingest(
    authenticated_agent,
    report_bytes,
    received_at,
) -> StoredReportReceipt
```

一个 SQLite transaction 内完成：

- body hash / idempotency；
- protocol、epoch、boot、sequence；
- Inventory invariants；
- Node ownership/Transfer；
- Network Identity；
- component revision merge；
- current projections；
- Block Summary/history/high-water mark；
- History Gap/divergence；
- Collector state；
- Alert evaluation input；
- Invalidation Event；
- exact Report Receipt。

错误边界：

- auth、protocol、envelope 或全局 invariant error：拒绝整份；
- 完整 Inventory 先独立校验并给出整体 disposition；Inventory rejected 时不应用 lifecycle revision，不 retire/transfer 任何 Node；
- Inventory accepted 后，单 Node ownership/revision/current field error 可拒绝该 Node current/history，其他合法 Node 同事务提交；
- Block Summary/History Gap 使用 sample/range disposition，明确 accepted、retryable rejected 或 terminal rejected；
- partial commit 返回 `PartiallyAccepted` exact receipt；
- per-Node/sample rejection 使用稳定 code；
- 只有 transaction commit 后返回 receipt；
- commit 前断线时 Agent 安全重试。

### 10.2 Background workers

Server 内部至少包括：

- Alert evaluator；
- Notification delivery worker；
- retention/aggregation job；
- Validator Provider collector；
- Geo database loader/cache cleanup；
- SSE invalidation buffer；
- Session cleanup；
- component health registry。

关键 worker panic/crash 会使 readiness degraded/false，而非静默停止。

### 10.3 Server graceful shutdown

1. readiness 变 false；
2. 停止接收新 connection；
3. 等待 in-flight ingestion；
4. 不启动新 retention/aggregation；
5. notification send 完成或回到 RetryScheduled；
6. SSE 发送 shutdown/reset；
7. WAL checkpoint；
8. 关闭 DB；
9. 超过 deadline 非零退出并记录未完成步骤。

---

## 11. SQLite 数据模型

### 11.1 数据库策略

Agent 和 Server 各自使用 SQLx SQLite，但 schema/migration 完全独立。

共同配置：

```text
foreign_keys = ON
journal_mode = WAL
busy_timeout = explicit
synchronous = FULL  # 默认
```

- release bundled SQLite；
- migration 在监听/采集前执行；
- migration failure 启动失败；
- Server 一个串行 write connection/pool，小型 read pool；
- Agent 单写 connection；
- 不使用 ORM；
- 主要查询 typed SQL；
- 不为每张表建立 repository trait；
- 测试使用临时真实 SQLite。

### 11.2 Server typed tables

建议表族：

```text
agents
agent_credentials
agent_report_receipts

networks
nodes
node_transfers

component_status
current_host_observations
current_node_process_observations
current_node_chain_observations
current_consensus_observations

block_summaries
block_history_state
block_coverage_intervals
block_identity_window
block_history_gaps
report_sequence_gaps
chain_divergence_observations
block_aggregate_1m
block_aggregate_1h

current_node_peers
peer_presence_intervals
peer_aggregate_5m
peer_aggregate_1h
geo_location_cache

validators
node_validator_links
validator_current
validator_history
validator_daily_aggregate
validator_monthly_aggregate

alert_rules
alert_rule_state
alert_incidents
silences
maintenance_windows
notification_events
notification_outbox

users
sessions
audit_events
server_settings
```

规则：

- identity/current/high-frequency query 使用明确列与约束；
- `component_status` 保存 Observation Envelope；
- typed current row 保存 last-good value；
- 完整 AgentReport 成功后不长期保存；
- receipt 保存 hash 和 exact result；
- JSON 只用于 bounded error detail、协议扩展、Audit before/after；
- Web API 不直接读取 AgentReport DTO；
- 不使用通用 JSON/EAV metrics table。

### 11.3 Block retention

默认：

```text
raw Block Summary       7 days
1-minute aggregate      90 days
1-hour aggregate        long-term
History Gap             >= 180 days
Divergence Observation  >= 180 days
Audit Event              >= 365 days
Alert/Notification       180 days
```

Owner 可配置但有安全上下限。retention 分批执行。删除 raw history：

- 不降低 historical high-water mark；
- 不删除 cumulative counters、coverage interval、open/permanent gap state；
- block identity evidence 只按独立 divergence retention 删除；
- 不影响 resync replay 或合法 GapBackfill 判断。

1m/1h aggregate 保存：

- first/last height；
- block count；
- transaction count；
- avg/min/max block interval；
- Self seal match count；
- gap/divergence count。

### 11.4 Server report atomicity

同一事务写入 exact receipt、Inventory disposition、accepted current/history/gap/error、Alert input 和 invalidation。外部通知不在该事务中直接发送，只创建 outbox row。任何 rejected item 不得部分修改其对应 projection；上一版 lifecycle/current/last-good 按各自语义保留。

---

## 12. Human 身份、授权与安全

### 12.1 Principal

- Owner：访问 Admin，管理系统；
- Viewer：只使用 Home；
- Guest：仅在 anonymous Home 被显式开启时使用 Public Projection。

支持多个 Owner/Viewer：

- 不公开注册；
- 首个 Owner 由 Server CLI 创建；
- 不允许禁用/删除最后一个有效 Owner；
- disabled user 的 Session 立即失效；
- Owner 可创建、禁用、重置账户和撤销 Session；
- 所有身份 mutation 进入 Audit。

### 12.2 首次初始化

```bash
platpulse-server init --config /etc/platpulse/server.toml
platpulse-server owner create --username admin
```

`init` 创建 state directory 和 SQLite、执行 migration、生成独立 pepper file、校验 file ownership/permission 和 Web assets，并只输出后续步骤。它不生成默认 password。

- 无 Web setup wizard；
- 不创建默认密码；
- password 只从 TTY 或安全 stdin/fd 读取，不允许 `--password`；
- 无 Owner 时 liveness 可成功，readiness 为 `setup_required`；
- 未初始化状态不允许 Agent Enrollment。

### 12.3 Human Session

使用 DB-backed opaque Session，不使用 JWT。

Cookie：

```text
__Host-platpulse_session
Secure; HttpOnly; SameSite=Lax; Path=/; no Domain
```

- token 至少 256-bit；
- Server 只保存 keyed digest；
- 登录后轮换 ID；
- idle timeout 默认 12h，absolute lifetime 7d；
- 活动时间限频更新；
- password/role/disabled 变化撤销相关 Session；
- Owner 可查看 Session 创建时间、最近活动、粗粒度客户端信息，并区分当前/其他 Session；
- 用户可撤销自己的其他 Session，Owner 可撤销任意 Session；“保留当前”和“全部撤销”必须是不同操作；
- 不保存完整长期 User-Agent 或原始 IP；
- Server restart 不使全部 Session 失效。

### 12.4 CSRF

认证后，WebUI 通过 `GET /api/public/v1/session` 获取非敏感 Session projection 和当前 synchronizer CSRF token。Admin mutation 同时验证：

1. valid Session；
2. `X-CSRF-Token` 与当前 Session 匹配；
3. exact configured Origin；
4. allowed Content-Type；
5. Owner role。

- 登录请求没有既有 Session，使用严格 Origin 校验和独立 rate limit；
- Session ID 轮换时 CSRF token 同时轮换；
- Public read 不需要 CSRF；
- Agent API 不使用 Cookie；
- GET/HEAD 无 mutation；
- 不接受 query mutation；
- failure code 统一为 `csrf_validation_failed`，不泄露具体失败项。

### 12.5 Token digest 与 Pepper

```text
Human password → Argon2id
high-entropy token → HMAC-SHA-256(server_pepper, full_token)
```

Agent、Session、Enrollment、Recovery token 采用：

```text
pp_<kind>_<token_id>_<secret>
```

- 先按 non-sensitive token ID 查 row，再 constant-time 比较 digest；
- pepper 独立 secret file，仅 Server user 可读；
- 不存 DB、不显示 WebUI；
- pepper 丢失：Session/Agent/one-time token 失效，用户密码和历史仍保留；
- pepper rotation 只通过明确 CLI 流程。

### 12.6 Agent Credential

- TLS 上的 Bearer token；
- 不能访问 Human/Public/Admin API；
- Human Session 不能提交 AgentReport；
- 支持 overlap rotation；
- revoke 立即生效；
- token 不进入 URL、log、error；
- 非 loopback plaintext Agent auth 被拒绝。

---

## 13. HTTP API 与 SSE

### 13.1 Route group

```text
/api/public/v1/*   Home 固定 Public Projection
/api/admin/v1/*    Owner 管理和完整运维数据
/api/agent/v1/*    Enrollment / Recovery / AgentReport
```

- `public` 表示 DTO 脱敏，不表示一定匿名；
- Guest disabled 时 Public API 要求 Viewer/Owner Session；
- Admin 只接受 Owner Session；
- Agent API 只接受对应 Agent credential；
- 三组使用独立 middleware 和 DTO；
- Public/Admin filtering 在 Server query 层执行；
- Admin DTO 不通过运行时删字段复用为 Public DTO。

### 13.2 Public Projection

可显示：

- Node display name、label、Network；
- health reason、freshness；
- current head、block time/interval、transaction count；
- sync/consensus summary；
- process running；
- CPU/memory/disk percentage；
- Peer count、direction/type/country aggregate；
- 显式关联且允许公开的 Validator summary。

永不显示：

- RPC Endpoint；
- hostname、Host IP、raw Peer IP；
- Agent ID/credential/Enrollment Token；
- disk mount path、Node data directory；
- raw Collector error/stack；
- Server internal config 和精确 Host capacity；
- 默认的软件版本；
- Node key、enode/ENR。

Node Visibility 必须在 Public list/detail/history/export/SSE 全部一致过滤。

### 13.3 Browser API wire

- fields `camelCase`；
- UUID/hash/address/key 为 string；
- 可能超过 JS safe integer 的 height、cumulative tx、amount 使用 decimal string；
- bounded count/duration 可 number；
- timestamp RFC 3339；
- chart `null` gap 与 `0` 分开；
- amount 使用最小单位 integer string，加 display decimals；
- percentage/rate 有明确单位。

统一 error：

```json
{
  "error": {
    "code": "node_ownership_mismatch",
    "message": "The node belongs to another agent.",
    "requestId": "0195...",
    "fields": []
  }
}
```

客户端只依赖 code；message 不泄露 SQL、RPC URL、credential 或 stack。

### 13.4 OpenAPI

- Server route DTO 生成 OpenAPI 3；
- Public/Admin 独立 tag；
- Agent API 可在 schema 中，但不生成到 browser client；
- generated TS 放 `platpulse-web/src/api/generated/`；
- CI 重生成并检查无 diff；
- frontend 业务代码不复制 DTO interface；
- schema 明确 nullable/omitted、enum、timestamp、pagination、error；
- AgentReport contract 仍由 `platpulse-core` 负责。

### 13.5 SSE

```text
/api/public/v1/events
/api/admin/v1/events
```

只发送 versioned invalidation：

```json
{
  "eventId": "...",
  "resource": "node",
  "resourceId": "...",
  "revision": 42
}
```

- REST 是权威状态；
- browser 收到 event 后 invalidate query 并 refetch；
- Public/Admin SSE 分开；
- Public event 不引用 private Node；
- Server 为长连接绑定 Human Session generation/role 或 Guest access generation；Session revoke、user disable/role change 时主动关闭对应 Human stream；anonymous Home 关闭时主动关闭所有 Guest stream；
- Node 从 public 改为 private 时，不发送包含该 Node ID 的 Public event，而发送 collection-level invalidation 或 `reset`，让已打开页面清除缓存并重新经过权限查询；
- EventSource 重连必须重新执行完整授权，不能沿用旧连接决定；
- 支持 `Last-Event-ID`；
- buffer miss 时发送 `reset`；
- Server 合并高频 invalidation；
- keepalive comment；
- SSE 失败不影响 REST；
- 不用 WebSocket，不发送完整 current/history payload。

---

## 14. WebUI

### 14.1 技术栈与打包

```text
React
TypeScript strict
Vite
React Router
TanStack Query
native EventSource
native fetch
```

- 单 SPA build，Home/Admin 使用独立 layout；
- 不用 SSR/Next.js、Rust/WASM、Redux；
- Server state 由 TanStack Query 管理；
- filter/time range/Network/Node 写入 URL；
- transient UI state 留在 component；
- production 只运行 `platpulse-server`；
- Web assets 作为 release bundle 安装到 `/usr/share/platpulse/web/`；
- 不编译进 Rust binary；
- `/api/*` 不进入 SPA fallback；
- hashed assets immutable cache，`index.html` no-cache；
- Web assets 缺失时 API 进程仍可启动，但 `/health/ready` 返回明确的 `web_assets_missing` 非就绪状态；
- Vite dev server proxy API；
- 不支持运行时自定义 script/theme injection。

### 14.2 Home 信息架构

```text
Home
├── All Networks
├── Network Overview
│   └── Node list/cards
└── Node Detail
    ├── Health and freshness
    ├── Block and transactions
    ├── Sync and consensus
    ├── Process summary
    ├── Sanitized Host percentages
    ├── Peer summaries
    └── Validator summary
```

Home 围绕 Network → Node，而不是 Agent。Agent/Host topology 只在 Admin 中管理。

### 14.3 Admin 信息架构

v1 Admin 包含：

- Server/DB/background worker health；
- Agent Enrollment、Recovery、rotation、revoke；
- Agent/Node current state 和 Collector diagnostics；
- Node display name、label、visibility；
- retire/reactivate/transfer；
- Owner/Viewer/Session；
- anonymous Home；
- timezone/retention 等有限 settings；
- spool、clock skew、protocol/version diagnostics；
- Audit Log；
- Alert/notification/Silence/Maintenance（Phase 2）；
- Network/Validator/Geo diagnostics（对应阶段）。

不包含 Endpoint 编辑、remote command、restart、upgrade、Docker control 或 plaintext credential retrieval。Node Purge v1 只通过 CLI。

### 14.4 Observation UI

每个 component 可显示：

```text
Current state
Last successful value
Observed at
Received at
Stale age
Current error
Capability state
```

- Error 仍显示 last-good；
- Unknown 不显示 0；
- authoritative empty 有明确 empty state；
- History Gap 在图表中断线，不补 0；
- Resyncing 同时显示 current head、historical high-water mark、Network reference 和 progress；
- Network Identity Mismatch 使用阻断性说明；
- 状态使用文字、icon 和 color，不能只靠颜色；
- relative time 可查看 absolute UTC/selected timezone；
- SSE 更新不得自动改变排序、展开项、scroll 或用户输入。

### 14.5 响应式与移动端要求

Home 和 Admin 从 Phase 1 起都必须支持移动端，不接受“桌面页面缩小后勉强可用”。

#### Breakpoint 原则

不依赖固定设备型号，以内容断点为准。至少验证：

```text
360 × 800   小屏手机
390 × 844   常见手机
768 × 1024  平板/窄屏
1280+       桌面
```

#### 导航

- Desktop：持久侧栏/顶部 context；
- Mobile：可访问的 drawer 或 bottom navigation；
- 当前 Network/Node context 始终可见；
- drawer 打开时正确管理 focus trap、Escape 和 body scroll；
- 不把关键操作藏在 hover-only UI；
- browser back/forward 保持 filter 和 detail context。

#### 列表与表格

- 桌面高密度 table；
- 窄屏转换为信息优先级明确的 cards/rows；
- 关键字段先展示：状态、Node、head/sync、freshness、主要 reason；
- 次要字段放 expandable details，不做横向滚动作为主要交互；
- 必须横向滚动的审计/原始诊断表要提供 sticky first column、scroll hint 和可替代 detail view；
- sort/filter 控件具有可访问 label 和足够 touch target。

#### 图表

- 图表容器响应式；
- 手机上减少 tick/legend 密度，但不删除数据语义；
- 支持触摸 tooltip，不依赖 hover；
- 提供文字 summary 或数据表；
- History Gap、Unknown、Stale 在小屏仍可辨认；
- 不使用自动播放或持续动画；
- `prefers-reduced-motion` 关闭非必要 animation。

#### Admin mutation

- form 单列布局；
- label 不仅使用 placeholder；
- destructive confirmation 不依赖小型 modal 内复杂表格；
- mobile keyboard 不遮挡当前 field/error/action；
- sticky action bar 不覆盖内容；
- credential 一次性展示支持安全复制，但不自动暴露到 screenshot-friendly query/URL；
- Transfer、revoke、password reset 等操作不做 optimistic update。

#### Accessibility

- WCAG AA contrast；
- 全键盘支持；
- 44×44 CSS px 级别 touch target 作为目标；
- semantic heading/table/form；
- focus visible；
- status live region 只播报重要变化，避免 SSE 高频打扰；
- landscape/portrait 均可操作；
- 200% zoom 不丢失功能。

### 14.6 REST/SSE 客户端流程

```text
route open
  → REST query
  → connect Public/Admin EventSource
  → invalidation
  → invalidate exact query key
  → REST refetch
```

- response 带 revision；旧 response 不覆盖新 revision；
- mutation success 立即 invalidate，不等待 SSE；
- hidden tab 可降低非关键 refetch；
- realtime connection status 可见但不遮挡内容。

---

## 15. Peer 与 Geo（Phase 3）

### 15.1 Typed Peer Snapshot

Agent 从 `admin_peers` 提取：

```text
peer_id
remote_ip
direction = Inbound | Outbound
trusted
static_peer
consensus_peer
client_name
caps
cbft_protocol_version?
cbft_highest_qc_block?
cbft_locked_block?
cbft_commit_block?
```

不上传 `localAddress`、完整 enode/ENR 或 raw protocols JSON。remoteAddress 只接受标准库可解析的 literal IPv4/IPv6。string/list 均有上限。

### 15.2 Snapshot 语义

每 Node 独立：

- `Ok + list`：事务替换 current set；
- `Ok + []`：权威清空；
- `Error`：保留 last-good + stale/error；
- `Unsupported`：能力状态，不触发 failure Alert；
- snapshot 内按 peer ID 唯一；
- peer count 是 Peer 数，不是 unique IP；
- 只有连续两次成功 snapshot 的差异形成 presence interval；
- Collector Error 不能把全部 Peer 误判 disconnect。

### 15.3 Raw IP 隐私

- raw IP 只经 Agent TLS 上报；
- 只进入 current Peer table 和 Geo cache；
- Public/Admin v1 均不提供 raw IP list/search；
- log mask，Audit 不包含；
- private/loopback/link-local/multicast/unspecified 不发给外部 provider；
- long-term aggregate 不包含 raw IP；
- Geo cache 使用 canonical IP，并记录 `last_referenced_at`；最后一个 current Peer 引用消失后立即删除 raw IP cache row，或由 cleanup 在 24 小时硬上限内删除；
- presence interval 不保存 raw IP，长期 country aggregate 只保存 country/count；
- 即使 current 引用仍存在，raw IP cache row 最长 30 天必须重建/刷新；不能因 MMDB reload 无限延长旧 raw IP retention；
- SQLite 文件、snapshot 和 backup 必须依赖严格 OS ownership/permission 保护。

### 15.4 Geo Database

- Server 只读取 operator-provided `GeoLite2-Country.mmdb`；
- PlatPulse 不 bundle、download 或保存 MaxMind credential；
- 推荐部署方使用 MaxMind 官方 `geoipupdate`；PlatPulse 只提供该官方工具/官方 image 的部署示例或 sidecar 配置，不发行自己的 Geo downloader；
- 未配置为 `Disabled`，Peer count 正常；
- 只解析 country code，不保存 city、coordinate、ASN；
- map 使用 country static centroid；
- reload failure 保留上一个成功 DB；单个 IP lookup 失败也保留该 IP 的最后成功 country，直到 cache retention 到期；
- Admin 显示 build epoch、digest、load status；
- Home/About 显示 MaxMind attribution；
- 发行物不包含 MMDB，避免未经许可再分发；
- 启用 Geo 的部署方负责 MaxMind attribution、及时更新，并在新版本发布后 30 天内停止使用和销毁旧版；
- 获取与许可细节见 `docs/research/geolite2-country-acquisition.md`。

### 15.5 Peer history

默认：

```text
current Peer               current only
presence interval          30 days
5-minute aggregate         90 days
1-hour aggregate           long-term
```

aggregate：total、inbound/outbound、trusted/static/consensus、known/unknown country、country distribution、churn、CBFT lag summary。

---

## 16. Validator Analytics（Phase 4）

### 16.1 Validator 与 Node

Validator 是 Network 级实体：

```text
Validator
- network_id
- validator_node_id

NodeValidatorLink
- node_id
- validator_id
- valid_from
- valid_until
- Primary | Standby | Observer
```

- PlatPulse Node、P2P Node ID、Validator ID 独立；
- 一个 Node 同时最多一个 active link；
- 一个 Validator 可关联多个 primary/standby Node；
- link 由 Owner 明确配置/确认；
- consensus membership 不自动创建 link；
- key rotation/Transfer 不重写历史 link。

### 16.2 Provider

Validator Provider 在 Server 按 Network 集中采集：

- PlatON Explorer 是一个 adapter，不是领域模型；
- 相同 Validator 只请求一次；
- Agent 不请求公共 Explorer；
- provider response 必须区分 `NotFound`、权威空集合、`Error` 和 `Unsupported`；
- provider failure 不改变 Node health；
- last-good 保留并显示 source、provider timestamp、Server received time 和 freshness；
- 未配置为 `Disabled`；
- adapter API 变化为 `Unsupported/Error`，不清空 current；
- future on-chain provider 不改变 browser DTO。

### 16.3 数值与历史

禁止 `f64` 保存 reward/stake/rate：

- amount：最小单位 integer/decimal string；
- percentage：numerator/denominator 或 decimal string；
- rank/count/epoch：integer；
- UI 才格式化 display string；
- cumulative counter decrease 标记 Counter Reset or Correction，不 clamp 为 0。

默认：

```text
current refresh      60s
ranking history      on confirmed value change
daily snapshot       configured IANA timezone
monthly aggregate    calendar month
```

排名变化连续两次成功观察后确认。缺少 baseline 为 Unknown；同 Validator 不因关联多个 Node 重复统计。

---

## 17. Alert 与 Notification（Phase 2）

### 17.1 Typed Alert Rule

首批：

```text
agent.offline
node.rpc_unreachable
node.head_subscription_disconnected
node.observation_stale
node.process_not_running
node.block_stalled
node.sync_lag
node.network_identity_mismatch
node.consensus_stalled
host.disk_pressure
host.memory_pressure
```

后续：

```text
node.peer_count_drop
node.consensus_peer_lag
validator.ranking_changed
validator.counter_reset
geo.database_stale
agent.spool_pressure
```

每条支持 enable、severity、threshold、`for`、recovery threshold/hysteresis、global default、Network override、Node override。subject 类型明确为 Agent、Host、Node、Network、Validator 或 Server。Agent 只报告事实；Server 评估。无 script/SQL/DSL/network action。

### 17.2 状态机

Evaluation：

```text
Normal → Pending → Firing → Recovering → Normal
```

Incident：

```text
Open → Resolved
```

- 持续超过 `for` 才 Open；
- 持续超过 `recovery_for` 才 Resolved；
- 同 `(rule_key, subject_key)` 最多一个 Open；
- 重复触发创建新 incident sequence；
- Server restart 恢复 timer/state；
- Incident 保存 rule version、threshold 和 evidence。

### 17.3 Unknown/Stale

评估输入：

```text
Known(value)
Unknown(reason)
Stale(last_value, age)
```

- 只有 Known 做 threshold comparison；
- Unknown 不代表 false/recovered；
- Open threshold Incident 遇到 Unknown 时保持 Open，并标记 EvaluationUnavailable；
- fresh known recovery 才 Resolve；
- Unsupported 默认不告警；Disabled 仅 Admin 展示；NeverObserved 不为 Healthy；
- Resyncing 不用普通 block-stalled rule。

### 17.4 Transactional Outbox

```text
Incident transition
  └─ same SQLite transaction → Notification Event + Outbox
                                  └─ worker → Telegram / future Webhook
```

- per channel/destination delivery；
- stable idempotency key；
- exponential backoff、Retry-After、DeadLetter、manual retry；
- restart 后继续；
- provider token 存 secret file，不返回 WebUI；
- Owner 可发送 test notification，操作和结果进入 Audit；
- log 不记录 token、完整 provider response 或敏感 destination；
- external delivery 为 at-least-once，不承诺 exactly-once。

### 17.5 Silence 与 Maintenance

- Silence 和 Maintenance 都保存 scope/matcher、starts/ends、reason、created_by；
- Silence 只抑制 delivery，不停止 evaluation/Incident；
- Maintenance 让预期 offline/process/RPC Incident 标记 suppressed；
- Window 结束后按当前事实重评；
- 已恢复的历史不补发；仍 firing 的发送一次当前通知；
- 都必须有 expires；
- v1 一次性 window，无 cron；
- quiet hours 属于 channel delivery policy；Daily Summary 是否绕过 quiet hours 由 channel 配置明确决定。

---

## 18. Server 配置、CLI 与 Secret

### 18.1 文件布局

```text
/etc/platpulse/server.toml
/etc/platpulse/secrets/server-pepper
/etc/platpulse/secrets/tls-key
/etc/platpulse/secrets/telegram-token
/var/lib/platpulse/platpulse.db
/usr/share/platpulse/web/
```

TOML 保存 bind/base URL、TLS/proxy、DB path、web root、secret file path、Geo path、retention/cadence。它不保存 plaintext password、Agent credential、Telegram token 内容或 MaxMind License Key。

所有 pepper、TLS private key、notification token、Agent credential 等 secret file 必须由对应专用 OS user 拥有；启动时拒绝 group/world-readable regular file，并使用 no-follow/open-then-stat 策略防止意外 symlink 置换。`doctor` 和 backup 不跟随配置目录外的非预期 symlink，也不输出 secret 内容。

### 18.2 CLI

```text
platpulse-server init
platpulse-server check-config
platpulse-server owner create
platpulse-server owner reset-password
platpulse-server agent create-recovery-token
platpulse-server agent revoke-credentials
platpulse-server node purge
platpulse-server sessions revoke-all
platpulse-server doctor
platpulse-server backup
platpulse-server restore
```

- CLI 复用 Server domain/storage module，不复制 SQL；
- mutation 使用 transaction 并写 Audit，actor=`local-cli`；
- 可在线运行的 CLI 与 Server 遵守同一 DB invariant；restore 等要求独占的命令检测到运行中 Server 时拒绝执行；
- destructive command 要求完整 ID + confirm phrase；
- automation 需显式 `--yes`；
- output 脱敏；doctor 不输出 secret 内容。

---

## 19. TLS 与部署

### 19.1 Local development

```text
bind = 127.0.0.1:8080
HTTP allowed
explicit development mode may use a non-`__Host-` development cookie without Secure
```

生产 cookie 的 `__Host-` 前缀与 `Secure` 属性不可拆开；只有显式 development mode 可以改用单独命名的开发 cookie，避免产生不合法或误导性的 `__Host-` cookie。

### 19.2 Trusted reverse proxy

```text
Client/Agent ─ HTTPS ─ Caddy/nginx/Traefik ─ private HTTP ─ Server
```

- 显式 trusted proxy CIDR；
- 只信任来自这些地址的 Forwarded/X-Forwarded；
- forwarded scheme 必须 `https`；
- `public_base_url` 显式配置；
- Human Cookie 始终 Secure；
- Agent auth 只接受 actual TLS 或 trusted HTTPS proxy。

### 19.3 Native Rustls

- certificate chain + private key file；
- 不自动 ACME；
- certificate reload 首版通过受控 restart；
- 无 CA management UI。

### 19.4 启动安全

- 非 loopback bind 且无 TLS/trusted proxy 时拒绝启动；
- CORS 默认关闭；
- HTTPS 确认后启用 HSTS；
- CSP 禁止 inline/third-party script；
- login、Enrollment、Recovery、AgentReport 独立 rate limit；
- credential 不进入 URL。

---

## 20. 备份、恢复、健康与升级

### 20.1 Backup

```bash
platpulse-server backup --output /backup/platpulse-2026-08-12.db
```

- SQLite Online Backup API；
- 不复制 live `.db/-wal/-shm`；
- 生成 snapshot、manifest、SHA-256、schema version、Server version、timestamp、data range；
- temp + fsync + atomic rename；
- 默认不覆盖；
- 不内置 S3/WebDAV/scheduler；
- backup output 不写回 Server state directory；
- Admin 不提供 DB 下载，只显示最近一次成功 backup 的时间、摘要和目标标识。

Backup 不包含 pepper、TLS key、Telegram token、MaxMind credential 或 Agent local state。运维必须把 DB 与匹配 secret 作为恢复集合保护。

### 20.2 Restore/corruption

Restore 仅离线：checksum、`integrity_check`、schema compatibility、拒绝更高且不受支持的 schema、current DB safety copy、atomic replacement；首次启动再执行正常 forward migration。恢复后 current observation 自然 stale，不修改 Agent epoch/receipt。若配套 pepper 不匹配，Human Session 和 Agent Credential 失效，Owner password 与历史数据仍可用，Agent 逐一 Recovery。

启动执行轻量 `quick_check`。corrupt 时拒绝 writable/readiness，不自动删除、改名或创建空 DB；doctor 给出人工恢复步骤。

### 20.3 Health

```text
/health/live
/health/ready
```

- live 只说明 event loop 响应，不返回 version、DB path、Agent count 等内部信息；
- ready 检查 migration、SQLite、Owner、Web assets、关键 worker 和非 shutdown/corrupt 状态；
- Geo/Validator/Telegram error、单 Agent offline 不使整个 Server unready，只显示 degraded component。

### 20.4 Logging/Metrics

- production structured JSON tracing；
- request ID/span；
- ingestion 只记录 Agent ID、report ID、result、duration，不记录 body；
- RPC URL、IP、token、password、cookie、CSRF、private key 脱敏；
- log rotation 交给 journald/runtime；
- optional Prometheus metrics 独立 loopback/management listener；
- label 低基数，不含 raw Node/Peer/user ID；
- 只暴露 Server internal operational metrics。

### 20.5 Upgrade

- SemVer；
- Server first、Agent later；
- 先 DB backup；
- migration forward-only；
- downgrade 恢复旧 backup；
- destructive migration 分 release expand/backfill/switch/contract；
- large backfill 分批；
- migration 文件带 schema version/checksum；
- Agent Store migration failure 时不采集、不覆盖旧 DB；
- Server migration failure 时不监听业务 port；
- Web assets 与 Server API 必须同一 release bundle；
- release notes 必须说明 schema migration、Agent protocol、config change、rollback 和第三方许可变化。

---

## 21. Workspace 与依赖

### 21.1 目录

```text
Cargo.toml
crates/
├── platpulse-core/
├── platpulse-agent/
└── platpulse-server/
platpulse-web/
```

不预建 `platpulse-db/api/auth/alerts/storage/collectors` 等浅 crate。

### 21.2 Crate 职责

`platpulse-core`：AgentReport v1、wire identity、Observation Envelope、Block Summary、History Gap、receipt/error code 和 wire validation。I/O-free；不依赖 Axum/SQLx/Alloy；不包含 Server row 或 Public/Admin DTO。

`platpulse-agent`：config/CLI、Enrollment/Recovery client、RPC/Host/Process collector、Node Supervisor、Report assembler、AgentStore、sender。

`platpulse-server`：HTTP/SSE、auth、Report Ingestion、SQLite projection、Network/Validator/Geo、Alert/outbox、static Web assets。

每个 binary crate 使用薄 `main.rs` + 可测试 `lib.rs`。

### 21.3 基础依赖

```text
Runtime             Tokio
HTTP Server         Axum + Tower
HTTP Client         Reqwest
TLS                 Rustls
Serialization       Serde + serde_json
Database            SQLx SQLite
PlatON RPC          Alloy
Host metrics        sysinfo + Linux /proc helpers
CLI/config           Clap + TOML
Logging             tracing
OpenAPI              utoipa
Passwords           Argon2id
Random secrets       OS CSPRNG
Identifiers          UUID
Time                 time
GeoLite MMDB         maxminddb
```

- HTTP client 统一 Rustls；
- Alloy 只在 Agent；
- 不引入 ORM、gRPC、Kafka、NATS、Redis、workflow engine 或全局 DI container；
- dependency 由明确 constructor 注入；
- 只有真实外部系统建立 adapter seam：PlatON RPC、Validator Provider、Notification Channel；
- SQLite 用真实临时 DB 测试，不建立 hypothetical repository abstraction。

---

## 22. 分阶段交付

### Phase 0 — Workspace 与协议基础

- workspace；
- core protocol；
- Observation Envelope；
- wire fixtures；
- Agent/Server migration；
- OpenAPI/Web skeleton；
- CI。

验收：协议、migration、最小 binary 和 Web build 通过，不宣称产品可用。

### Phase 1 — 首个纵向切片

从一开始支持：

- 一个 Agent 多 Node；
- 每 Node 一个 IPC/WS/WSS Endpoint；
- Enrollment；
- Host/optional Process；
- RPC/sync/consensus current；
- per-Node Head Subscription；
- get block by hash + transaction count；
- AgentStore/spool；
- Report Ingestion/Receipt；
- 最小 Network Registry + CLI bootstrap + identity validation；
- SQLite current + Block history；
- Owner/Viewer login；
- 默认私有 Home；
- Network → Node overview/detail；
- Admin Agent/Node diagnostics；
- Desktop/tablet/mobile responsive UI。

不包含 Peer/Geo、Validator Provider、Alert Notification、Transfer UI、高级 aggregation。

核心 acceptance：

1. 一个 Agent 监控两个 Node；
2. Node A RPC failure 不影响 Node B；
3. Server offline 时 spool；恢复后 oldest-first 且无重复；
4. partial receipt 不误 retire Node、不永久重试同 report，retryable/terminal sample 都有明确处置；
5. old backlog → new boot 转换和 duplicate boot 拒绝符合状态机；
6. Agent/Server restart 后 identity/sequence/receipt 连续；
7. Node resync 时 current head 下降，旧历史不重复累计，显式 open gap 仍可合法回补；
8. private Node 不出现在任何 Public API/history/SSE，public→private 会清除已打开客户端缓存；
9. 360px 手机可完成 Home 浏览、Node Detail、登录和核心 Admin diagnostics；
10. keyboard、200% zoom 和 reduced motion 基本验收通过。

### Phase 2 — 运维闭环

Recovery/rotation、Node lifecycle/Transfer、Network Registry 的完整 Admin lifecycle、multi-user/session、Audit、Alert/Telegram、Silence/Maintenance、retention aggregate、backup/restore/doctor、Admin 完整闭环。Phase 1 已包含 Network Registry 的存储、验证和 CLI bootstrap。

### Phase 3 — Peer/Geo

Typed Peer Snapshot、presence、country aggregate、operator MMDB、Home Peer insight、Admin diagnostics、raw IP privacy。

### Phase 4 — Validator

Provider seam、Explorer adapter、NodeValidatorLink、ranking/reward/block metrics、日/月 aggregate、Validator Alert。

### Phase 5 — Hardening

Native TLS、internal metrics、packages、load/fault/soak、migration rehearsal、security review。

每 Phase 可独立部署；不得为未来阶段加入空 trait、空 crate、无消费者 schema。

---

## 23. 测试与发布门槛

### 23.1 Core

- historical JSON fixtures；
- round-trip；
- required/optional/unknown；
- revision property；
- timestamp/unit；
- attribution evidence。

### 23.2 Agent/AgentStore

真实临时 SQLite：

- plan/ack/retry；
- transaction crash boundaries；
- overflow → History Gap；
- epoch/boot/sequence；
- corruption；
- multi-Node isolation；
- bounded queue overflow；
- graceful shutdown。

### 23.3 RPC Adapter

脚本化本地 JSON-RPC/PubSub fake：

- `eth_subscribe("newHeads")`；
- `platon_getBlockByHash`；
- hash/number mismatch；
- disconnect/reconnect；
- bounded backfill；
- resync replay；
- method not found；
- malformed/oversized response；
- admin/debug capability；
- timestamp units。

非每 PR 的真实 PlatON-Go smoke test固定 verified develop commit，验证 namespace registration/subscription compatibility。更新基线必须人工复核。

### 23.4 Server

通过 Report Ingestion interface + real SQLite：

- exact receipt idempotency；
- hash conflict/old sequence/duplicate Agent；
- old backlog、Closing/DrainedPrevious/RecoveredAfterStale 和并发 boot race；
- partial Node rejection + Inventory whole disposition + sample retryable/terminal disposition；
- retire/reactivate/Transfer；
- unknown Network key/Network mismatch；
- current/last-good merge；
- replay high-water mark、open-gap backfill、coverage closure、raw retention 后 dedup/divergence；
- Alert/outbox transaction；
- retention 不降低高水位。

### 23.5 Web

- TypeScript compile/lint/unit；
- generated API no diff；
- component accessibility；
- Playwright 固定 projects：
  - `phone-360-touch`：360×800，`hasTouch=true`；
  - `phone-390-touch`：390×844，`hasTouch=true`；
  - `tablet-768-touch`：768×1024，`hasTouch=true`；
  - `desktop-1280`：1280×800，keyboard/mouse；
- 上述 projects 覆盖：
  - Owner/Viewer/Guest；
  - private Node filtering；
  - CSRF；
  - stale/error/unknown；
  - SSE invalidation/reset、Session revoke、role change、Guest disable、public→private runtime transition；
  - responsive navigation/table-to-card/chart；
  - touch interaction；
  - keyboard/focus；
  - portrait/landscape；
  - 200% zoom。

### 23.6 安全 matrix

必须覆盖：

- Public DTO 不含 Admin-only 字段；
- Guest/Viewer/Owner route matrix；
- Agent/Human credential isolation；
- retired/private filtering across list/detail/history/SSE；
- CSRF/Origin/session fixation/revocation；
- Enrollment/Recovery single-use；
- token/RPC URL/raw IP redaction；
- body/string/list limits；
- malformed JSON；
- SQL injection；
- CSP/proxy spoof/open redirect/path traversal；
- backup/doctor secret redaction。

### 23.7 CI/Release

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
cargo audit
frontend lint/typecheck/test/build
OpenAPI generation check
Playwright desktop/mobile security smoke
```

Release 生成 SBOM、checksum，并记录 Rust/npm audit。首版发布 Agent/Server Linux x86_64/aarch64 binary、native tar/deb/rpm bundle、Web assets、Server OCI image、systemd units、Caddy 示例、Server Compose、MaxMind 官方 `geoipupdate`/官方 image 的 optional sidecar 示例、config reference 和 backup timer 示例。容器 non-root，state/web/secret/Geo 分卷挂载。若 release 尚未提供 artifact signing，不得把 unsigned artifact 描述为已验证供应链。

---

## 24. 关键风险与待实现时复核项

1. **PlatON capability 漂移**：运行节点可能不是 pinned develop；必须 probe 并记录 client build。
2. **Seal recovery 版本敏感**：Header fork/field 改变时 attribution 必须退回 Unknown，不能错误匹配。
3. **历史 Node key**：仅有当前 enode key 时不能可靠归因旧 block；需要有效时间范围。
4. **admin/debug 暴露**：读取需求不能成为公网启用整个 namespace 的理由。
5. **SQLite 增长**：逐区块、Peer、Validator retention 必须配合 benchmark 和分批 job。
6. **跨 Host 时钟**：无可靠 NTP 时不提供亚秒传播排名。
7. **GeoLite 许可**：发行前复核 MaxMind 当时 EULA；PlatPulse 不分发 MMDB。
8. **移动端复杂 Admin**：每个新 Admin workflow 都必须有 mobile interaction design 和 Playwright coverage，不能默认桌面表格可缩放解决。
9. **外部 Provider**：Explorer/Telegram failure 不得扩散为 Node health failure。
10. **Phase 边界**：不得用“以后需要”作为添加空 abstraction、crate 或 schema 的理由。

---

## 25. 完成定义

本文设计完成的实现必须满足：

- Node 级数据归属和故障隔离无例外；
- current state、last-good、history、high-water mark、gap 和 error 语义可从 API/UI 直接辨认；
- Agent/Server crash、retry、restart 不造成静默历史重复或 current rollback；
- Public/Admin/Agent 三个信任边界无法混用；
- Home/Admin 在桌面、平板和手机上均可完成对应核心任务；
- TUI、Endpoint failover、remote control 和完整链索引未重新混入范围；
- 每一阶段都以可运行纵向行为和自动化验收结束，而不是只完成内部模块。
