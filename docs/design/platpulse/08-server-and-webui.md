本文件属于 [PlatPulse 产品与技术设计](../platpulse.md)：§8–§9。章节保留原编号，引用写作 `design §X.Y`。

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
GET  /api/public/v1/reward-wallet
GET  /api/public/v1/nodes/{node_id}
GET  /api/public/v1/nodes/{node_id}/history
GET  /api/public/v1/nodes/{node_id}/history/export
GET  /api/public/v1/nodes/{node_id}/metrics?window=live|1h|6h|24h|7d
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

这些 family 表示当前实际 route 集合；每个 operation 的 GET/POST/PUT/PATCH/DELETE 方法、参数和响应以源 handler/OpenAPI operation 为准。备份创建与恢复是**离线操作**（[ADR 0008](../../adr/0008-offline-server-backup.md)）：服务进程不创建 backup artifact，因此 `/api/admin/v1/backups` 只保留列表/详情/verify，`POST /api/admin/v1/backups` 随 `backup_create` Operation 一起移除。Session 撤销是 `POST /api/admin/v1/sessions/{session_id}/revoke`，不是 DELETE。运行时 handlers 还可能返回 OpenAPI 未列出的 typed `ApiErrorBody`（特别是 503/500），客户端必须把任何非 2xx 当作错误处理。Admin 路由组的存储失败统一回答 `503`：唯一或检查约束违反为 `storage_conflict`（message「Server storage rejected the change」），其余存储故障为 `unavailable`（message「Server database is unavailable」）；失败的读取不会伪装成 404/400，处理器同时把该失败以 `error` 记入请求日志并锁存数据库损坏状态。 规则编辑与 Network/Node 继承覆盖的保存是版本安全的（issue #204）：`PUT /api/admin/v1/alerts/rules/{rule_key}` 与 `PUT /api/admin/v1/alerts/rules/{rule_key}/overrides` 都要求请求体携带 `expectedVersion`；与当前 composed version 不匹配时返回 `409` 加 `alert_rule_version_conflict` 的 `ApiErrorBody`，并在同一事务中不写入任何规则或覆盖变更。composed version 是整个规则聚合（baseline 加全部 Network/Node 覆盖）的并发令牌：baseline 编辑与覆盖 upsert 成功时都会追加一条不可变的历史记录并把 `alert_rules.version` 推进到新版本；`DELETE /api/admin/v1/alerts/rules/{rule_key}/overrides/{scope_kind}/{scope_value}` 只删除显式、可重建的覆盖、不带版本前置条件，但删除同样推进 composed version，持有旧版本的写入者必须重载后重审。

通知测试与命令对账由 issue #206 交付：`POST /api/admin/v1/notifications/channels/{channel_id}/test` 与 `POST /api/admin/v1/notifications/deliveries/{delivery_id}/retry` 的请求体都必须携带 `requestId`（1..128 字符，空白或超长返回 400 `notification_request_id_invalid`），`GET /api/admin/v1/notifications/requests/{request_id}` 返回该 request id 的持久 Server 命令结果。Server 用 `notification_requests` 台账做**请求级**去重：同一 request id 加同一意图重放时返回已记录的结果且不重发（`deduplicated: true`），同一 id 换意图返回 409 `notification_request_id_conflict`；被拒绝的命令（冷却、未配置、禁用等）不写台账，因此查不到结果。测试命令还受 `[notifications] test_cooldown_seconds`（默认 30，范围 1..3600）约束，命中时返回 429 `test_cooldown_active` 并带 `Retry-After`；冷却时钟只统计其它 request id 已接受的测试命令，并发重复的同一条命令不会因自己刚记录的测试被判为冷却命中，而是按台账回放同一条命令结果；台账保留期由 `dedup_retention_seconds`（默认 86400，范围 60..604800，且不得小于冷却）决定，过期行在读写时按不存在处理。这一层只保证 Server 命令不重复，**不等于**外部通道的 exactly-once 投递：delivery 仍是 at-least-once，重放只保证「同一条命令得到同一个 Server 结果」。retry 只接受 `retry_scheduled`/`failed`/`dead_letter`，`pending`/`in_flight` 返回 409 `delivery_already_queued`，终态或 suppressed 返回 409 `delivery_not_retryable`。

