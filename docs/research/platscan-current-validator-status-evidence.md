# PlatScan Current Validator Status 证据谓词（#168）

调研日期：2026-09-20。用于 #165（§15）"自动 Validator 身份与 Current Validator Status"的实现前置（#173），回应 [Validator Provider](../design/validator-provider.md) 与 [Validator metrics](../design/validator-metrics.md) 中的 **Technical verification pending** 占位，以及 [ADR 0005](../adr/0005-automatic-validator-identity.md) 中"exact PlatScan evidence mapping still requires primary-source verification"的未决项。

本文只确立证据谓词与可复用 fixture；不实现分类、不执行迁移、不删除或改写任何运行时数据。所有结论以**一手来源**（browser-server 源码、PlatON-Go 源码）与**主网部署实测**为准；未能证实之处一律标注为限制，不以推测代替事实。本文不把旧的整数 Activity 映射当作已经验证的当前有效性（见 §8）。

## 1. 结论摘要

1. PlatScan 详情接口 `status` 不是链上 `CandidateStatus` 位掩码，而是 browser-server 由**自身数据库状态** `CustomStaking.StatusEnum` 加 `isConsensus` / `isSettle` 派生的**展示码**；它由 `StakingStatusEnum.getCodeByStatus` 唯一决定。
2. `candidate`(1)、`active`(2)、`producing`(3) 是当前有效质押身份：`1/2` 由详情接口给出，`3` **只能**由 `aliveStakingList` 的"当前出块节点"分支给出——详情接口没有任何分支会返回 3。
3. `exiting`(4) 与 `locked`(7) 在**新鲜、成功且身份匹配**的详情观测下是当前有效质押身份，但必须显示限定词：链上候选人在退出冻结期结束前仍是有效候选人（`WithdrewStaking` 才置 `Invalided|Withdrew`），锁定是保留质押、可恢复为候选人的惩罚态。
4. `exited`(5) 是**已完成退出**的权威否定；`nodeId==""` 且 `status==0` 的 200 空对象是**权威缺席**。二者共同构成可接受的否定证据。
5. PlatScan **不存在**名为"inconclusive"的状态码。`verifying`(6) 的源码语义是"候选人在共识周期"（CANDIDATE + `isConsensus=1`）；但 [CONTEXT.md](../../CONTEXT.md) 的 Current Validator Status 定义、[validator-metrics.md](../design/validator-metrics.md) 与 ADR 0005 都把"verification in progress"归为 **Unknown**。因此 **6 判 Unknown**：既不是否定，也不是对当前有效性的确认。源码事实另行记录；要改判为 Validator 须先改领域定义（domain-modeling），不是实现细节。
6. **HTTP 404、传输失败、`aliveStakingList` ranking 缺失都不得作为无质押/否定的证据**；详情接口对"不存在的节点"本来就以 200 空对象回答，404 只会来自路由/部署异常。
7. 主网部署在 2026-09-20 实测确认了 1/2/4/5/6/7、空缺席形态与"列表 3 / 详情非 3"的差异；原始响应与 provenance 见 §7。

## 2. 来源与 revision

| 来源 | revision | 日期 | 用途 |
| --- | --- | --- | --- |
| [`PlatONnetwork/browser-server`][bs] | `6daa5ad2e878474869407314a73a219ac49c8b51` | 2024-03-20 | 定义 `status` 展示码、详情/列表接口、空值序列化 |
| [`PlatONnetwork/PlatON-Go`][go] | `8bc8bddec1aa5771e59173f37ebd3a12258c3c95`（`develop`） | 2026-09-03 | 候选状态位掩码、退出/惩罚处理 |
| 主网部署 `https://scan.platon.network/` | 未知（部署自报 PlatON Mainnet，chainId 210425） | 2026-09-20 实测 | 状态码与空缺席形态的部署证据 |

