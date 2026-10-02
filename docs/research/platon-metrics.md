# Research: PlatON-Go metrics 能提供什么数据，以及 PlatPulse 应直连还是经 InfluxDB

> 调研日期：2026-10-02（UTC）。只研究，不实施、不修改设计或 Issue。
> 范围：本地 PlatON-Go 检出 `/home/mowind/repos/go/PlatON-Go`，HEAD `8bc8bddec1aa5771e59173f37ebd3a12258c3c95`（2026-09-03，Merge PR #2368）。该仓库是 go-ethereum 派生分支（`metrics/FORK.md` 记录自 rcrowley/go-metrics 派生）。
> 背景：PlatPulse issue #227《feat: 支持从 PlatON metrics 获取节点数据》（0.3.0 里程碑）。本文只回答两个问题：**能取到什么数据**，以及 **Agent 应直连 platond 的 metrics 接口还是读 InfluxDB**。
> 已阅读 PlatPulse 根 `AGENTS.md`、`CONTEXT.md`，沿用 `docs/research/` 既有笔记约定。本文不是 ADR 或实施规格。
> **版本边界：** 以下结论只对上述固定提交负责。更早的 PlatON 版本是否支持 `--metrics.addr` 独立 HTTP 端点未逐一核验，见文末「未核验项」。

## TL;DR / 直接结论

1. **能取到的数据量很大但范围明确。** PlatON-Go 暴露的是 go-ethereum 风格的运行时/节点自监控指标：`system/*`（CPU、内存、进程、磁盘 IO）、`p2p/*` 与 `p2p/flow/*`（连接、各协议消息的包数/流量）、`cbft/*`（出块、视图/epoch、proposer index、validator count、QC/locked/commit 高度、共识网络收发）、`rpc/*`、`txpool/*`、`eth/downloader/*`、`eth/fetcher/*`、`trie/*`、`ethdb/*`、`snapshotdb/*`、`chain/*`、`state/snapshot/*`。指标类型是 go-metrics 的 counter / gauge / gaugeFloat64 / meter / histogram / timer / resettingTimer。
2. **取不到身份、奖励、质押语义的数据。** `x/`（ppos/staking/reward/slashing/gov/restricting/plugin）目录下**零 metrics 注册**。因此 metrics **不能**提供 Validator 身份、出块奖励、累计出块归属、质押/治理数据——这与 issue #227「不以 metrics 推算 Block Summary、Validator 身份或累计出块/奖励」完全一致，不是 PlatPulse 实现取舍，而是上游根本没有这些指标。
3. **有两条可选通路：(A) 直连 platond 的 HTTP metrics 端点；(B) 让 platond 推送到 InfluxDB，Agent 再查 InfluxDB。**
4. **推荐 (A) 直连抓取，不默认引入 InfluxDB。** 理由：PlatPulse 约束「每个 Node 独立配置采集」「不默认依赖外部 Prometheus 服务」；直连不新增被监控侧服务与凭证、失败可明确归类为不可达/超时/解析失败、与 last-good 语义天然契合；InfluxDB 会引入额外的服务依赖、鉴权、网络路径、聚合延迟与第二套 counter 语义。InfluxDB 最多作为未来可选扩展，不应进入 0.3.0 的必经路径。
5. **一个关键坑：只加 `--metrics` 不会开启任何 HTTP 端点。** 必须显式设置 `--metrics.addr`（端口由 `--metrics.port` 决定，默认 6060）才会起独立 metrics HTTP 服务。未启用要作为「未启用/不支持」的显式状态，绝不能当成 0 或 Healthy。
6. **Prometheus 输出没有时间戳，也拿不到 meter 的 EWMA 速率。** 直连时这些要在 Agent 侧自算/打采样时间戳，且必须显式处理 counter 回退（进程重启/重置）。

## 一、如何启用与暴露方式

### 1.1 CLI 开关

