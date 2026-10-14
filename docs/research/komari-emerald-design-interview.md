# Emerald / Komari 对比后的设计访谈记录

状态：用户已确认最终共同理解，设计访谈结束；不是实施授权、正式规格或交付证明。正式规格与实施任务仍按项目约定进入 GitHub Issues。各轮中的“未定”是该轮结束时的历史状态，后续确认覆盖这些待定项。

## 已确认的方向（第一轮）

- 审定范围：全量审定 Emerald UI / Komari 功能对比中的改进候选，而非仅讨论单项或只给出路线图。
- 首要目标：管理闭环。
- 次要目标：故障追查与 Validator 分析；两者尚未确认先后顺序。
- 产品边界：保留现有 PlatON 监控定位、Home/Admin 权限分界以及既有非目标；不转为通用远程控制平台。
- 第一轮仅确认方向，未确认具体页面、功能、版本或实现方案。

## 已确认的候选取舍（第二轮）

- 近期设计覆盖：告警闭环、通知闭环、Validator 分析、运维管理（Q4=A+B+C+D）。
- People/Viewer 管理、Node Transfer 等其他管理缺口暂缓（Q4=E 暂缓）。
- 通用指标长历史纳入设计，先面向 Admin 故障追查；Home 暂保留当前短窗口（Q5=A）。
- Home 保留搜索、卡片/列表切换，以及返回时恢复筛选、排序与滚动位置（Q6=B）。具体搜索字段、列表列、恢复规则及实施分期未定。
- 更多 Host 指标保留进入设计；尚未确定指标清单、采集能力和保存规则。
- 临时只读分享链接与延迟/丢包诊断暂缓，不作为本次近期设计任务。
- 通用插件/主题市场不做。这是本次产品取舍，不追溯为原设计已有禁令。
- Validator 分析不恢复手工身份绑定/角色编辑；运维管理不增加在线备份创建/恢复。
- 以上是设计范围确认，不是实施授权；不代表所有页面需同时上线。告警操作、通知配置能力、运维风险控制、历史范围等仍待审定。

## 已确认的功能级别（第三轮）

- Q8=B：新增独立 Incident Acknowledgment，记录确认人/时间；不表示故障恢复、不停止通知，不复用 Agent Attention Acknowledgment。本轮未纳入处置备注/处理记录；确认撤销、复发及多人竞争细节待定。
- Q9=C：规则/有效配置/Incident 读取、基础规则编辑、Silence/Maintenance 创建与结束，以及 Network/Node 覆盖管理与继承关系展示。覆盖不扩展为任意 Agent/Host 规则覆盖。
- Q10=1+A：仅 Telegram；渠道与密钥继续由运营配置/密钥文件管理。Web 提供脱敏状态、测试、投递记录与重试，不新增浏览器渠道/密钥编辑。
- Q11=A+C：Validator 自动身份对应、当前状态、排名/质押/委托等快照趋势与来源/新鲜度；与关联 Node 的同步/共识/Peer/Incident 时间线联查。本轮不做累计奖励/产块区间差分分析，不宣称净收益。
- Q12=C：运维读取、Doctor、备份校验、任务取消、Retention 手工执行与策略修改纳入。需补充影响预览、明确确认、并发/重复提交处理及真实部分完成/取消结果；具体保障方式仍待决策。不做在线备份创建/恢复。
- Q13=B：Admin 故障追查可查询最近 30 天；不是 30 天逐次原始样本保存承诺。精度、聚合、资源预算与时间查询规则未定。
- Q14=A：首批完善已有 Host CPU、物理内存、Load 1/5/15、存储容量/使用量、网络速率的展示与历史；本轮不新增 Swap/磁盘 I/O 采集。Host 与 Node Process 分开，Host 共享值不得按 Node 重复累加。
- Q15=B：Home 名称/公开 Node ID/Network 名称搜索，Network/健康/Validator 状态筛选；紧凑列表列为名称、Network、健康、Head、Peers、进程 CPU/内存，不默认展开 QC/Locked/Committed。字段与匹配、排序、窄屏规则仍待决策；仅使用 Public 安全字段。
- Q16=B：同标签页从 Node Detail 返回恢复筛选、排序、视图和滚动；刷新或直接 URL 恢复筛选/排序/视图。滚动只在该标签页的返回路径恢复，不跨浏览器会话恢复；普通 Home 状态 URL 不是受限数据分享授权。
- 以上为已确认设计范围，不是实施、已交付页面或正式规格；最终共同理解尚未确认。