限制：browser-server 只固定到 2024 年提交，**部署实际运行的 revision 未经证实**；PlatON-Go 使用的是 2026-09-03 的 `develop` 快照，与浏览器抓取时的链版本未必一致。两者只用于解释状态语义，不作为"部署一定运行该 revision"的断言。

[bs]: https://github.com/PlatONnetwork/browser-server/tree/6daa5ad2e878474869407314a73a219ac49c8b51
[go]: https://github.com/PlatONnetwork/PlatON-Go/tree/8bc8bddec1aa5771e59173f37ebd3a12258c3c95

## 3. `status` 是如何产生的（主源逐步还原）

### 3.1 存储层状态与展示层状态是两套枚举

- 存储层：`CustomStaking.StatusEnum` 只有 4 个值——`CANDIDATE(1)`、`EXITING(2)`、`EXITED(3)`、`LOCKED(4)`（[CustomStaking.java][customstaking]）。
- 展示层：`StakingStatusEnum` 是 API 合同里的 1..7（[StakingStatusEnum.java][statusenum]）：

  | name | code | 源码注释 |
  | --- | ---: | --- |
  | `candidate` | 1 | 候选中 |
  | `active` | 2 | 活跃中 |
  | `block` | 3 | 出块中 |
  | `exiting` | 4 | 退出中 |
  | `exited` | 5 | 已退出 |
  | `verifying` | 6 | 共识中 |
  | `locked` | 7 | 锁定中 |

- 映射函数 `StakingStatusEnum.getCodeByStatus(status, isConsensus, isSetting)`（源码 + [`StakingStatusEnumTest`][statustest]）逐分支为（源码第三个参数名为 `isSetting`，调用处传入 Node 的 `isSettle` 列）：

  | 存储状态 | `isConsensus` | `isSettle`（`isSetting`） | 展示码 |
  | --- | --- | --- | ---: |
  | CANDIDATE(1) | 1 | — | 6 verifying |
  | CANDIDATE(1) | 0 | 1 | 2 active |
  | CANDIDATE(1) | 0 | 0 | 1 candidate |
  | EXITING(2) | — | 1 | 2 active |
  | EXITING(2) | — | 0 | 4 exiting |
  | EXITED(3) | — | — | 5 exited |
  | LOCKED(4) | — | — | 7 locked |

  注意：该函数**没有任何分支返回 3**。`3` 只由列表接口用"当前出块节点"覆盖（见 3.3）。

### 3.2 详情接口 `stakingDetails`

`StakingService.stakingDetails`（[StakingService.java][stakingservice]）：

- 先 `nodeMapper.selectByPrimaryKey(nodeId)` 查 `node` 表；命中则 `setStatus(getCodeByStatus(...))`。
- **未命中时不报错、不 404**，直接返回一个全默认值的 `StakingDetailsResp`（`BaseResp.build(RET_SUCCESS, ...)`）。

因此详情接口的可能取值是 `{0,1,2,4,5,6,7}`：正常状态永远不含 `3`。

### 3.3 列表接口 `aliveStakingList`（ranking 同源）

`aliveStakingList`（`queryStatus=all`）实际查询条件是：

`store.status == CANDIDATE` **OR** (`store.status == EXITING` AND `isSettle == YES`)

即它排除 `LOCKED`、`EXITED` 以及**不在结算周期验证人集合中的** `EXITING`。随后逐行：若 `nodeId == networkStatRedis.getNodeId()`（Redis 缓存的当前出块节点）则强制 `status = 3 block`，否则调用 `getCodeByStatus`。

两个直接推论：

1. `3 producing` 是"列表 + 当前出块节点"专属状态；详情接口无法复现，因为详情没有这个分支。
2. **不在 `aliveStakingList` 里 ≠ 没有质押**：`LOCKED` 与"非结算期的 `EXITING`"都保留有效质押却被该查询排除。ranking 缺失因此不能当作否定证据。

### 3.4 空缺席形态：200 + 全默认对象，而不是 404