| 开关 | 作用 | 源码 |
| --- | --- | --- |
| `--metrics` | 打开指标采集总开关 | `cmd/utils/flags.go:712-716` |
| `--metrics.expensive` | 打开高开销指标（state trie 等） | `cmd/utils/flags.go:717-721` |
| `--metrics.addr` | 独立 metrics HTTP server 监听地址；**不设则不起服务** | `cmd/utils/flags.go:722-730` |
| `--metrics.port` | 独立 metrics HTTP 端口，默认 6060 | `cmd/utils/flags.go:731-736`；`metrics/config.go:35-45` |
| `--metrics.influxdb` (+ endpoint/database/username/password/tags) | 推送 InfluxDB v1 | `cmd/utils/flags.go:739-778` |
| `--metrics.influxdbv2` (+ token/bucket/organization) | 推送 InfluxDB v2 | `cmd/utils/flags.go:779-809` |

`metrics.Enabled` 由一个 `init()` **直接扫描 os.Args** 中精确等于 `metrics` 的参数来置位（`metrics/metrics.go:23-54`，enablerFlags=`["metrics"]`，expensiveEnablerFlags=`["metrics.expensive"]`）。这意味着指标注册发生在参数解析之前，采集行为取决于命令行而非 TOML 的 `Metrics.Enabled` 字段；虽然 `metrics.Config`（`metrics/config.go:19-34`）存在且默认关闭，实际 gate 以 os.Args 为准。

启动流程：`cmd/platon/main.go:258-264` 在 `app.Before` 调用 `utils.SetupMetrics(ctx)`，随后 `go metrics.CollectProcessMetrics(3*time.Second)` 每 3 秒采集一次进程/运行时指标。

### 1.2 直接 HTTP 端点（通路 A）

`utils.SetupMetrics` 仅当 `--metrics.addr` 被显式设置时调用 `exp.Setup(address)`（`cmd/utils/flags.go:1712-1720`）。`metrics/exp/exp.go:58-66` 起一个独立 `http.ServeMux`，暴露两个路径：

| 路径 | 格式 | 源码 |
| --- | --- | --- |
| `/debug/metrics` | JSON（expvar 风格，模拟标准 `expvar` handler 输出） | `metrics/exp/exp.go:58-61` |
| `/debug/metrics/prometheus` | Prometheus/OpenMetrics 文本 | `metrics/exp/exp.go:61`；`metrics/prometheus/prometheus.go:30-63` |

地址形如 `http://<metrics.addr>:<metrics.port>/debug/metrics/prometheus`。

另有一条与 pprof 共用的路径：`internal/debug/flags.go:213-221` 在 `--pprof` 且 **未设置 `--metrics.addr`** 时调用 `StartPProf(address, !ctx.IsSet("metrics.addr"))`，由 `StartPProf`（`internal/debug/flags.go:248-256`）在 pprof server 上挂载同样的 `/debug/metrics` 与 `/debug/metrics/prometheus`（`exp.Exp`，`metrics/exp/exp.go:42-48`）。即：

- 设了 `--metrics.addr` → 独立 metrics 服务承载指标；pprof 不再重复挂载。
- 只开 `--pprof`（未设 `--metrics.addr`）→ 指标挂在 pprof 端口。
- 两个都没设 → **没有任何 HTTP 指标端点**，即使采集在内存里进行。

> 安全含义：这两个端点都是**明文 HTTP、无鉴权、无 TLS**（`http.ListenAndServe`）。独立 metrics 端点注释明确说它是为了避开 pprof 的敏感面而设计的「public-OK」端点（`cmd/utils/flags.go:722-724`）。PlatPulse Agent 应固定抓 loopback（127.0.0.1），不要把该端口暴露到公网。

### 1.3 InfluxDB 推送（通路 B）

`utils.SetupMetrics` 里 v1/v2 二选一（`CheckExclusive`，混用错误 flag 会 `Fatalf`，`cmd/utils/flags.go:1665-1697`）：