## 已确认的行为与规模（第四轮）

- Q17=A：确认绑定一次 Incident，持久共享、不提供撤销；并发以首次成功为准。真正恢复后再次触发的新 Incident 不继承；恢复后的旧确认作为历史保留。
- Q18=A：规则保存校验版本，冲突重新加载；已有 Incident 保留打开时规则版本与证据，另展示当前有效规则。改版或禁用不自动恢复、不清除确认；Preview 不承诺立即产生 Incident。
- Q19=B+1：发送前重新检查当前有效 Silence/Maintenance，抑制尚未交给渠道的匹配投递；在途/已发送不能撤回。结束窗口不补发旧消息、不生成新提醒，UI 明示仍 Open。该发送前检查是待实现的新行为。
- Q20=B：Telegram 测试/重试增加 Server 请求去重、测试冷却、结果查询；请求超时不自动重发。保持原投递状态限制和尝试历史，不承诺渠道恰好送达一次。
- Q21=B：Retention 提交后端校验预览所对应策略版本、范围与截止时间；策略/范围变化需重新预览。不承诺冻结精确删除行数；缩短保留期和实际执行明确提示后果，重复请求去重，同类清理不重复排队；取消显示已完成部分，不回滚。Doctor/备份校验不套删除确认。
- Q22=B：各自语义时间统一 UTC 坐标并置：指标采样时间、Peer 收据桶、Incident Server 评估时间、Validator Provider 优先快照时间；标明来源/接收时间/延迟。Validator 日/月边界保持配置时区；时钟可疑不自行修正，不承诺同步快照/因果。
- Q23=A：最近 24 小时原始样本、更早允许聚合；保留尖峰、样本数、覆盖/缺口信息。精确聚合粒度和容量规则尚未确认。新历史图不补零、不跨真实缺口；接受建议一并修正 Home 同类绘图问题，仍保留 60 秒窗口。
- Q24：规模目标 Agent 2 / Node 6；挂载点数量、CPU、内存、历史预算全部未知，是待验证条件，不推定容量或性能。未知硬件不是当前开发机硬件的代称。
- Q25=A：存储历史按 Agent＋挂载路径；路径改变是不同序列，不冒称物理磁盘身份或可靠识别同路径换盘，不在首批扩充稳定设备身份协议。
- Q26=B：Validator 联查按已记录关联有效期间，无证据显示未知、不倒推；日快照排名/质押/委托人数为首批趋势，last-good 保留年龄，累计值不称区间收益。
- Q27=A：只有 Network 选择改变 Home 统计与地图；文本搜索/健康/Validator 筛选仅作用结果列表，显示匹配数/范围总数，标明口径差异。
- Q28=A：普通大小写不敏感子串匹配，不加正则/模糊拼音，搜索与筛选组合。保留 Health/Name/Head 排序，加入 Peers/进程 CPU/内存；未知置后、并列稳定。窄屏列表保留名称/健康，其他列局部横向滚动不溢出整页；无效 URL 筛选回退并提示。
- Q29=A：Admin 四个新增一级领域入口：告警（Incident/规则与覆盖/静默维护）、通知（渠道/事件投递）、Validator（列表/详情联查）、运维（任务/Doctor/Retention/备份读取校验）；Node/Agent 详情提供快捷入口，保留 Emerald workspace/手机抽屉。
- 上述是用户确认的设计行为，不是实现授权或已交付证明。仍需收敛存储粒度、压力下行为、完整查询覆盖与验收、交付分期及最终共同理解。

## 已确认的保存、调查与交付规则（第五轮）

用户确认 Q30～Q38 全部按推荐：Q30=A、Q31=A、Q32=B、Q33=B、Q34=B、Q35=A、Q36=A、Q37=A、Q38=A。