browser-server 用 `CustomBeanSerializerModifier` 给不同字段类型的 null 注册默认序列化器（[CustomBeanSerializerModifier.java][modifier]）：

- `String` 的 null → `""`（[NullStringJsonSerializer][nullstr]）
- `Integer/int/Long/long/Double/double` 的 null → `0`（[NullIntegerJsonSerializer][nullint]）
- `List/Set/Array` 的 null → `[]`
- 其它类型（`Boolean`、`BigDecimal`）**不处理**，保持 `null`

所以未命中节点时，`StakingDetailsResp` 会序列化成 `nodeId:""`、`status:0`，而金额字段仍为 `null`。2026-09-20 的主网实测与推导完全一致：

~~~json
{"errMsg":"Success","code":0,"data":{"nodeName":"","stakingIcon":"","status":0,
 "totalValue":null,"delegateValue":null,"stakingValue":null,"delegateQty":0,
 "...":"...","nodeId":"","...":null,"version":""}}
~~~

见 fixture [`staking-details-empty.json`][emptyfixture] 与 [`provenance.json`][prov]。这同时说明：**"节点不存在"在部署合同里是 200 语义，`HTTP 404` 只可能来自路由/网关/部署异常。**

## 4. 证据谓词

定义 PlatPulse 的 Current Validator Status 取值：`Validator`（含 `locked`/`exiting` 限定）、`NotValidator`、`Unknown`、`Stale`（保留 last-good）。

对**已建立关联**（已校验 Network + 观测到的完整 P2P 公钥）且**新鲜、成功、身份匹配**的 `stakingDetails` 观测：

| 详情 `status` | browser-server 语义（§3.1） | Current Validator Status | 依据 |
| ---: | --- | --- | --- |
| 1 candidate | 在册候选人，非共识期、非结算期 | **Validator** | 候选人质押在册；链上 `CandidateStatus.IsValid()` |
| 2 active | 候选人且在结算期，或退出中且在结算期 | **Validator** | 两种来源都是有效在册身份；仅凭状态无法区分，无需区分 |
| 3 producing | 列表专用：当前出块节点 | **Validator** | 只可能来自 `aliveStakingList`；详情不会返回 |
| 6 verifying | 候选人且在共识周期 | **Unknown** | 源码语义是 CANDIDATE + `isConsensus=1`，但 CONTEXT.md / ADR 0005 / metrics 把"verification in progress"判为 Unknown（见 §4.2） |
| 4 exiting | 退出中，且不在结算期 | **Validator（exiting 限定）** | 退出冻结期未结束，链上尚未 `Invalided`（§5） |
| 7 locked | 低/零出块惩罚锁定 | **Validator（locked 限定）** | 质押保留且可恢复为 CANDIDATE（§5） |
| 5 exited | 已完成退出 | **NotValidator** | 链上候选人已 `Invalided|Withdrew` 并可能删除（§5） |
| 0 且 `nodeId==""` | `node` 表未命中 | **NotValidator（权威缺席）** | §3.4 实测空缺席形态 |
| 0 且 `nodeId!=""` | 无法映射的存储状态 | **Unknown** | 不得伪造成否定 |
| 其它整数 / 状态缺失 | 未识别 | **Unknown** | 不得伪造成否定 |

**观察方法**：表中 `Validator`/`NotValidator` 行都要求一次针对关联身份的**新鲜成功** `stakingDetails` 观测（HTTP 2xx、`code==0`、`data` 为对象、`nodeId` 等于请求、`status` 为整数）。`Unknown` 行不是某个状态值，而是**非建立性结果**：§6 的失败/超时/404/畸形/非 0 `code`/未识别状态（适配器 `platscan_status_activity` 只接受 1..7，其余归 `Error`）、从未成功观测，或关联本身未建立。

### 4.1 locked / exiting 何时有效，何时 Unknown

**有效（Validator + 限定词）**：对已建立关联的身份，一次**新鲜成功**的详情观测返回 `status ∈ {4,7}`（或可能被折叠为 `2` 的"结算期 exiting"）。依据：

