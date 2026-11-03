# PlatPulse 产品与技术设计（当前实现基线与已确认演进）

## 1. 文档状态

- 状态：§1–§14 保留当前实现与边界基线；[§15](platpulse/15-management-and-validator-identity.md#accepted-management-target)记录已确认、部分实现的演进——已实现部分以对应 GitHub Issue 为准，未实现部分不能据此宣称新 API、页面或迁移已交付。实现、迁移、运行时路由和本文件应相互校验。
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

## 各章节所在文件

本设计的 §2 起各章节分布在 [platpulse/](platpulse/) 下的文件中，每个文件放一个顶层章节或一段相邻章节，使每个文件都不超过文档篇幅上限（`scripts/check-doc-budgets.py`）；§1 留在本文件。每个章节保留原编号与标题：`design §X.Y` 无论章节在哪个文件都指向同一节，`scripts/check-design-refs.py` 会把本文件与 `platpulse/` 下所有文件的标题合并解析。新章节放进其相邻章节所在的文件；该文件会超过上限时，新建文件并在下表登记。

| 章节 | 内容 | 文件 |
|---|---|---|
| §2–§3 | 产品形态与边界；当前范围、核心链路、已实现扩展与明确排除 | [02-product-shape-and-scope.md](platpulse/02-product-shape-and-scope.md) |
| §4–§5 | 运行时拓扑与领域模型；观测维度、采集、Peer 与 Geo Provider | [04-runtime-model-and-observation.md](platpulse/04-runtime-model-and-observation.md) |
| §6–§7 | Block Summary 与 Block History；Agent 配置、Agent Store、AgentReport 与 Report Receipt | [06-block-history-and-agent.md](platpulse/06-block-history-and-agent.md) |
| §8–§9 | Server 职责、数据模型、Report Ingestion、HTTP API 与 Admin Overview；WebUI | [08-server-and-webui.md](platpulse/08-server-and-webui.md) |
| §10–§11.5 | 身份与安全；故障语义、数据边界与低空间保护 | [10-security-and-reliability.md](platpulse/10-security-and-reliability.md) |
| §11.6（至「Host 证据属于 Agent」前） | 24 小时原始样本、30 天聚合层、tier 边界与读取预算 | [11.6-metric-history-raw-and-aggregate-tiers.md](platpulse/11.6-metric-history-raw-and-aggregate-tiers.md) |
| §11.6（自「Host 证据属于 Agent」起） | 共享 Host 系列、按挂载路径的存储系列、Public Node Detail 的 Metric Window | [11.6-shared-host-series-and-metric-window.md](platpulse/11.6-shared-host-series-and-metric-window.md) |
| §11.7–§14 | 同步与共识状态历史；部署；当前实现验收基线；设计原则 | [11.7-state-history-deployment-and-acceptance.md](platpulse/11.7-state-history-deployment-and-acceptance.md) |
| §15–§15.5 | Agent/Node 管理、Node Purge、自动 Validator 身份与快照趋势、Validator 模型迁移 | [15-management-and-validator-identity.md](platpulse/15-management-and-validator-identity.md) |
| §15.6–§15.9 | Attention Acknowledgment、恢复与通知、待实现验收、Inventory 声明纪律 | [15.6-attention-recovery-and-inventory-discipline.md](platpulse/15.6-attention-recovery-and-inventory-discipline.md) |
| §15.10 | Server 托管 Inventory Revision | [15.10-server-managed-inventory-revision.md](platpulse/15.10-server-managed-inventory-revision.md) |