Delivery 的显式安全重试由 issue #207 交付（[WebUI §15.10](../webui/15-agent-node-management.md#1510-explicit-safe-retry-of-a-retryable-delivery-issue-207-delivered)）。重试只接受 `retry_scheduled`/`failed`/`dead_letter`；`pending`/`in_flight` 返回 409 `delivery_already_queued`，`succeeded`/`suppressed`/`cancelled` 等终态返回 409 `delivery_not_retryable`，两种拒绝都不改动该 Delivery。成功重试只把同一行 `notification_deliveries` 重新武装为 `pending`（清空 `next_attempt_at`），不动 `attempt_count`，也不改写 `delivery_attempts`：历史尝试、provider 结果与 last error 原样保留，下一次 worker 轮次按 `attempt_number = attempt_count + 1` 追加新尝试，并且不会新建 Notification Event、Incident 或业务状态迁移。重试后仍在 handoff 前重新检查 Silence、Maintenance Window 与主体删除：期间生效的抑制或删除会抑制或取消这次发送（`suppressed_by_silence:<id>` / `cancelled_subject_deleted`）且不记录新的 provider 尝试，不会盲发。retry 命令与测试命令共用同一 request id 台账：并发重复折叠为一条命令与一条 Audit，换 Delivery 使用同一 id 属于换意图返回 409，进程重启后仍可按 id 查到已记录结果。
运维任务可见性与 Doctor 诊断由 issue #208 交付（[WebUI §15.11](../webui/15-agent-node-management.md#1511-operations-task-visibility-and-doctor-diagnostics-issue-208-delivered)）。`GET /api/admin/v1/operations` 按 `created_at DESC, operation_id DESC` 返回最新一页（`limit` 钳制到 1..200，默认 50），`status` 与 `kind` 分别按 `OPERATION_STATUSES` 与 `OPERATION_KINDS` 校验；`GET /api/admin/v1/operations/{operation_id}` 缺失时返回 404 `operation_not_found`。`POST /api/admin/v1/operations/{operation_id}/cancel` 只记录取消请求：缺失返回 404 `operation_not_found`，非 `queued`/`running` 返回 409 `operation_not_cancellable`，`queued` 直接转为 `cancelled` 并写入 `finished_at`，`running` 记录 `cancel_requested = 1` 但保持 `running` 与空 `finished_at`，直到 worker 观察到；因此「已请求取消」不等于「已经完成」。`POST /api/admin/v1/doctor/run` 只入队一条 `doctor_run` Operation 并返回其 id，`GET /api/admin/v1/doctor` 返回最后一份报告及其 `currentRun`；Doctor 只诊断、不修复。这些是读与命令操作，不施加删除确认。

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

`AttentionItem.kind`、`severity` 与 `subject_kind` 是 typed Server contract，不是浏览器任意字符串。`subject_kind` 限定为 `agent`、`node`、`network`、`settings`。当前 kind 为 `agent_offline`、`agent_spool_fatal`、`agent_spool_overflow`、`agent_report_gap`、`agent_security_event`、`agent_shutdown_incomplete`、`agent_inventory_rejected`、`node_unhealthy`、`node_health_unknown`、`node_resync`、`node_identity_mismatch`。severity 只有 `critical` 与 `warning`：Spool fatal/overflow、security event、Inventory rejection、unhealthy Node 与 Network Identity Mismatch 为 Critical；Agent offline、report gap、incomplete shutdown、unknown Node health 与 resync 为 Warning。新 Agent 在尚无 accepted report 时是 Unknown 而非 Offline；新 Active Node 在 Server-owned first-observation grace period 内仍计为 Unknown（理由为等待首次 accepted observation），grace 只是 Attention 规则，不提前产生 unknown-health Attention；identity mismatch 造成的 Unknown 只产生 `node_identity_mismatch`。Server 按 Critical 优先、权威观察时间与稳定身份排序；WebUI 可以按 Subject 分组展示，但不能丢弃 Item 或重算 severity。

Public 与 Admin Projection 都由 Server 计算 Health，浏览器不得自行重算；两者共用 `node_currency` module 的唯一 Node Health Summary 推导，同一 Node 在两种 DTO 中的 health 与 health_reason 相同。顺序为：Retired → Node Currency（Never Observed 或 Stale 时为 Unknown；只在 stale 证据里看到的失败写进理由，如 observations are stale; last RPC observation failed，绝不判 Unhealthy）→ process 采集失败 → RPC 失败 → identity mismatch 或 identity 采集失败（Unknown）→ Sync、Consensus 失败 → 三者 ok 才 Healthy，其余 Unknown。Node Currency 只看 RPC、Sync、Consensus 的 Server-clock observation instant，窗口固定为 `FRESHNESS_LIMIT_SECONDS`，不读 Owner 告警阈值（`node.observation_stale`、freshness bound）：Public Projection 对所有读者相同，Owner 调整告警不得改变 Home 的 Health；`node.observation_stale` 衡量证据到达（receipt instant），与 Node Currency 无关。告警 head-lag 只被 Stale 阻断，从未观察的组件不阻断。Admin 诊断读取失败返回 500，不以缺失行拼出诊断。Node 级 DTO 以 Node Currency 表达当前性：Public 的 `currency`/`currencyAsOf` 与 Admin 的 `currency`/`currency_as_of` 由同一推导给出，`currency` 取 `current|stale|never_observed`，as-of 为三个 observation instant 中最早者、任一从未观察时为 null；同一 Node 在两种 DTO 中的 currency 相同。