- `exiting`：`StakeExitAnalyzer` 在**收到撤销质押交易时**写入 `EXITING`（[StakeExitAnalyzer.java][stakeexit]）；链上真正结束退出发生在目标 epoch 的 `WithdrewStaking`，届时才 `can.Status |= Invalided | Withdrew` 并可能 `DelCandidateStore`（[staking_plugin.go][stakeplugin]）。冻结期内候选人仍在册、质押转为赎回中，身份仍有效。
- `locked`：`OnElectionAnalyzer.slash` 对低/零出块节点扣罚，若剩余质押仍达门槛则置 `LOCKED` 并保留质押（[OnElectionAnalyzer.java][onelection]）；`OnSettleAnalyzer` 在冻结周期结束后把 `LOCKED` 恢复为 `CANDIDATE` 并清理低出块计数（[OnSettleAnalyzer.java][onsettle]）。这是"保留质押、暂停参与、可恢复"的惩罚态，符合"当前有效质押身份"，但不是正常共识参与。

**Unknown（不构成有效性的新证据）**：

- 该关联从未成功观测到（首次 `error`/超时/未配置/不支持）；
- 只有 last-good 值而最近一次刷新失败 —— 标为 **Stale**，不得当作新鲜确认；
- 身份或状态无法建立：`nodeId` 与请求不匹配、响应畸形、状态缺失、`status==0` 且 `nodeId!=""`、出现未识别的整数；
- Node Validator Link 本身未建立（缺少可信完整公钥、Network Identity 不匹配、Network 未配置 Provider）。

**任何时候都不得把 locked/exiting 判成 NotValidator**：它们只在"已确认完成退出(5)"或"权威缺席(空形态)"时才是否定。

### 4.2 verifying(6) 的裁定与冲突标注

源码事实：`getCodeByStatus(CANDIDATE, isConsensus=1, *)` 返回 6，源码注释为"共识中"，即候选人在共识周期，且有在册质押。

**与既有权威定义的冲突**：CONTEXT.md 的 Current Validator Status 定义明确写 "verification in progress ... remain Unknown"；`docs/design/validator-metrics.md` 写 "verifying or inconclusive evidence is Unknown"；ADR 0005 把 "verifying, missing/conflicting evidence, and provider failures" 并列为不可作为否定。三处一致。因此本文**不**按源码事实把 6 改判为 Validator，而是保留 **Unknown**，并显式标注"源码语义（在册共识候选人）与目标定义（verification in progress = Unknown）不一致"（`docs/agents/domain.md` 要求 flag ADR conflicts）。实现 #173 必须按 Unknown 处理；若 Owner 决定改判，应先更新 CONTEXT.md 与 ADR 0005，而不是让实现猜测。

## 5. 链上有效性的一手证据

PlatON-Go `x/staking` 用**位掩码** `CandidateStatus`（[staking_types.go][staketypes]）：

~~~go
Invalided     CandidateStatus = 1 << iota // 0001: the current candidate withdraws from the staking qualification
LowRatio                                  // 0010: low package ratio AND no delete
NotEnough                                 // 0100: von below the minimum staking threshold
DuplicateSign                             // 1000
LowRatioDel                               // 0001,0000: lowRatio AND must delete
Withdrew                                  // 0010,0000: the Active withdrew
Valided       = 0                         // 0000: in force
~~~

`IsValid()` 只等价于"未置 `Invalided`"。

