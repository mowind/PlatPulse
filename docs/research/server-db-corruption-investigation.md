# PlatPulse 现场库物理损坏调查（#196）

调查日期：2026-10-01（首次现场损坏 2026-09-17，issue 报告 2026-09-23）。用于 #196「查清 2026-09-23 现场库损坏的来源；与 #194 的监控语义修复分开跟踪」。

本文只记录事实、证据与结论，不改动运行时行为，不扩大改动范围。所有结论以一手来源为准：SQLite 官方变更日志与文档、systemd journal 现场记录、损坏产物的原始页字节。**未能证实之处一律标注为限制，不以推测代替事实。**

## 1. 结论摘要

**主因（最高置信假设）：SQLite「WAL-reset bug」（WAL 重置竞态）。**

- 官方说明：该 bug「likely present in all version of SQLite from 3.7.0 (2010-07-21) through 3.51.2 (2026-01-09)」，修复于 **3.51.3（2026-03-13）**，并为旧分支提供 backport **3.44.6 / 3.50.7**（[sqlite.org/changes.html](https://sqlite.org/changes.html)、[sqlite.org/draft/wal.html §11](https://sqlite.org/draft/wal.html)）。
- 现场 Server 内置 SQLite 为 **3.46.0**（`Cargo.lock`：`libsqlite3-sys 0.30.1`、`sqlx 0.8.6`；`strings target/release/platpulse-server` 输出 `3.46.0`），处于受影响区间。
- 触发条件：WAL 模式下，**同一文件上 ≥2 个连接（不同线程或进程）同时尝试写入或 checkpoint**；应用主动/激进地手动 checkpoint 会显著抬高中招概率。
- 现场架构吻合度极高：WAL + `locking_mode=Exclusive`，且 **2026-09-18 起存在第二个活库连接**（运行时完整性监控 + 定时在线备份 `VACUUM INTO`），并在每次关闭时执行 `PRAGMA wal_checkpoint(TRUNCATE)`。
- 损坏形态吻合：受影响的页是「刚写入、结构合法、但未挂入任何 B-tree、也不在 freelist」的叶页（事务的部分页持久化），并表现为叶页被**双链**（同一 rowid 出现两次）、`2nd reference to page`、`Rowid ... out of order`。
- 首次报错的扩展码吻合：2026-09-17 17:47:56 `(code: 779)` = `SQLITE_CORRUPT_INDEX`（索引损坏），随后 `(code: 11)` = `SQLITE_CORRUPT`。
- 真实世界先例：Tailscale 控制面因同一 bug 在六个月内发生 **19 次库损坏**；其自述被击中的原因正是「手动接管 checkpoint，且 checkpoint 非常激进」（[tailscale.com/blog/sqlite-wal-reset-bug](https://tailscale.com/blog/sqlite-wal-reset-bug)）。Phil Eaton 于 2026-08-23 给出不依赖 SQLite 内部测试钩子的公开复现（[theconsensus.dev](https://theconsensus.dev/p/2026/08/23/another-look-at-sqlite-wal-reset.html)）。

**限制（必须保留）**：WAL-reset bug 需要 ≥2 个并发连接，而 09-17 当天运行的提交 `1076718` 尚无运行时完整性监控，连接池 `max_connections = min_connections = 1` 且 `locking_mode=Exclusive` 使语句串行。因此 **09-17 事件的「≥2 连接」前提尚未被现场证据证实**；09-17 与 09-23 是否同源，仍需产物比对或隔离复现坐实。09-18 及之后的构建（`abe342f`）同时具备运行时监控与定时 `VACUUM INTO`，触发面明显扩大。

**2026-10-10 更新（现场库静止态判定）：现场库本身不处于损坏状态。** 停机后对副本用两个独立引擎复核：系统 `sqlite3 3.53.4`（`immutable=1`）`PRAGMA integrity_check(1)` → `ok`（54s）；新构建的 `platpulse-server verify-integrity`（内置 SQLite 3.53.2）→ `SQLite integrity check: ok`，exit 0（63s）。同一份库在**运行中**用 `immutable=1` 打开却报 `malformed`（`Freelist: freelist leaf count too big on page 3819225`）——差异来自 `-wal` 未回填（`immutable=1` 忽略 `-wal`，读到陈旧主文件视图）。因此 2026-10-09/10 期间在活库上观察到的 `malformed`/`btreeInitPage() returns error code 11` 属**外部只读打开造成的伪影，不是盘上损坏**；09-17 的两份原始产物在静止态复核仍为真损坏。⇒ 现场库**无需修复/重建**，只需按 §7 建议 1/2 消除触发面。详见 §11。

**已排除**：介质/文件系统故障；09-17 首次报错前的外部进程；09-23 的 gdb 注入式 abort。详见 §4。

## 2. 事件与时间线

### 2.1 2026-09-17（首次现场损坏）

| 时间（本地） | 事件 |
| --- | --- |
| 17:19:34 | 部署重启：停止 + 启动 `platpulse-server`（commit `1076718`，2026-09-17 17:18:01）。旧进程 Consumed `1h 11min 12.541s` CPU / `22h 38min` wall，峰值 243.7M |
| 17:19:38 | 新进程 `platpulse-server[1514740] 0.1.0` 监听 `127.0.0.1:8080` |
| 17:47:04–05 | 从损坏叶页恢复的行时间戳（见 §3.1）：损坏涉及的数据在**实时摄取过程中写入**；行时间戳只定位数据时间，不等于损坏时刻 |
| 17:47:56.141287 | 首条损坏错误：`raw block retention cleanup deferred after ingestion: error returned from database: (code: 779) database disk image is malformed`（每秒重复） |
| 17:48:08 起 | `save_current error: error returned from database: (code: 11) database disk image is malformed`（每秒重复）；Agent 随后记录 `server returned HTTP 503` |
| 17:51–18:14 | 人工多次停启（17:51:57/17:52:57、17:54:04/17:55:29、18:09:16/18:09:21、18:13:14/18:14:07、18:14:52/18:14:56） |

关键事实：**17:19:34 部署到 17:47:56 首次报错之间，journal 中只有 server/agent 自身日志、一条 17:34:12 的 SSH postauth 会话，以及无关的 agent ping/IP 日志；没有任何外部 sqlite3、备份、gdb 或拷贝进程。** 当日 sudo 仅 `smartctl -H /dev/nvme0n1`(17:49)、`ss -ltnp`(18:19)、`supervisorctl`(18:26)，均发生在损坏之后。损坏页行时间为 17:47:04–05，距首条报错约 **51 秒**，期间无重启、无 checkpoint。

### 2.2 2026-09-23（issue 报告的事件）

| 时间（本地） | 事件 |
| --- | --- |
| 12:30:46–12:31:08 | 取证 gdb attach：`gdb -p 3380751 ... info threads`、`call (int)malloc_info(0, ...)` / `fclose()` |
| 12:31:08.606492 | `platpulse-server[3380751]: Fatal error: glibc detected an invalid stdio handle` |
| 12:31:08.612821 | `audit ANOM_ABEND sig=6`；systemd-coredump signal 6/ABRT |
| 12:32:28–12:34:00 | 停止 → SIGKILL（stop-sigterm 超时）→ 重启 |
| 12:34:00 | `ServerDatabase::open_existing` → `verify_integrity`（无预算完整 `PRAGMA integrity_check`）**通过**，否则进程退出 |
| 12:34:15 | 监听；`SQLite runtime integrity check unavailable` |
| 12:42–12:44 | 在线备份 API 复制件已损坏：`Tree ... Child page depth differs`、`btreeInitPage() returns error code 11`、`Rowid out of order` |
| 12:50 | 外部 `sqlite3 'file:...?mode=ro'` 只读直查原库同样报损坏 |

运行版本：`abe342f`（2026-09-18，`fix(server): surface runtime SQLite corruption and schedule online backups`）之后的构建，即**已启用运行时监控与定时在线备份**；`ac9f8ac`（2026-09-24，#194 重写）尚未部署。

## 3. 证据

### 3.1 损坏形态：页级「半应用」，不是 cell 级畸形

以 `/home/mowind/.local/state/platpulse/platpulse.db.corrupt-20260917-175157` 为例（一律以 `sqlite3 'file:PATH?immutable=1'` 只读分析；带 `immutable=1` 之外的读取会在产物旁生成新的 `-shm`/`-wal` 副文件，会污染证据）：

- **首条完整性错误**：`*** in database main ***` / `Tree 64576 page 6804 cell 17: Rowid 1232489 out of order`。
- **重复行证明双链**：`SELECT rowid FROM host_metric_samples WHERE rowid BETWEEN 1232470 AND 1232495 ORDER BY rowid` 先返回 `1232472..1232489`，随后**再次**返回 `1232487,1232488,1232489,1232490..1232495` —— 即 rowid 1232487/1232488/1232489 存在于两个叶页；`SELECT count(*)` = 128。`dbstat WHERE pageno=6804` = `6804|host_metric_samples|/000/`。
- **页本身结构完好**：page 6804 为 type 13（表叶），ncell 18，cell 指针数组严格递减，每个 cell 都能解析出合法的 payload/rowid varint，rowid 严格递增 1232472..1232489，cell 无重叠。→ 缺陷是**叶页被双链**，不是坏 cell。
- **孤儿页**：三个 `never used` 页（447099 / 447103 / 447115）经原始字节解析均为结构合法、刚写入的 B-tree 叶页（447115 的 rowid 家族 1232588..1232592 与损坏页 6804 同族），但未挂入任何树、也不在 freelist → 与「事务的部分页已持久化、但父/兄弟指针未更新」的**推断**一致（推断，非直接观测）。
- **损坏行内容与时间**：rowid 1232487 = agent `2f7c5c72-6654-4ab5-8e94-97c208a7e07b` `network_rx_bytes_per_sec` observed 2026-09-17T09:47:03Z received 09:47:04Z；1232488 = 同 agent `network_tx_bytes_per_sec`；1232489 = `network_rx_bytes_per_sec` observed 09:47:04Z。→ 损坏行写入于 09-17 17:47:04–05（本地），**距首条报错 51 秒，处于活跃摄取中**（行时间戳仅定位数据写入时间，损坏时刻本身不可由它确定）。
- **受影响的表族**：`host_metric_samples`、`node_metric_samples`、`block_summaries`、`agent_report_receipts` —— 正是高翻动的 `INSERT ... ON CONFLICT DO UPDATE` + 有界 `DELETE` 表（HEAD：`crates/platpulse-server/src/http/report_ingestion.rs:830` 的 upsert、`:838` 的 `DELETE ... ORDER BY received_at DESC LIMIT -1 OFFSET ?`；`crates/platpulse-server/src/retention.rs:281` 的批量 `DELETE`）。

### 3.2 产物与首条错误

| 产物 | 大小 | schema_version | `PRAGMA integrity_check(1)` 首条错误 |
| --- | --- | --- | --- |
| `platpulse.db.corrupt-20260917-175157` | 1,831,444,480 | 214 | `Tree 64576 page 6804 cell 17: Rowid 1232489 out of order` |
| `platpulse.db.recorrupted-20260917-175524` | 1,814,585,344 | 122 | `Tree 25 page 208236 cell 431: 2nd reference to page 381576` |
| `platpulse-pre-repair-20260922T073657Z.db` | 5,341,560,832 | 1 | `Freelist: size is 79 but should be 81` |

两份 09-17 原始件携带**同一错误家族**：`Rowid ... out of order`、`2nd reference to page`、`never used` 页、索引条目数错误、`row N missing from index`、`non-unique entry in index`。

血缘澄清：`/home/mowind/.local/state/platpulse-backups/platpulse.db.predelete-20260917-181452` 含 `lost_and_found|422335|table`、schema_version 122、193 个 sqlite_master 对象、`integrity_check=ok`，是 **sqlite3 `.recover` 重建件**，不是原始现场文件；`platpulse-pre-repair-20260922T073657Z.db` 同样是重建后再次损坏的谱系。**原始损坏件只有两份 09-17 文件。**

### 3.3 运行环境（HEAD `f355eb8`）

| 事实 | 位置 / 值 |
| --- | --- |
| SQLite 版本 | 调查时（HEAD `f355eb8`）为 3.46.0（`libsqlite3-sys 0.30.1` + `sqlx 0.8.6`）；**2026-10-10 起构建的二进制为 3.53.2**（vendored patch，§7 建议 1）；系统 `/usr/sbin/sqlite3` 为 3.53.4 |
| 日志模式 | WAL；`synchronous=Full` |
| 独占锁 | `crates/platpulse-server/src/database.rs:114` `Self::new(path).with_exclusive_locking(!development)`；`:124` 定义；`:477` `options.locking_mode(SqliteLockingMode::Exclusive)` |
| 连接池 | `crates/platpulse-server/src/database.rs:29` `pub const SERVER_WRITE_CONNECTIONS: u32 = 1;`；`:501-502` `max_connections(1).min_connections(1)` → 单连接串行 |
| 关闭期 checkpoint | 调查时 `crates/platpulse-server/src/http/mod.rs:573-574` 为 `PRAGMA wal_checkpoint(TRUNCATE)`（唯一显式 checkpoint 点，也是最激进的一种）；**2026-10-10 起改为 `PRAGMA wal_checkpoint(FULL)`（`crates/platpulse-server/src/http/mod.rs:616`）** |
| 运行时监控（09-18+） | `abe342f` 新增 `http/health.rs` 的 `monitor_integrity` / `bounded_integrity_query` / `IntegrityConnection`，以及 `backup_schedule.rs` |
| 在线备份（09-18+） | `crates/platpulse-server/src/backup.rs:126` `VACUUM INTO '{temp}'` 在活池上执行；快照自身的 `VACUUM` 在 `backup.rs:226`（只作用于快照文件） |

**配置版本**（`/home/mowind/.config/platpulse/`，均 0600）：

- `server.toml.bak-20260917-183925`：`development = false`、https、`trusted_proxy_cidrs`，**无 `backup_dir`** → **09-17 生效配置，独占锁开启**。
- `server.toml.bak-20260918-183624`：在上者基础上新增 `backup_dir = "/data/platpulse-backups"` 与 `[backup_schedule] required_mount="/data" interval_hours=24` → 第二个活库连接自 09-18 起出现。
- 当前 `server.toml`：保留 `backup_dir`/`backup_required_mount`，已移除 `[backup_schedule]`（对应 ADR 0008）。

### 3.4 存储与内核（排除介质故障）

- `nvme1n1`（承载 `/` 与现场库）：WD SN560E PW 232141WD，SMART overall-health **PASSED**，Critical Warning 0x00，Available Spare 100%，Percentage Used 0%，**Media and Data Integrity Errors 0**，Error Information Log Entries 1，Unsafe Shutdowns 61，Data Written 9.61 TB。
- `nvme0n1`（`/data`）：WD SN560E，PASSED，Critical Warning 0x00，Percentage Used 4%，Media/Data Integrity Errors 0，Unsafe Shutdowns 25，Data Written 46.5 TB。
- `/proc/diskstats` 共 20 个字段、**不含 I/O 错误计数器**（故不能据它排除介质错误）；其 discard 相关计数两盘均为 0；`journalctl -k` 自 2026-09-01 起无 nvme/ext4/IO/remount/corrupt 错误。`/` 挂载 `ext4 rw,relatime`（ordered data mode）。
- 唯一硬件侧异常：`nvme1n1` 的 61 次 Unsafe Shutdown。无任何介质错误证据。

## 4. 已排除 / 已澄清

- **介质与文件系统**：见 §3.4，两盘 SMART 全绿、零媒体错误、内核无 IO/FS 错误。
- **09-17 外部进程**：17:19–17:47:56 之间无外部 sqlite3 / 备份 / gdb / 拷贝；当日 sudo 命令均在损坏之后（§2.1）。
- **09-23 的 `Fatal error: glibc detected an invalid stdio handle`**：来自取证 gdb 会话注入的 `fopen/malloc_info/fclose`（12:31:08 `audit ANOM_ABEND sig=6`），**是操作/取证行为所致，不是服务端代码的损坏源**。
- **09-23 12:50 的外部只读查询**（`#137` 记录的 `-shm` 截断 SIGBUS 风险）发生在**首次观察到损坏（12:42–12:44）之后**，不能解释该次损坏本身。
- **#194 评论中已较强排除**：`bounded_integrity_query` 用 progress handler 取消进行中的 `integrity_check(1)`（只读语句，中断不写回任何页面，该论证不依赖具体实现；#194 另将其重写为 `sqlite_check::bounded_scalar_query`，运行时不再对主库做周期整库扫描，属额外加固）；`VACUUM INTO` 与并发摄取共用同一写连接（池 `max_connections = 1`，语句串行；`VACUUM INTO` 不写源库）。
- **09-11 的 4 次 SIGBUS coredump** 属于 `walFindFrame` / `-shm` 截断一类（#137），与本次页级损坏形态不同；09-17 与 09-22 无 platpulse-server core。

## 5. 根因机制

WAL-reset bug 的五步（[sqlite.org/draft/wal.html §11.1](https://sqlite.org/draft/wal.html)）：

1. 一个连接做了一次必须完成的 checkpoint；
2. 紧接着第二个 checkpoint 启动；
3. 在第二个 checkpoint 启动期间，另一个连接提交事务并**重置（reset）WAL**、从头写入新内容；由于数据竞态，第二个 checkpoint 没有意识到 WAL 已被重置，导致 WAL-Index 头部字段错误地认为 WAL 的一部分**已经** checkpoint 过；
4. 更多事务把 WAL 增长到超过第一次 checkpoint 的大小；
5. 第三次 checkpoint 跳过第 3 步事务的全部/部分帧 → 这些部分**永远不会写回主库文件** → 损坏。

**为什么能解释「启动检查通过、约 10 分钟后副本损坏」**：`integrity_check` 读取的是**包含未回填 WAL 帧的逻辑视图**；当 WAL 被 reset/truncate 后，未被回填的帧丢失，主库文件随即暴露损坏。要判定这一细节需要原始 `-wal`/`-shm` 副本（**现场未保存**）。

**备选假设**（同样晚于 3.46.0 才修复，不能排除）：`2025-11-04 (3.51.0) Improved resistance to database corruption caused by an application breaking Posix advisory locks using close()`。若应用在 SQLite 持锁期间关闭了底层 fd，另一连接可能抢先获得锁并造成损坏。

## 6. 待验证 / 未决

- 09-17 当天「≥2 连接并发」这一前提**未被现场证据证实**（当时无运行时监控，池为单连接 + 独占锁）。09-17 与 09-23 是否同源仍需坐实。
- **原始 `-wal`/`-shm` 未保存**，无法直接判定 WAL 视图 vs 主文件差异。
- 未在隔离副本上做故障注入复现（#196 建议的 `abort_v2_receipt` 触发器路径）。
- 09-17 与 09-23 产物未做页级/表族级比对。

## 7. 建议下一步（按优先级）

> 状态（2026-10-10）：建议 1、2、3 已实施（见各条下的「已实施」）；建议 4–5 待办。损坏库的最终处置：静止态判定为**无损坏、无需修复**（§11），现场库保持原样继续使用。

1. **升级内置 SQLite 至 ≥ 3.51.3**（或 backport 3.44.6 / 3.50.7）。注意：`libsqlite3-sys 0.33.0` 自带 SQLite **3.49.1，仍然受影响**，不能只升这一档。
   - **已实施**：sqlx 0.8.6 的 `sqlx-sqlite/bundled` 把 `libsqlite3-sys` 钉在 `^0.30.1`（= SQLite 3.46.0）；放宽该约束要等 sqlx 0.9.0（`>= 0.30.1, < 0.38.0`），而那是破坏性大版本迁移。因此把 `libsqlite3-sys` **0.30.1 原样 vendor 进仓库**（版本号保持 0.30.1，继续满足 `^0.30.1`），只把 `sqlite3/sqlite3.c`、`sqlite3/sqlite3.h`、`sqlite3/sqlite3ext.h` 换成 **SQLite 3.53.2** amalgamation，并用根 `Cargo.toml` 的 `[patch.crates-io] libsqlite3-sys = { path = "vendor/libsqlite3-sys" }` 接入。依据：0.30.1 的预生成绑定没有任何 `libsqlite3_sys_<版本>` cfg 门控、C API 向后兼容，故只换 C 源可行。sha256、改动清单与退出计划见 `vendor/libsqlite3-sys/README.platpulse.md`。
   - 复核：`strings target/release/platpulse-server | grep -E '^3\.[0-9]+\.[0-9]+$'` 应打印 3.53.2（修复前为 3.46.0）。
2. **收缩触发面**：去掉/减少关闭路径的 `PRAGMA wal_checkpoint(TRUNCATE)`（`http/mod.rs:574`）；避免在活库上开第二连接做整库扫描。ADR 0008 已将备份改为离线，会顺带降低暴露。
   - **已实施**：关闭路径的 `checkpoint_wal` 改为 `PRAGMA wal_checkpoint(FULL)`（`crates/platpulse-server/src/http/mod.rs:616`，`TRUNCATE` 时位于 `:611-612`）。TRUNCATE 会重置 WAL，正是 §5 机制的触发点，而截断对关停没有任何收益；调用方与优雅关停断言（`crates/platpulse-server/src/cli.rs:1469`、`:1504` 两处）语义不变。
   - **未发现**运行时第二个长驻活库连接或周期整库扫描（`crates/platpulse-server/src` 内只剩连接池）；本报告此前所称「2026-09-18 起存在第二个活库连接」在现码里无法复现。
3. **离线权威判定**：停机后对副本执行 `platpulse-server verify-integrity --config <COPY>/server.toml`（#194 新增），记录**首条错误与耗时**。
   - **已实施**（2026-10-10，取证目录 `/data/platpulse-forensics-20261010T113926`）：停机副本（sha256 `68618c30c6df9efecd09f12b517eb7dbdb30de7706a26308ce2d8e4a43a33767`）在系统 `sqlite3 3.53.4` `immutable=1` 下 `integrity_check(1)` → `ok`（54s）、在 `platpulse-server verify-integrity` → `SQLite integrity check: ok`、exit 0（63s）。结论与完整证据见 §11。
4. **保留并比对现场产物**：`corrupt-20260917-175157` / `recorrupted-20260917-175524` / `platpulse-pre-repair-20260922T073657Z.db`，做 `dbstat`、freelist 计数、首条 `integrity_check` 错误比对。
5. **坐实机制（可选但最有说服力）**：用 SQLite **3.46.0 amalgamation** 编译 Phil Eaton 复现程序（2 线程 / 3 连接 + 大 `PRAGMA mmap_size`），在隔离环境跑出「丢失写入 + 主库损坏」；再用系统 `sqlite3 3.53.4` 作对照，应只在 3.46.0 上坏。

## 8. 明确不作为损坏检测手段

`VACUUM INTO` 只重建逻辑内容，既不反映源库物理损坏，也不构成可验证的恢复点（ADR 0008）。本调查不使用 `VACUUM INTO` 判断损坏。

**对运行中的 WAL 库做外部打开同样不作为判定手段**：`immutable=1` 忽略 `-wal`，读到的是未回填的陈旧主文件视图，会给出与静止态相反的 `malformed`（本例运行中副本报 `Freelist: freelist leaf count too big on page 3819225`，同一库静止后为 `ok`）；`mode=ro` 则需要 `-shm`，存在 #137 记录的截断风险。判定必须以**停机后的副本**为准。

## 9. 来源

- SQLite：[changes.html](https://sqlite.org/changes.html)（3.51.3 / 3.53.0 修复记录）、[draft/wal.html §11 The WAL-Reset Bug](https://sqlite.org/draft/wal.html)、修复提交 [7168988acbec2d8d](https://sqlite.org/src/info/7168988acbec2d8d)、测试夹具 [tmstmpvfs.c](https://sqlite.org/src/file/ext/misc/tmstmpvfs.c)。
- Tailscale 事后分析：<https://tailscale.com/blog/sqlite-wal-reset-bug>（2026-08-12）。
- Phil Eaton 复现：<https://theconsensus.dev/p/2026/08/23/another-look-at-sqlite-wal-reset.html>（2026-08-23）。
- 仓库内：[ADR 0008](../adr/0008-offline-server-backup.md)（离线备份）、issue #194（运行时监控语义，#196 独立）、#137、#176、#195。
- 现场一手证据：systemd journal（09-17 / 09-23）、`coredumpctl`、SMART/`/proc/diskstats`、损坏产物的原始页字节。

## 10. 复现命令（可复查）

```bash
# 完整性首条错误（务必带 immutable=1，避免污染产物）
sqlite3 "file:/home/mowind/.local/state/platpulse/platpulse.db.corrupt-20260917-175157?immutable=1" \
  "PRAGMA integrity_check(1);"

# 重复行 / 双链叶页
sqlite3 "file:...corrupt-20260917-175157?immutable=1" \
  "SELECT rowid FROM host_metric_samples WHERE rowid BETWEEN 1232470 AND 1232495 ORDER BY rowid;"
sqlite3 "file:...corrupt-20260917-175157?immutable=1" \
  "SELECT pageno,name,path FROM dbstat WHERE pageno=6804;"

# 内置 SQLite 版本（2026-10-10 起应为 3.53.2）
strings target/release/platpulse-server | grep -E '^3\.[0-9]+\.[0-9]+$'
```

## 11. 2026-10-10 停机取证与静止态判定

取证目录 `/data/platpulse-forensics-20261010T113926/`：`hot/`（运行中副本）、`stopped/`（停机后副本）、`verify/{server.toml,state}`、`logs/stage1.log`、`logs/stage2.log`、`MANIFEST.sha256`。修复后二进制（内置 SQLite 3.53.2）于 11:38 构建，11:42:08 重启服务。

| 阶段 | 做法 | 结果 |
| --- | --- | --- |
| 停机前 | 运行中库 `cp`（14s） | `hot/platpulse.db` sha256 `97c922bd450352ae58f0371f1db0f9d077898f603a20fa560a2df495760a8b2f`（`-wal` `7ff5e7ef…fe2d63`、`-shm` `bad077a0…86d783`） |
| 停机 | `systemctl --user stop platpulse-agent` → `platpulse-server` | 均 exit 0、无残留进程；停机后库 17,629,487,104 B 且 **`-wal` 消失**（干净关停已 checkpoint 并删除 WAL） |
| 静止副本 | `stopped/platpulse.db` sha256 `68618c30c6df9efecd09f12b517eb7dbdb30de7706a26308ce2d8e4a43a33767`（无 wal） | `page_size=4096`、`page_count=4,304,074`、`freelist_count=4`、`journal_mode=delete`、`application_id=0`、`sqlite_master` 259 对象 |
| 系统引擎 | `sqlite3 3.53.4` `file:…?immutable=1` `PRAGMA integrity_check(1)` | **ok**（54s） |
| 内置引擎 | `target/release/platpulse-server verify-integrity --config <COPY>/verify/server.toml` | **`SQLite integrity check: ok`**、exit 0（63s） |
| 对照：运行中副本 | `hot/platpulse.db` `immutable=1` | `*** in database main *** Freelist: freelist leaf count too big on page 3819225`（0s） |
| 对照：09-17 原始件 | 两份产物 `immutable=1` | 静止态仍为真损坏：`Tree 64576 page 6804 cell 17: Rowid 1232489 out of order`；`Tree 25 page 208236 cell 431: 2nd reference to page 381576` |

结论：

- 现场库**静止态无损坏**（两个独立引擎一致 `ok`）⇒ **无需修复、无需重建**；处置为保持原库继续使用，同时按 §7 建议 1/2 消除触发面。
- 同一份库**运行中**以 `immutable=1` 打开即报 `malformed`；差异只来自 `-wal` 未回填（`immutable=1` 忽略 `-wal`）。**2026-10-09/10 记录的 `malformed` / `btreeInitPage() returns error code 11` 属此类伪影，不是盘上损坏**（详见 §8）。判定损坏必须在停机后对副本进行。
- 09-17 的真实损坏（§3.2 两份原始件）在静止态复核仍成立，与本结论不冲突：那两份是 09-17 现场的产物，与当前库的物理状态无关。
- 重启耗时与预算相符：启动整库校验约 226s（11:42:08 → 11:45:54 `listening on 127.0.0.1:8080`），远低于 `STARTUP_INTEGRITY_BUDGET = 600s`。重启后 `/health/ready` 各组件全 ready；Agent 在 11:45–11:46 出现两条 `stale_report` 拒绝后恢复，`/api/admin/v1/agents` 显示两个 Agent `online`、节点 `Sync` `healthy`、`last_received_at` 持续更新。