- v1：`go influxdb.InfluxDBWithTags(metrics.DefaultRegistry, 10*time.Second, endpoint, database, username, password, "platon.", tagsMap)`（`cmd/utils/flags.go:1700-1703`）
- v2：`go influxdb.InfluxDBV2WithTags(..., "platon.", tagsMap)`（`cmd/utils/flags.go:1704-1710`）

两者都是 **每 10 秒推送一次**，namespace 前缀 `platon.`。推送失败只 `log.Warn` 并重试（`metrics/influxdb/influxdb.go:108-128`，v2 每 5 秒 health check 重建 client），不阻塞节点。

## 二、数据模型：类型、字段与两条通路的差异

go-metrics 的 7 种注册类型（`metrics/*.go`）：Counter、Gauge、GaugeFloat64、Meter、Histogram、Timer、ResettingTimer。**Meter 内部维护累计 count + 1/5/15 分钟 EWMA 速率 + mean rate；Histogram/Timer 维护分位数。**

### 2.1 Prometheus 文本端点（推荐通路）的映射

`metrics/prometheus/collector.go`：

- 名称转换：仅把 `/` 替换为 `_`（`mutateKey`，`metrics/prometheus/collector.go:118-120`）。例如 `cbft/gauage/block/mined` → `cbft_gauage_block_mined`。
- Counter / Gauge / GaugeFloat64 / **Meter** 全部走 `writeGaugeCounter`，输出 `# TYPE <name> gauge` + 单值（`collector.go:50-62, 98-102`）。**Meter 只输出累计 count，不输出 m1/m5/m15 速率。**
- Histogram / Timer：输出 `# TYPE <name>_count counter` + count，再输出 `# TYPE <name> summary` + 分位点（0.5/0.75/0.95/0.99/0.999/0.9999）（`collector.go:64-88`）。
- ResettingTimer：输出 0.50/0.95/0.99 的 summary（`collector.go:89-96`）。
- **没有时间戳行**——抓取方需自行以采集时刻打点。

### 2.2 expvar JSON 端点

`metrics/exp/exp.go:66-140` 起：counter/gauge 为 int，gaugeFloat64 为 float；histogram/timer 展开为 `<name>.count`、`<name>.min`、`<name>.max`、`<name>.mean`、`<name>.std-dev`、`<name>.50-percentile` 等（要点分名）。适合不需要 Prometheus 文本解析的场景，但结构与 Prometheus 端点不同。

### 2.3 InfluxDB 端点

- measurement 名 = `platon.` + 指标名 + 类型后缀：`.count`、`.gauge`、`.histogram`、`.meter`、`.timer`、`.span`（`metrics/influxdb/influxdb.go:126-236`）。
- v1：Counter 的 field `value` = **累计值**（`influxdb.go:134-141`）。
- v2：Counter 的 field `value` = `metric.Count() - cache[name]`，即**两次推送之间的增量**（`metrics/influxdb/influxdbv2.go:92-107`），并把当前值写回 cache。**进程重启后 cache 归零，首个采样会推出一个近似「全量」的巨大增量；若发生 counter 回退则可能为负。** 这是两条通路最重要的语义分歧。
- Meter 在 v1/v2 里额外带 m1/m5/m15/mean 字段。
- tags 来自 `--metrics.influxdb.tags`（如 `host=localhost`），PlatPulse 可据此区分节点，但更可靠的是每个 Node 独立 endpoint/database。

## 三、可用指标清单（按命名空间）

以下均为源码实际注册名（`/` 在 Prometheus 输出中转为 `_`）。并非穷举到单条，按族给出代表性指标与源码位置。