- **退出完成**（权威否定）：`WithdrewStaking` 清理份额并置 `can.Status |= staking.Invalided | staking.Withdrew`；若无可退还余额则 `DelCandidateStore`（[staking_plugin.go][stakeplugin]）。这与 browser-server 的 `EXITED(5)` 对应。
- **退出中仍有效**：`WithdrewStaking` 只在冻结期结束触发；触发前候选人未被置 `Invalided`。
- **惩罚锁定**：`handleSlashTypeFn` 对 `LowRatio` 追加 `Invalided` 并要求移出验证人列表（[staking_plugin.go][slashfn]）。这是**链上将候选人暂时移出共识、但保留质押并允许恢复**的路径，对应 browser-server 的 `LOCKED(7)` → `CANDIDATE(1)` 恢复。浏览器状态本身不暴露链上 `Invalided/LowRatio/NotEnough` 位掩码，因此 PlatPulse 只能以 browser-server 的展示语义为准；这是 §8 的主要限制。

## 6. 权威否定的可接受证据与排除项

**可接受（唯一两类）**

1. 新鲜成功、身份匹配的 `stakingDetails`：`code==0`、`data.nodeId == 请求的 130 字符 0x-hex`、`data.status == 5` → 已完成退出 → **NotValidator**。
2. 新鲜成功 `stakingDetails` 的严格空形态：`code==0`、`data.nodeId == ""`、`data.status == 0`（可同时要求其它默认值，但这两个字段已足够且与部署实测一致）→ 该部署的 `node` 表中无此身份 → **NotValidator（权威缺席）**。

**必须排除**

| 证据 | 处置 | 理由 |
| --- | --- | --- |
| HTTP `404` | 不得作为否定；降级为 Unknown | 缺席节点在部署合同里是 200 空对象（§3.4）；404 来自路由/网关/部署异常。当前实现把详情 404 归为 `NotFound`，应予修正 |
| HTTP `405`/`501` | `Unsupported` → Unknown | 端点不支持 |
| 传输失败/超时/其它 4xx、5xx | `Error` → Unknown，若有 last-good 则 Stale | 无证据 |
| 非 0 `code`、畸形/超长响应、`nodeId` 不匹配 | Unknown | 无法建立证据 |
| `aliveStakingList` 中无 ranking | 不构成否定 | 该查询排除 LOCKED 与非结算期 EXITING（§3.3）；unranked ≠ NotValidator |
| Network 未配置 Provider / 不支持 | `NotConfigured`/`Unsupported` → Unknown | 从未成功 → Unknown |

## 7. 可复用 fixture

目录：`crates/platpulse-server/tests/fixtures/platscan-validator-status/`（原始 UTF-8 响应体 + `provenance.json` 记录 URL、请求体、时间、HTTP 状态、Date、字节数、SHA-256，以及每个 detail fixture 的请求 nodeId、列表观测状态、详情观测状态）。

| fixture | 观测 |
| --- | --- |
| `staking-details-status-1.json` | candidate |
| `staking-details-status-2.json` | active |
| `staking-details-status-4.json` | exiting |
| `staking-details-status-5.json` | exited（否定） |
| `staking-details-status-6.json` | verifying |
| `staking-details-status-7.json` | locked |
| `staking-details-empty.json` | 严格空缺席形态（200/code 0/nodeId ""/status 0） |
| `staking-details-list-producer.json` | **列表报 3、详情返回 6** 的差异证据（与下方 `alive-staking-list-producer.json` 同一次生产） |
| `alive-staking-list-page-1.json` … `page-5.json` | 完整 `queryStatus=all` 队列（5 页，状态分布可复现） |
| `alive-staking-list-producer.json` | 含 status 3 的列表页，与上方 `staking-details-list-producer.json` 成对 |
| `locked-staking-list-page-1.json` | `LOCKED` 队列（status 7） |
| `history-staking-list-page-1.json` | `EXITING`/`EXITED` 队列（status 4/5） |
| `provenance.json` | 全部抓取 provenance 与当时的状态分布 |

使用方式：实现（#173）与测试直接回放这些原始响应，**CI 不访问实时 PlatScan**；fixture 只代表抓取时刻的部署行为。原始第三方节点名/描述按字节保真保留，测试不得抓取其中 URL。

## 8. 限制与非断言