当前 SPA 对未注册的扩展页面使用 Admin fallback；Server/API 可以先于页面实现，但不得把 fallback 误写成可用页面。

---

## 9. WebUI

页面与交互契约的权威文档是 `docs/design/webui.md`；此处只列当前边界和路由事实。Server 扩展 API 存在不代表 `platpulse-web` 已注册对应页面。

### 9.1 Home

只读 Public Projection，All Networks → Node → Node Detail：

- All Networks（Network 筛选与 Active Node 列表/卡片）；
- Node Detail：用户批准的统一展示目标以 [WebUI Node Detail 契约](../webui/11.1-node-detail-and-visual-acceptance.md#node-detail-composition-page-home-node) 为准，不声明实现或验收完成，并取代 Validator 四 KPI／普通节点六 KPI 的分叉布局。保留 1280px shell 与 pinned Emerald：共用无卡片 Header（Node Health 圆点、名称、Sync、经证据确认的 Role、适用 Activity、inline 相对 Last report；下方完整 Node ID 与复制），六 KPI（Head、Sync、Peers、Uptime、Block interval、Transactions / block），同一 Chain state／Process／Host resources、条件 Validator performance、Latest 60 seconds 六图。桌面 KPI 三列两行，与地图约 5:6 分栏；地图沿用 Home 的轨道（低于 xl 保持 2:1 比例、达到 xl 用上游 22rem 固定带），KPI 网格拉伸填满该轨道高度、与地图上下齐平，地图不被压到 KPI 堆叠高度；手机 KPI 两列且地图在其后，角色不改变几何或顺序。仅 currentValidatorStatus=validator 展示紧凑单卡 3×2 performance；stale positive 保留且明确警告；仅 current not_validator 显示 Observer；unknown／无 Link／stale negative 不推断 Role、不展示 performance，身份原因在受影响数值旁就地可见。Process 显示 Collection Successful／Error／Disabled／Unsupported／Starting／Unknown，不从采集结果推断 Running／Stopped；Started 为绝对 UTC，Uptime 仅在 KPI，缺失可选行隐藏。Head lag 仅由当前有效且 high-confidence 输入比较：0 success、1–10 warning、>10 error，负值显示 ahead 而不归零；保留标注的 last-good 绝对高度。Host 三条细进度条和单行上下行速率保持共享 Host 口径。六图同序、同高、右上实时值、底部 legend、原 tooltip 与真实样本；CPU／Memory 保留 5／10／25／50／100% 动态轴。保留完整 ID 复制／展开与失败回退、Validator diagnostics、精确奖励值、独立 stale／error 解释和仅改变显示年龄的 Last report 时钟；不填充大量 Unknown、不扩展 Public API、不修改 Home／Admin。固定视口 360／390／768／1280／1440 验收角色切换与移动端，无 Bounded Block History 或历史导出。

Home 顶部为六卡片紧凑概览，顺序为 Active Nodes、Healthy Nodes、Attention、Balance、Cumulative blocks、Cumulative rewards（DOM 顺序即视觉顺序：>=640px 三列两行，手机两列三行）。三项可归因统计跟随当前列表选择：Active 为匹配的 Active Node 数，Healthy 为其中 Healthy 数，Attention 为其余匹配 Node 数（含 Unknown）；列表未被收窄时该选择就是整个 Network 选择。一旦搜索／Health／Validator status 收窄列表，这三项与 Peer 国家地图改为读该列表选择（Node Selection Geo Aggregate，见 [WebUI §15.28](../webui/15.28-home-selection-tiles-search-and-sorts.md#1528-home-readings-over-the-current-list-selection-the-node-selection-geo-aggregate-issue-233-delivered)）；排序只改变 Node 顺序。Balance 卡片显示 Reward Wallet Balance（见 `CONTEXT.md` 与 [WebUI §15.37](../webui/15.35-home-tiles-metric-window-and-animation.md#1537-home-balance-tile-the-reward-wallet-balance-replaces-the-networks-counter-issue-246-delivered)）：固定 Reward Wallet（PlatON 主网 `lat1zyctwgvw3ssagn722cneh6sz92y8j3dp8gqj5h`，Server 源码常量，不进配置、不由 Admin 或 Agent 提供）由 PlatScan 报告的可用余额，不是 Validator Lifetime Rewards，不含锁仓/质押/委托/未领取奖励；它不随 Network 选择或列表筛选变化，由独立的 `GET /api/public/v1/reward-wallet` 读取。Server 每 60 秒后台刷新一次（超时 10 秒，失败等下一周期），失败只记录 attempt 与错误类别（`upstream_unavailable`／`invalid_response`）而不覆盖 last-good 值；距上次成功超过 3 分钟为 `stale`，从未成功为 `unavailable`（显示 `—`，绝不显示 0）；development 模式不启动刷新器。累计两卡片是 Cross-Network Validator Overview：对当前选择内 Server 已按 Network 去重的累计出块/奖励精确数值相加，不重新累加 Node、不跨 Network 合并身份；奖励只是各 Network 原生单位数字相加，不表示同一资产余额或法币估值。两张累计卡片保留整个 Network 选择——这些总额由 Server 按 Network 发布，Node 过滤绝不重新求和——并在列表被收窄时各自就地标注该口径。每张累计卡片提供可访问的 Breakdown 对话框，保留精确值、逐 Network 独立的 eligible/known/stale 分母与计数、未关联 Node 和缺失/部分/last-good 语义，替代独立 totals 区域。整个页面仍以 1280px 为上限；桌面 >=1024px 概览左右约 5:6，地图占较大一侧，使六张卡片保持原四卡 2×2 布局的紧凑高度（44px 标题行内放标题与指标图标或 Breakdown 控件，数值在其下，不设页脚行），左侧三列两行，六卡在地图带内贴底对齐，使概览到下方网络分组的间距与网络分组到节点卡片的一致（16px）；右侧地图 >=1280px（xl，页面达到上限、列宽不再变化）改用上游 22rem 固定带，1280px 以下保持 2:1 比例轨道；两个图层都填满轨道盒（left/top/width/height 100%），不再按世界自身比例定高——2:1 轨道比该比例更扁，定高会溢出并切掉两极——因此任何宽度都不裁切。不沿用 37:61/232px 固定带。更窄屏幕沿用上游构图：地图在先、统计在后（DOM 仍把统计放在前以利读屏，靠 CSS order 在 lg 以下还原该视觉顺序），>=640px 三列、手机两列。Node grid 使用 `repeat(auto-fill, minmax(min(100%, 22.5rem), 1fr))` 列规则，手机一列、平板两列、1280px 上限内三列；不同 grid 行不统一高度，同一行卡片仍拉伸对齐底部。统计与地图共用浅绿渐变与淡网格，Network 筛选同步改变概览、下方 Node 列表和地图范围。地图只使用 Server 提供的国家计数与国家代表点：国家按 Peer 记录逐 Node 计数（不按 IP 去重），未知国家不绘制，无法绘制或缺少代表点的国家保留可访问文字统计，不使用 `[0, 0]` 或随机点回退，也不表示受监控 Node 的部署位置。底图为固定版本、本地托管的世界国家几何（Natural Earth 1:110m Admin 0 Countries，公有领域），由 `platpulse-web/scripts/build-world-geometry.mjs` 离线生成并随 WebUI 静态资源同源托管；运行时不访问地图 CDN 或在线瓦片。Geo 停用、底图加载失败或地图渲染失败只在概览局部降级，不影响统计、筛选、排序与 Node 卡片。

Home 的列表过滤与搜索全部在 Web 端、基于同一份 Public Projection 完成：`GET /api/public/v1/networks` 不接受任何查询参数、一次返回全部 Active Node，因此不新增 Server 搜索端点、不分页、不发起第二次读取。搜索是对三个公开字段的大小写不敏感子串匹配——Node 显示名、Node ID、Network 显示名（无显示名时仍可由 Node ID 找到）——Linked Validator 标识与名称、Host/Agent 名称、Health 与身份理由文本都不参与搜索；搜索与 Health、Validator status 按 AND 组合：Health 取 `all|healthy|unhealthy|unknown`，Validator status 直接使用 Server 的 `validator.currentValidatorStatus`（`all|validator|not_validator|unknown`），缺失 Link 记为 unknown 而不写成 not_validator，绝不从 Activity、Consensus 成员或缺失区块推断。Network 选择决定读取范围，搜索/Health/Validator status 收窄 Node 列表：列表被收窄后，三项可归因统计与 Peer 国家地图读该列表选择（Server 在同一次 `GET /api/public/v1/networks` 响应中为每个列出的 Node 附带紧凑国家分桶，浏览器只对这些 Server 事实求和，不新增请求、路由或查询参数；见 [WebUI §15.28](../webui/15.28-home-selection-tiles-search-and-sorts.md#1528-home-readings-over-the-current-list-selection-the-node-selection-geo-aggregate-issue-233-delivered)），两张累计卡片保留整个 Network 选择并就地标注，排序只改变顺序；列表同时给出匹配数与作用域内总数（`Showing <matching> of <in-scope> Active Nodes`），空态区分「该 Network 选择没有 Active Node」与「没有 Node 匹配这些过滤条件」，两者都不渲染成故障、不编造 0。Home 提供紧凑卡片与列表两种视图（`view=card|list`，默认 card 且不写入 URL）：列表是一张语义化表格，含 Node／Network／Health／Current Head／Peers／Process CPU／Process memory 七列，列宽固定、窄屏只在列表内部横向滚动而不造成页面水平溢出，Node 名称与 Health 固定在列表左边缘，滚动到任何位置都能读到身份与健康，且手机宽度下 Node／Network／Health 三列在未滚动时即全部落在列表可视带内；被保留（last good）的 Peer 计数保留数值并在其下方就地标注快照口径与不再当前的维度（例如 last good (collection error, freshness stale)），完全当前与从未观测到的计数不标注；手机宽度下工具栏首行改为换行排布，Network pill 行独占整行宽度、排序与视图控件落到下一行——视觉排布变化，DOM 顺序、阅读顺序与 Tab 走查顺序（Network → 排序 → 视图 → 搜索 → Health → Validator status → 首个卡片）均不变，已交付的「pill 行只在自己内部横向滚动」口径也不受影响；排序键为 Health／Name／Current Head／Peers／Process CPU／Process memory／Disk／Uptime／Attention First 九种，六种数值排序把「从未观测到」的值排到末尾并保持真实 0（0 与 Unknown 绝不互相代替），Health 排序用 Health 自己的三个词按同一条「未经 Server 确认的取值排末尾」口径分级（unhealthy 在先、healthy 随后、Home 只能读作 Unknown 的 health 排最后，即 attention 仍领先但未经确认的 health 绝不排在已确认的 health 之前），Disk 读取 Node data directory 的 size/capacity 比例而非 Host storage，Uptime 读取 process uptime；Attention First 独立按 unhealthy → unknown → healthy 排序，不改变默认 Health 排序或 Server 健康语义。所有排序都以 Node 身份作稳定 tie-break，重取后顺序不变。`network`/`q`/`health`/`validator`/`sort`/`view` 六个查询参数按固定顺序读写、默认值不写入 URL，刷新、书签、前进后退或直接导航都恢复链接所载内容；该 URL 明确不是受保护的分享链接、也不是授权凭据，不授予任何权限。本部署无法识别的取值就地回退为默认并显式告知读者（点名参数与取值、说明回退到默认、其余部分照常生效），载入时不改写 URL，只有读者操作才写回，避免前进/后退抖动；一次读者操作只写一条历史——Network pill 在获得焦点与按下时各上报一次取值，重复请求地址栏已有的或正在写入的同一 URL 会被丢弃，使「后退一步」恰好撤销一步；Network key 是否过时只在 Projection 可用后判定（`networks === null` 表示尚未加载，选择保持临时有效、不判为过时）。空值一律视为「未提供」而非拒绝，Home 不拥有的参数（`to`）原样保留。

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

Admin 的 Node 页面聚焦配置与诊断（显示名、RPC Endpoint 诊断、Node Inventory/生命周期、Node Currency 摘要），不复刻 Home 的完整 Node Detail。Admin 不执行远程 Node 操作。

### 9.3 REST / SSE / 响应式

- REST 是唯一权威数据来源；SSE 只发 invalidation/reset 信号，不携带完整业务 DTO；
- 收到 invalidation 后 WebUI 重新读取对应 REST 资源；
- Home 与 Admin 查询缓存隔离；权限变化先清敏感缓存再重新验证；
- 必须在 360×800、390×844、768×1024、1280×800 可用：无水平溢出、表格降级为卡片、键盘可完成登录/导航/主要操作、状态不只靠颜色、Reduced Motion 下不依赖动画表达状态。

---