- Q30=A：最近 24 小时保留通用指标原始样本，第 2～7 天使用 1 分钟聚合，第 8～30 天使用 5 分钟聚合；保留极值、样本数、覆盖/缺口，标明聚合不是原始观测、放大不恢复已淘汰原始点。此策略不把 Validator 日/月或 Peer 既有不同粒度改造成通用指标采样。
- Q31=A：首批调查所需历史设置最低约束：通用指标原始至少 24 小时、相应聚合及新增状态历史覆盖至少 30 天，运维页面不能将其缩短到目标以下。现有更长保存不自动缩短；不改变永久删除 Node 的边界，不承诺既有或启用时已积累完整历史，失败/低空间缺口仍真实展示。
- Q32=B：新增容量展示和可配置低空间保护；压力下优先暂停可选的新增长历史写入，显式缺口，条件恢复后恢复写入。不隐式删除承诺范围内的数据，不自动降精度。核心 Report 无法持久化，不返回成功 Receipt。没有未经验证的固定 GiB 上限；保护阈值/容量结论通过后续实测确定，不把开发机当生产承诺。
- Q33=B：首批新增已有采集的同步状态与共识关键高度历史，支持 30 天联查。数值遵循已选精度，状态变化、采集失败、未知独立表达；不新增链上推断指标，不用 Block/Incident 倒推。新增记录从功能启用后积累。
- Q34=B：Node 调查同时纳入有历史关系证据的 Agent/Host/Network Incident，标为相关主体而非 Node 自身事件；不足期间标未知，不把当前归属覆盖全段历史。Validator Link 只称 Server 已记录关联窗口，不证明链上实际换键时刻或缺采期关系持续真实；Purge 后关联证据不可用不等于从未关联。
- Q35=A：Admin 调查共用 UTC 时间窗，1h/6h/24h/7d/30d 与范围内自定义，默认 24h；从 Incident 进入定位发生时间并可调整。按各源真实粒度/来源时间/配置日月边界展示，不称同步采样；明确覆盖、缺口、截断、加载失败，事件分页或分段查询，有限返回不当完整结果。启用前、清理、删除边界均明确，不伪造回填。
- Q36=A：Home 首批完整 Public Node 集合本地搜索/排序，不新增分页。返回以 Node ID＋相对位置恢复，排序变化优先定位同一 Node；目标消失回到附近合理位置并提示，不回空白。滚动仅当前标签页返回，URL 不保存滚动、会话不恢复滚动。
- Q37=A：每阶段独立验收：正常流程以及并发确认/规则冲突、超时不自动重发、发送前抑制、清理取消部分结果、来源失败、历史缺口、关系变化与删除；继承 Owner/Public 隔离、Unknown 非零、审计等要求。覆盖 360×800、390×844、768×1024、1280×800、1440×900，明暗、键盘/触摸。历史阶段交付 2 Agent/6 Node、明确挂载数量及采样条件的容量/查询/写入影响报告，注明硬件与适用条件，不声称已证明未知生产资源可容纳 30 天。
- Q38=A：四阶段独立交付/验收：①告警＋Telegram 通知闭环（确认/并发/抑制/去重保障）；②运维闭环＋历史保存查询基础（Retention 安全控制）；③Validator 分析＋Node 历史联查；④Home 搜索/卡片列表/返回恢复/视觉完善。底层依赖可提前准备；顺序不取消既定范围，不要求同时上线，不构成立即实施授权。

## 复核后的事实基线

实现事实、已接受设计与建议必须分开。下列事实取自当前本地代码，不以设计文档中的目标替代实现证据。

