本文件属于 [PlatPulse 产品与技术设计](../platpulse.md)：§2–§3。章节保留原编号，引用写作 `design §X.Y`。

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
8. 初始部署保持 Linux-first、单租户、单 Server；Server 存储为 SQLite（默认）或 PostgreSQL，不做 HA。PostgreSQL 后端是 0.3.0 目标，实施中，尚未实现；当前唯一已交付的后端是 SQLite（见 [ADR 0013](../../adr/0013-postgresql-server-storage-backend.md)）。

### 3.2 已实现的 Server/Agent 扩展

当前代码和迁移已经提供：Agent Enrollment、Recovery、Credential Rotation/Revocation、Boot/Shutdown 生命周期；Peer Snapshot、Peer Presence 与聚合历史；provider-keyed country cache 与后台国家解析（Settings 可选择 Disabled / Local MMDB / IPinfo / GeoJS）；Server-side Validator Registry/Links/PlatScan Provider、历史与日/月 analytics；Typed Alert/Incident/Silence/Maintenance；at-least-once Notification Delivery；Node Transfer；Retention、Operation、Backup/Restore、Doctor；独立 Prometheus metrics listener。它们由 Server/Admin API、CLI 或后台工作器提供，是否有当前 SPA 页面由 `docs/design/webui.md` 的路由矩阵单独决定。

### 3.3 明确未实现或明确排除

以下仍不是当前产品边界：完整交易 Body、Block Explorer/Archive、RPC Endpoint failover、Server 远程控制（命令、重启、升级或脚本下发）、多租户/HA/集群、SSO/OIDC/TOTP/WebAuthn，以及非 Linux Agent。当前 SPA 未注册的扩展页面也不应被链接为已存在的页面；这不等于对应的 Server/API 能力不存在。PostgreSQL 不再属于非目标：它是 Server 可选的第二存储后端（0.3.0 目标，实施中，尚未实现），HA、多实例与集群仍然排除，见 [ADR 0013](../../adr/0013-postgresql-server-storage-backend.md)。

---