| 命名空间 | 代表指标（类型） | 源码 |
| --- | --- | --- |
| `system/cpu/*` | sysload、syswait、procload、threads、goroutines（gauge）；schedlatency（runtime histogram） | `metrics/metrics.go:146-151` |
| `system/memory/*` | held、used、objects（gauge）；allocs、frees（meter）；pauses（histogram） | `metrics/metrics.go:152-157` |
| `system/disk/*` | readcount、readdata、writecount、writedata（meter）；readbytes、writebytes（counter） | `metrics/metrics.go:158-163` |
| `p2p/*` | peers（gauge）；serves、dials、ingress、egress（meter）；`p2p/handle*`（histogram） | `p2p/metrics.go:32-43` |
| `p2p/flow/eth/*`、`p2p/flow/cbft/*` | 各协议消息 OutboundTraffic（meter） | `common/flow_metrics.go:9-38` |
| `cbft/gauage/*` | block/mined、block/qc_collected、view/number、epoch/number、proposer/index、validator/count、block/number、block/qc/number、block/locked/number、block/commit/number | `consensus/cbft/metrics.go:23-44` |
| `cbft/meter/*` | block/produce、block/check_failure、signature/check_failure、block/confirmed | `consensus/cbft/metrics.go:28-31` |
| `cbft/counter/*` | view/count、consensus/count、mined/count | `consensus/cbft/metrics.go:33-35` |
| `cbft/req/*`、`cbft/prop/*`、`cbft/misc/*` | 各共识消息 in/out packets、traffic（meter） | `consensus/cbft/network/metrics.go:29-98` |
| `rpc/*` | requests、success、failure（gauge）；duration/all（timer）；`rpc/duration/<method>/<success 或 failure>`（lazy histogram） | `rpc/metrics.go:27-45` |
| `txpool/*` | pending/queued 各类丢弃计数（meter）；pending、queued、local、slots（gauge） | `core/txpool/txpool.go:107-137` |
| `eth/downloader/*`、`eth/fetcher/*` | 区块/交易/回执的 in、drop、timeout 等（meter、gauge） | `eth/downloader/metrics.go`、`eth/fetcher/` 各文件 |
| `trie/memcache/*` | clean/dirty 命中、读写、flush、gc（meter） | `trie/database.go:45-65` |
| `ethdb/leveldb/*`、`ethdb/pebble/*` | compact/time、compact/input、compact/output、disk/size、disk/read、disk/write、memory/*（meter、gauge） | `ethdb/leveldb/leveldb.go:137-149`、`ethdb/pebble/pebble.go:243-255` |
| `snapshotdb/*` | basedb/size、fork（gauge） | `core/snapshotdb/metrics.go:27-28` |
| `chain/*` | head/block、head/header、head/receipt（gauge）；account、storage 各 read/hash/update/commit（timer）；snapshot/*；triedb/commits；reorg/add、reorg/drop（meter） | `core/blockchain.go:55-82` |
| `state/snapshot/*` | clean/dirty × account/storage 的 hit/miss/read/write、flush、bloom（meter/gauge） | `core/state/snapshot/snapshot.go:37-77` |
| `cbft/gauage/block/executed` | 已执行块（gauge） | `core/blockchain_cache.go:41` |
| `db/preimage/*` | preimage 读写（meter） | `core/rawdb`（grep `db/preimage`） |

`--metrics.expensive` 额外 gate 一批 state 深度指标（如 `core/state/statedb.go:612`、`core/state/state_object.go:242-433` 中的 trie/state 更新计时），未开启时这些不注册、端点里也不出现。

### 3.1 明确不可得的类别

- **Validator 身份、签名者、proposer 地址**：无。
- **出块奖励、质押、治理、经济模型**：无（`x/` 全目录零注册，已用 `grep NewRegistered --include=*.go x/` 验证为空）。
- **Block Summary / 交易明细 / 账户余额**：无。
- 这些只能继续走现有 RPC 采集（Head Subscription / Block Resolution），与 #227 的约束一致。

### 3.2 与 RPC 重叠的指标

`cbft/gauage/block/number`、`chain/head/block`、`p2p/peers`、`txpool/*` 与 RPC 可取得的链/网络状态语义重叠。**应按 issue 要求明确来源与优先级**：链高度、peer 身份等「链事实」仍以 RPC/订阅为权威，metrics 仅作运行时可观测性补充，不可静默覆盖。

## 四、直连（A）vs InfluxDB（B）

| 维度 | A. Agent 直连 platond HTTP | B. platond → InfluxDB → Agent |
| --- | --- | --- |
| 被监控侧新增依赖 | 无（只用节点自身端点） | 需要部署/维护 InfluxDB 服务、库、账号、网络策略 |
| 与 PlatPulse 约束 | 契合「每 Node 独立配置采集」「不默认依赖外部服务」 | 强制外部服务，破坏默认部署假设 |
| 采集粒度/新鲜度 | 由 Agent 抓取频率决定，可贴近采样周期 | 固定 10s 推送 + 查询延迟 |
| 失败状态 | 可区分「未启用/不可达/超时/非 200/解析失败」 | 只能看到 InfluxDB 查询结果，节点侧失败与 InfluxDB 故障难区分 |
| last-good 语义 | 每次抓取即一份快照，天然适配 | 需额外判断数据是否陈旧 |
| 鉴权/TLS | 明文、无鉴权，但可限制 loopback | 需管理 InfluxDB 凭证（v1 用户名密码 / v2 token） |
| counter 语义 | 累计值，Agent 统一处理回退 | v1 累计、v2 增量，两套语义且 v2 重启首采样异常 |
| 版本漂移面 | 只需适配 Prometheus/expvar 文本 | 还要适配 measurement 命名与 InfluxDB v1/v2 schema |
| 速率（m1/m5/m15） | **拿不到**，需 Agent 自算 | v1/v2 直接带 in-process 速率 |

## 五、建议

1. **0.3.0 采用通路 A：Agent 直连 PlatON 节点的 metrics HTTP 端点**，默认抓 `/debug/metrics/prometheus`（Prometheus 文本，生态成熟、解析库多、名称稳定 `/`→`_`）。JSON 端点可作为备用/调试，不作为首选协议。
2. **不引入 InfluxDB 作为默认或必需依赖。** 它增加服务、凭证、网络与第二套 counter 语义，与 PlatPulse 的部署姿态和 #227 的「不默认依赖外部 Prometheus 服务」精神相悖。若未来确有历史留存/集中查询需求，可作为**可选适配器**另开 issue，不与直连互斥。
3. **配置与状态**：每个 Node 独立配置 metrics endpoint（默认 `http://127.0.0.1:6060/debug/metrics/prometheus`）、超时与响应体上限。必须把「节点未启用 metrics（无 `--metrics.addr`）」「端点不可达」「超时」「非 200」「解析失败」「指标缺失」区分为不同状态，保留 last-good，绝不以 0/false/Healthy 代替未知。
4. **counter 处理**：Prometheus 端点是累计值。Agent 需自行差分并**在值下降（进程重启/重置）时显式处理**，不跨无效区间伪造增长；这与 #227 验收方向一致。若需要速率，由 Agent 基于相邻采样按时间窗计算，不得把 meter 的 count 当成速率。
5. **安全**：端点明文无鉴权，默认只抓 loopback；不把 metrics 端口暴露公网，不把 endpoint/凭证/敏感 tag 透传到 Public Projection。
6. **版本差异**：`--metrics.addr` 独立端点是较新 go-ethereum 特性。对更早 PlatON 版本，指标可能只在 `--pprof` 端口暴露，或格式不同。以上结论只对固定 HEAD 负责；接入前需按目标版本实测。

## 六、运行时实测（本地节点，2026-10-02）

在本地运行的 platond 上实测。节点命令行（未开 `--metrics.expensive`）：

```
platon --identity platon-test --datadir /data/platon-node/data --port 16790 \
  --http.port 6789 --http --ws --syncmode snap --db.nogc \
  --metrics --metrics.addr 127.0.0.1
```

| 项 | 实测值 |
| --- | --- |
| 端口 | `127.0.0.1:6060`（独立 metrics server；另有 http 6789、p2p 16790） |
| `/debug/metrics/prometheus` | HTTP 200，`text/plain`，58,857 B，~1.3 ms；1904 行、535 个 `# TYPE` 族、1369 条样本 |
| `/debug/metrics`（JSON） | HTTP 200，`application/json`，137,690 B，~39 ms；2497 个顶层键 |
| JSON 命名空间键数 | p2p 1011、eth 324、cbft 284、state 281、chain 209、rpc 117、txpool 117、trie 88、system 60、snapshotdb 2、db 2；另有 expvar 的 `cmdline`、`memstats`（非 go-metrics 指标） |
| 抓取时链高 | `chain/head/block` = 160,190,984；`p2p/peers` = 28 |
| 共识 | `cbft/gauage/view/number`=7、`epoch/number`=372538、`block/commit/number`=160190984、`block/mined`=0、`validator/count`=0、`proposer/index`=0 |
| 交易池 | `txpool/pending`=0、`queued`=0、`valid`=20 |

运行时确认的源码结论：

1. **Prometheus 输出无时间戳**：非注释行只有「name value」或「name{labels} value」，无第三列。
2. **Meter 输出为 gauge + 累计 count**：`p2p_serves`、`p2p_egress`、`p2p_dials`、`cbft_meter_block_produce`、`system_memory_allocs` 均声明为 `gauge` 单值；全文件 `m1|m5|m15|mean` 出现 0 次。
3. **counter 确为累计值**：相隔 20 秒两次抓取，`chain_head_block` 160190984→160191025、`p2p_egress` 2607770→4067850、`rpc_requests` 730→1176、`cbft_meter_block_confirmed` 73→114。速率必须 Agent 自算。
4. **`--metrics.expensive` 门控生效**：未开启时 `# TYPE state_*` 下无 state 深度/trie 更新类指标。
5. **端点为明文无鉴权 HTTP**，直接返回 200，无需凭证。

> **对 PlatPulse 的实证警示：** 本次实测中 `validator/count=0`、`proposer/index=0`、`block/mined=0`，而该节点是 snap sync 的跟随节点。这正是「0 可能只是未填充/不适用」的真实例子——绝不能把 0 当成 Healthy 或「无验证者」，必须按未知/last-good 处理。

原始抓取样例已存为 [docs/research/platon-metrics-sample.prom.txt](platon-metrics-sample.prom.txt)（535 族，可作解析器 fixture）。

### 6.1 验证者节点实测（54.37.253.62:6063，2026-10-02）

对一台带 BLS 密钥的共识节点抓取（identity `hydra`，datadir `/opt/platon-hydra/data`，`--metrics.addr 0.0.0.0 --metrics.port 6063`）：

| 项 | 实测值 |
| --- | --- |
| `/debug/metrics/prometheus` | HTTP 200，text/plain，54,410 B，~0.41 s（跨公网）；1795 行、507 族 |
| `/debug/metrics`（JSON） | HTTP 200，129,640 B；2365 个顶层键 |
| cmdline（expvar） | `platon --identity hydra ... --metrics --metrics.addr 0.0.0.0 --metrics.port 6063` |

与本地跟随节点对比：

| 指标 | 本地跟随节点 | 公网验证者节点 |
| --- | --- | --- |
| `chain/head/block` | 160190984 | 160191321 |
| `p2p/peers` | 28 | 26 |
| `cbft/gauage/view/number` | 7 | 41 |
| `cbft/gauage/epoch/number` | 372538 | 372538 |
| `cbft/gauage/block/locked/number` | 160190985 | 160191322 |
| `cbft/gauage/proposer/index` | 0 | **0** |
| `cbft/gauage/validator/count` | 0 | **0** |
| `cbft/gauage/block/mined` | 0 | **0** |
| `cbft/gauage/block/qc_collected` | 0 | **0** |
| `cbft/counter/mined/count` | 0 | **0** |
| `cbft/meter/block/produce` | 0 | **0** |
| `cbft/meter/block/confirmed` | 73 | 76 |
| `txpool/pending` · `queued` · `valid` | 0 · 0 · 20 | 0 · 0 · 5 |

**重要语义修正（源码确认）：这些 `cbft/gauage/*` 不是全网状态，而是「本节点自身上一次动作」，且大多只在出块路径上写入：**

- `cbft/gauage/proposer/index`、`cbft/gauage/validator/count`：只在 `consensus/cbft/cbft.go:1303-1304` 的 sealing 路径写入，即**仅当本节点是当前 proposer 且准备成功出块**时才更新；否则保持上次值（进程内从未出块则恒为 0）。
- `cbft/gauage/block/mined`：`consensus/cbft/cbft.go:921` 写入的是 `common.Millis(time.Now()) - int64(preBlock.Time())`，即**「距父块的时间间隔（ms）」而非出块计数**；计数是 `cbft/counter/mined/count`（`consensus/cbft/cbft.go:918`）。
- `cbft/gauage/block/qc_collected`：`consensus/cbft/consensus_process.go:503` 写入 `int64(block.Time())`，是一个**区块时间戳**。

对验证者节点连续 8 次、约 32 秒轮询，`proposer/index`、`validator/count`、`block/mined`、`qc_collected`、`mined/count`、`block/produce` **始终为 0**，而 `block/confirmed` 由 109 增至 138、区块高度持续推进。→ **绝不能用这些指标判断验证者身份、验证者数量或出块归属；0 只代表「本节点自进程启动以来还没轮到/没做过」，不代表「网络没有验证者」。** 这与 issue #227 的约束一致。

> **暴露面提醒：** 该验证者节点把 `--metrics.addr` 设为 `0.0.0.0`，端点直接暴露在公网（明文、无鉴权）。PlatPulse 采集侧不应复制这种配置，默认只绑 loopback 或走内网。

验证者节点原始样例已存为 [docs/research/platon-metrics-sample-validator.prom.txt](platon-metrics-sample-validator.prom.txt)（507 族）。

## 七、未核验项

- **运行时抓取**已完成（第六节）：本地跟随节点 + 一台公网验证者节点。仍未覆盖：多验证者节点横向对比、`--metrics.expensive` 开启后的完整指标集、验证者身份/奖励类指标最终缺失的确认。
- 更早 PlatON 版本（尤其引入 `--metrics.addr` 之前）的实际暴露路径与指标集合。
- 官方 devdocs 是否已有 metrics 启用说明的中文文档（本次未引用官方文档，全部以本地源码为准）。

## 主要源码索引

- `cmd/utils/flags.go:712-809`（metrics 全部 CLI flag）、`cmd/utils/flags.go:1660-1730`（SetupMetrics）
- `cmd/platon/main.go:159-175`（metricsFlags）、`cmd/platon/main.go:258-264`（启动采集）
- `metrics/metrics.go:23-54`（enabler）、`metrics/metrics.go:123-163`（CollectProcessMetrics）
- `metrics/config.go:19-45`（Config/DefaultConfig）
- `metrics/exp/exp.go:42-66`（HTTP 端点 `/debug/metrics`、`/debug/metrics/prometheus`）
- `internal/debug/flags.go:213-221, 248-256`（pprof 挂载 metrics 的条件）
- `metrics/prometheus/prometheus.go:30-63`、`metrics/prometheus/collector.go:29-120`（Prometheus 格式与命名）
- `metrics/influxdb/influxdb.go:36-141`（v1，累计 counter）、`metrics/influxdb/influxdbv2.go:39-107`（v2，增量 counter）
- 指标注册：`consensus/cbft/metrics.go`、`consensus/cbft/network/metrics.go`、`p2p/metrics.go`、`common/flow_metrics.go`、`rpc/metrics.go`、`core/txpool/txpool.go`、`core/blockchain.go`、`core/state/snapshot/snapshot.go`、`core/snapshotdb/metrics.go`、`trie/database.go`、`ethdb/leveldb/leveldb.go`、`ethdb/pebble/pebble.go`