| 领域 | 当前事实 | 源码证据 |
| --- | --- | --- |
| SPA | 已注册 Home、登录、Node Detail 和 Admin Overview/Agents/Nodes/Networks/Settings/Sessions/Audit；下述领域没有独立管理页面 | [App.tsx](../../platpulse-web/src/App.tsx#L125-L169) |
| 告警 | 有规则、预览、覆盖、Incident、Silence 和 Maintenance API | [alerts_admin.rs](../../crates/platpulse-server/src/http/alerts_admin.rs#L2955-L2988) |
| 通知 | 有事件/投递/渠道读取、投递重试和渠道测试 API；没有渠道创建/编辑 API | [notifications_admin.rs](../../crates/platpulse-server/src/http/notifications_admin.rs#L812-L837) |
| 通知渠道 | 唯一生产适配器为 Telegram；渠道来自运营配置及密钥文件，不能假定已支持浏览器配置任意渠道 | [config.rs](../../crates/platpulse-server/src/config.rs#L135-L156)、[notifications.rs](../../crates/platpulse-server/src/notifications.rs#L167-L204) |
| Agent 离线 | 已存在 agent.offline 规则；被监控 Agent 离线不等于发送通知的 Server 停机 | [alerts/mod.rs](../../crates/platpulse-server/src/alerts/mod.rs#L184-L188) |
| Validator | 有读取、历史和分析 API；旧手工管理路径返回 410 manual_validator_management_retired，不能设计手工绑定/角色编辑按钮 | [validators_admin.rs](../../crates/platpulse-server/src/http/validators_admin.rs#L527-L565)、[router](../../crates/platpulse-server/src/http/validators_admin.rs#L847-L875) |
| 运维 | 有 Retention、Operations、Doctor API；具体动作必须逐项评估，而非统称只缺前端 | [operations_admin.rs](../../crates/platpulse-server/src/http/operations_admin.rs#L1882-L1914) |
| 备份/恢复 | 备份 API 仅列表、详情和校验；无在线创建端点。服务中的恢复 worker 返回 restore_requires_stopped_server；实际恢复要求停服后走离线 CLI | [routes](../../crates/platpulse-server/src/http/operations_admin.rs#L1905-L1913)、[restore.rs](../../crates/platpulse-server/src/restore.rs#L268-L274) |
| 当前指标图 | Public Node 指标图窗口固定为 60 秒，不是采样周期，也不是所有历史的总保存期限 | [public.rs](../../crates/platpulse-server/src/http/public.rs#L1212)、[query](../../crates/platpulse-server/src/http/public.rs#L1431-L1467) |
| 指标保存 | SQLite 真实持久化；每条指标序列仅保留最新 64 个样本。通用长历史与聚合不能视作已有功能 | [report_ingestion.rs](../../crates/platpulse-server/src/http/report_ingestion.rs#L786-L843)、[retention.rs](../../crates/platpulse-server/src/retention.rs#L67-L82) |
| 现有聚合 | Peer 有 5 分钟/小时聚合；Validator 有日/月聚合，不等于通用 CPU/内存指标聚合 | [peer_history.rs](../../crates/platpulse-server/src/peer_history.rs#L109-L161)、[validator.rs](../../crates/platpulse-server/src/validator.rs#L1968-L2027) |
| 权限 | Admin 仅 Owner；Viewer 只读 Home。保留独立 Public/Admin 投影，不因功能补齐混用 DTO 或扩大权限 | [CONTEXT.md](../../CONTEXT.md#L371-L381) |

### 下一轮所需的进一步事实

- Incident 当前仅提供列表/详情读取，没有人工确认、已处理或手工恢复 API，DTO 无确认字段。既有 Agent Attention Acknowledgment 不是 Incident 处置状态（[routes](../../crates/platpulse-server/src/http/alerts_admin.rs#L2955-L2988)、[DTO](../../crates/platpulse-server/src/http/alerts_admin.rs#L260-L277)、[领域定义](../../CONTEXT.md#L319-L336)）。
- Host 和 Node Process 是独立指标主体。已有 Host Load 1/5/15 分钟平均值与容量/吞吐字段；Swap 和磁盘 I/O 没有现有 Host 协议字段，纳入首批需新增采集等能力（[observation.rs](../../crates/platpulse-core/src/observation.rs#L47-L98)、[领域定义](../../CONTEXT.md#L227-L233)）。
- Validator 日数据是每日最新快照；月数据包含月内快照计数、排名范围以及最后累计值，不能直接标为每日/月奖励或每日/月产块量（[DTO](../../crates/platpulse-server/src/http/validators_admin.rs#L103-L147)、[日/月写入](../../crates/platpulse-server/src/validator.rs#L1968-L2027)）。
- `mutation_guard` 的布尔参数只控制 JSON Content-Type 校验，实际保护包括 Origin 和 CSRF；不是危险操作确认或任务忙门禁（[admin.rs](../../crates/platpulse-server/src/http/admin.rs#L2941-L2967)）。Retention 策略更新仅接收 `retentionDays`，没有强制绑定影响预览/确认/版本条件；手工 run 会实际按批清理历史，取消不能恢复已删数据（[请求](../../crates/platpulse-server/src/http/operations_admin.rs#L90-L118)、[run 请求](../../crates/platpulse-server/src/http/operations_admin.rs#L486-L490)、[执行](../../crates/platpulse-server/src/retention.rs#L569-L593)）。
- Doctor、Retention run、Backup verify 以独立 UUID 创建排队 Operation；工作线程依次调度，但提交端未实现“已有同类任务忙”或请求去重保护（[入队](../../crates/platpulse-server/src/http/operations_admin.rs#L1820-L1866)、[Operation](../../crates/platpulse-server/src/operations.rs#L64-L129)）。Doctor 不自动修复；Backup verify 不创建/恢复备份。运行中取消是请求到检查点停止，不是回滚或立即结束。
- 通知测试是真正向目标发送，并创建独立 Event/Delivery；HTTP 200 不等于送达成功，重复测试没有已有幂等/冷却保障。手工重试复用原 Delivery，保留尝试计数；只允许 `retry_scheduled`/`failed`/`dead_letter`（[retry](../../crates/platpulse-server/src/notifications.rs#L429-L463)、[test](../../crates/platpulse-server/src/notifications.rs#L803-L852)）。
- 更丰富的 Validator 计划产块、来源产块率、委托分配比例等已有 Provider/Public 字段，但现有 Admin Insight/日/月 DTO 未完整输出；分析页若纳入，需扩充 Admin 能力，不复用 Public DTO 掩盖权限/口径差异（[Admin](../../crates/platpulse-server/src/http/validators_admin.rs#L36-L55)、[Public](../../crates/platpulse-server/src/http/public.rs#L692-L782)）。
- 实际通用历史中，Node 有进程 CPU/内存百分比、数据目录百分比、入/出 Peer 数；Host 仅有收发速率历史。Host CPU/内存/Load 当前值已有采集，不代表已有历史序列。Host 以 Agent 为主体共享，在多个 Node 页面显示时不可重复累加（[采样写入](../../crates/platpulse-server/src/http/report_ingestion.rs#L1006-L1027)、[Node 引用 Host 网络](../../crates/platpulse-server/src/http/public.rs#L1435-L1443)）。

### 生命周期与联查时间口径的再次复核（第四轮前提）

下列是现状，不是第四轮用户已接受的行为设计：

- 恢复后再次满足触发条件会产生新 Incident ID；恢复等待期间再度异常仍是原 Incident。禁用规则仅停止评估，不关闭 Open Incident；改版不替换打开时的证据/版本。Unknown/Stale 不表示恢复；主体删除只标注历史、停止评估及取消未发送投递，不构成恢复（[状态机](../../crates/platpulse-server/src/alerts/mod.rs#L587-L668)、[禁用/改版/发生](../../crates/platpulse-server/src/alerts/mod.rs#L1525-L1619)、[删除](../../crates/platpulse-server/src/subject_deletion.rs#L74-L112)）。注意：Unknown 分支的 evaluation_unavailable 并非所有状态统一为 true，不能从一个布尔值推断全部情况。
- Silence/Maintenance 只在创建 Delivery 时决定抑制；suppressed 不发送且不可重试。现有发送 worker 不重新匹配后来创建的静默，结束/取消不补发旧投递或自动提醒仍 Open 的 Incident；恢复事件也按恢复当时的抑制策略处理，可能只收到恢复消息（[创建](../../crates/platpulse-server/src/notifications.rs#L327-L369)、[重试](../../crates/platpulse-server/src/notifications.rs#L429-L447)、[发送](../../crates/platpulse-server/src/notifications.rs#L630-L670)、[恢复事件](../../crates/platpulse-server/src/alerts/mod.rs#L1782-L1844)）。
- 规则 Preview 不提交业务变更、不发送通知；would_fire 是 enabled 与即时阈值结果，不能等同此刻一定打开 Incident；projected_state 也不能忽略 enabled 单独解释为实际触发（[preview](../../crates/platpulse-server/src/http/alerts_admin.rs#L1190-L1249)）。
- 诊断不是单一同步快照：每个组件的采样/接收时间与新鲜度需分别解释，Node 整体 freshness 不覆盖全部 Host/Process 指标（[DTO](../../crates/platpulse-server/src/http/admin.rs#L1136-L1227)、[整体 freshness](../../crates/platpulse-server/src/http/admin.rs#L1991-L1997)）。
- Host mount 只有路径、容量、使用量，没有稳定设备/文件系统身份；同路径换盘不可可靠识别，路径更名不可直接拼成同一磁盘历史。已有采集也不表示 Admin DTO 已输出所有挂载点（[协议](../../crates/platpulse-core/src/observation.rs#L69-L98)、[投影](../../crates/platpulse-server/src/http/report_ingestion.rs#L997-L1003)）。
- Peer 桶使用 Server received_at；first_observed_at/last_observed_at 当前实际上也赋值为 received_at。读取固定最近 288 个 5 分钟桶/168 个小时桶，没有任意时间范围（[时间赋值](../../crates/platpulse-server/src/peer_history.rs#L360-L361)、[分桶](../../crates/platpulse-server/src/peer_history.rs#L443-L458)、[API](../../crates/platpulse-server/src/http/admin.rs#L2541-L2569)）。
- Block from/to 是高度不是墙钟时间；链时间与 Agent observed_at 分开，历史混有 Gap/Divergence。Incident 当前也没有 from/to 或 subjectKey 过滤；打开/恢复时间是 Server 评估时间，不是异常样本时刻（[Block](../../crates/platpulse-server/src/http/admin.rs#L4082-L4112)、[Incident filters](../../crates/platpulse-server/src/http/alerts_admin.rs#L288-L294)、[评估时间](../../crates/platpulse-server/src/alerts/mod.rs#L1585-L1610)）。
- Validator sample_at 优先 Provider 时间、否则接收时间；日/月边界按配置 IANA 时区，历史/分析仅条数限制而非统一 from/to。联查必须保留来源/延迟/日期边界，不冒称同步采样或直接因果（[时间口径](../../crates/platpulse-server/src/validator.rs#L1913-L1937)、[日 DTO](../../crates/platpulse-server/src/http/validators_admin.rs#L103-L118)）。
- 现有 Home 折线排序后直连所有有限点，并把最后值持平延伸到右边界，没有缺样分段；长历史不能照搬后宣称已符合 Unknown/Gap 要求。无样本不自动补零和真实缺口断线是不同问题（[绘图](../../platpulse-web/src/pages/HomePages.tsx#L569-L600)、[状态原则](../design/webui.md#L199-L218)）。

### 对上一份比较的勘误

- “所有后端 API 已完整、补页面成本都低”不成立，需按功能核实。
- “缺少通知渠道”应改为：已有 Telegram，尚无已实现的其他生产适配器。
- “离线通知是非目标”不成立；现有 Agent 离线规则与通知链路已实现。
- 未发现权威文档明确禁止所有插件；没有插件架构不能推导为既有禁令。是否建设插件能力仍是候选取舍，不能冒充产品边界事实。
- 不以功能数量判断任一产品整体安全性。

## 第四轮后新增的只读核实

- [Report 写入](../../crates/platpulse-server/src/http/report_ingestion.rs#L786-L843) 每个指标序列仅保留最新 64 点，不是 24 小时时间保留；[迁移约束](../../crates/platpulse-server/migrations/0041_node_metric_history.sql#L8-L39) 当前仅 Node 五项、Host 网络两项。扩为 24h/30d 和 Host 新历史需要 schema/写入/聚合/保存期/查询，不是改前端范围。
- 默认 5s 仅是报告节奏：理想单序列 24h 新观测为 17,280 点；当前 6 Node×5＋2 Agent×2＝34 序列，理想合计 587,520 点/24h，不含新增 Host/挂载项及其他数据。目录扫描较低频、重复 observed_at 去重、断采会降低实际点数；相同数值但新 observed_at 仍新增点。未知挂载数/布局/硬件，不据此估算 GiB 或承诺吞吐；Host 按 Agent 共享存储。
- [Retention 通用聚合](../../crates/platpulse-server/src/retention.rs#L67-L81) 仍 unsupported；已核实配置/schema/Server 搜索未找到通用指标主动字节预算或低磁盘保护。[Doctor storage](../../crates/platpulse-server/src/doctor.rs#L460-L476) 是权限/所有权，不是容量；[内部指标](../../crates/platpulse-server/src/metrics.rs#L623-L633) 有 SQLite page_count/freelist_count、WAL 字节，不等于历史专属空间或剩余磁盘。
- [自动 Link](../../crates/platpulse-server/src/validator.rs#L2328-L2439) 时间是 Server 发现轮次，半开区间；缺 key/观测可能保留开放区间，不证明其中每时刻关联持续真实，不倒推为链上/Agent 换键时间。[Node Purge](../../crates/platpulse-server/src/node_purge.rs#L270-L307) 删除 Node、Link 和其历史，之后关联证据不可用，不代表从未关联；不改变已定永久删除边界。
- [Sync/Consensus 写入](../../crates/platpulse-server/src/http/report_ingestion.rs#L1321-L1343) 是 latest 投影，暂无完整历史；选中的历史联查需要新增记录/查询，不可把当前值贴到过去。Peer 默认保存可覆盖 30d 但当前查询只取最新固定桶；实际策略/删除可造成更短覆盖。
- [Incident filters](../../crates/platpulse-server/src/http/alerts_admin.rs#L281-L294) 无 subjectKey/from/to；列表最多 500，Validator 事件最多 200、日/月最多 366、Peer 固定桶，均没有统一分页/完整覆盖承诺。Node 联查 Agent/Host/Network 事件需按有证据的历史关系纳入并标来源；仅当前所属关系不能证明全段历史归属。新查询必须明确覆盖、截断、缺口和删除边界。
- 本次两代理均只读完成，无代码修改、测试、benchmark 或后台作业。以上仅事实复核，不替用户决定未确认的粒度/压力/补充记录/验收与分期。

## 设计树收敛与最终确认状态

- 范围/不做项、管理动作及并发副作用、历史精度/保存约束/压力行为、关系与时间口径、查询覆盖、Home 行为、验收与交付顺序均已逐轮选择。用户明确确认最终汇总并结束设计访谈；当前没有尚待选择的产品行为分支。
- 未知生产挂载数量、CPU、内存、磁盘预算仍是显式验证条件，不伪装成已解决事实、不阻塞已选策略的记录，也不由开发机代填。容量与性能实测、低空间阈值、测试冷却及请求去重保留时长等可配置/工程参数需在后续规格和验证中明确，不能在本记录中伪称已测或已承诺。
- 新功能须补充 Server/API/schema/保存/聚合/分页覆盖/审计与 Web UI，不是纯前端补页。新增历史从记录启用后逐步积累；既有真实证据只能按实际范围使用，不能伪造过去，低空间/断采/删除均保留可见边界。
- 最终确认只结束设计访谈，不自动授权创建/发布 GitHub Issues、代码实现、运行测试/benchmark、修改部署、创建在线备份或恢复。正式规格及实施任务仍使用 [GitHub Issues 约定](../agents/issue-tracker.md)；需要后续人类请求再进入相应工作。
- 已更新 [Incident Acknowledgment](../../CONTEXT.md#L339-L341) 与 [Node Validator Link](../../CONTEXT.md#L63-L65) 的领域含义，未把存储/API 实现方案写进词汇表。没有新增同时满足难逆转、出人意料及真实取舍三条件的架构决策，本轮不新增 ADR。

## 后续转规格与发布

- 用户随后请求 `/to-spec`，并确认现有 Playwright 真 Server＋临时 SQLite 主验收、Rust router/真实 SQLite/可替换 Provider 的竞态故障补充，以及 App 级纯状态测试边界；本次请求授权发布规格，不授权代码实现或部署。
- 正式规格已发布为 [GitHub Issue #202 — complete Admin workflows, 30-day investigation, and Home usability](https://github.com/mowind/PlatPulse/issues/202)，状态 Open，标签 `ready-for-agent`。包含完整模板七节、81 条 User Stories、四阶段交付顺序、实施与测试决定、不做项及未知资源验证条件。
- 发布后已回读验证正文与审查版本一致、标签与状态正确。未运行功能测试或 benchmark，未实施、提交或推送代码，未修改部署。本文件仍为访谈过程记录，正式规格以该 Issue 为准。