- **不验证旧 Activity 映射**：本文只确立"展示码 → 当前有效质押身份"的谓词；不主张现有整数→`ValidatorActivity` 映射已经证明正确，也不把旧状态分类直接当作当前有效性。
- **部署 revision 未知**：链语义来自 2026-09-03 的 `develop` 快照，浏览器语义固定在 2024-03-20 的提交；部署实际版本未知。状态语义可能随上游版本变化。
- **展示层有损**：browser-server 不暴露链上 `Invalided/LowRatio/NotEnough/DuplicateSign` 位掩码。`locked`/`exiting` 的"有效"以 browser-server 自身状态机与链上退出/恢复路径为依据，不能据此声称它们是正常共识参与者，也不构成所有权或历史出块的证明。
- **空缺席 = 部署索引视角**：`nodeId==""` 只说明该 nodeId 不在该部署当前的 `node` 表中；理论上仍可能受上游索引滞后/回填影响。设计把它作为权威缺席接受，本文记录该假设而不扩大结论。
- **无实时依赖**：本文所有实测仅用于一次性取证并落盘为 fixture；CI 与运行时不依赖 PlatScan 可用性。
- **否定证据只来自 §6 的两类**：排除了 HTTP 404、传输失败与 ranking 缺失后，实现侧应把详情 `404` 从 `NotFound` 降级为不可否定的 Unknown，把 `status==0 && nodeId!=""` 与未识别整数视为 Unknown，并记住 `3 producing` 只可能来自列表；**不得**由 ranking 缺失推导 NotValidator。以上是 #168 排除项在实现中的落点，具体改动属 #173，不属本次。
- **空缺席用一个合成 id 取证**：`0xffff…` 是合法 130 字符但不在部署 `node` 表中的标识；没有用"真实曾存在而后被清除"的 id 单独取证。空形态判定由源码推导 + 该实测共同支持。

[customstaking]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-common/src/main/java/com/platon/browser/bean/CustomStaking.java#L144-L166
[statusenum]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-service/src/main/java/com/platon/browser/enums/StakingStatusEnum.java#L12-L69
[statustest]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-service/src/test/java/com/platon/browser/enums/StakingStatusEnumTest.java#L9-L18
[stakingservice]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-api/src/main/java/com/platon/browser/service/StakingService.java#L117-L215
[stakeexit]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-agent/src/main/java/com/platon/browser/analyzer/ppos/StakeExitAnalyzer.java#L95-L115
[onelection]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-agent/src/main/java/com/platon/browser/analyzer/epoch/OnElectionAnalyzer.java
[onsettle]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-agent/src/main/java/com/platon/browser/analyzer/epoch/OnSettleAnalyzer.java#L83-L120
[modifier]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-api/src/main/java/com/platon/browser/serializer/CustomBeanSerializerModifier.java#L33-L104
[nullstr]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-api/src/main/java/com/platon/browser/serializer/NullStringJsonSerializer.java#L16-L25
[nullint]: https://github.com/PlatONnetwork/browser-server/blob/6daa5ad2e878474869407314a73a219ac49c8b51/scan-api/src/main/java/com/platon/browser/serializer/NullIntegerJsonSerializer.java#L16-L26
[staketypes]: https://github.com/PlatONnetwork/PlatON-Go/blob/8bc8bddec1aa5771e59173f37ebd3a12258c3c95/x/staking/staking_types.go#L34-L125
[stakeplugin]: https://github.com/PlatONnetwork/PlatON-Go/blob/8bc8bddec1aa5771e59173f37ebd3a12258c3c95/x/plugin/staking_plugin.go#L646-L720
[slashfn]: https://github.com/PlatONnetwork/PlatON-Go/blob/8bc8bddec1aa5771e59173f37ebd3a12258c3c95/x/plugin/staking_plugin.go#L2605-L2625
[emptyfixture]: ../../crates/platpulse-server/tests/fixtures/platscan-validator-status/staking-details-empty.json
[prov]: ../../crates/platpulse-server/tests/fixtures/platscan-validator-status/provenance.json
