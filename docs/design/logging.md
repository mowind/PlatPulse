# 日志（诊断）设计

状态：已实现（issue #250）。本文是 Agent/Server 进程诊断日志的权威说明。日志**不是** Audit、不是 Observation/Collection 状态、不是 Doctor 证据或 Incident：它不持久化、不进入任何 DTO/OpenAPI/SSE 契约，也不改变「未知不得表达为 `0`/`false`/Healthy」的规则。

## 1. 出口与流

- Agent 与 Server 都通过 `tracing` 打日志，只在二进制入口（`main.rs`）初始化，由各自 crate 的 `src/logging.rs` 安装唯一出口。`platpulse-core` 保持 I/O-free，不初始化日志。
- **stdout 只留机器可读输出与 CLI 结果**（`--print-openapi`、`init`/`owner create`/`backup` 等命令的结果行、`generate-node-id`）。生命周期与诊断一律写 **stderr**（systemd 下即 journal）。原 `listening on …` 已迁为日志行 `listening listen=…`。
- 写入经独立线程的有界队列（`tracing-appender` non-blocking，lossy）：journal 卡住时丢行而不阻塞 async 运行时。进程退出前 flush（Agent 的 fail-closed `process::exit` 路径也先 flush）。
- 不做日志文件与轮转（交给 systemd/journald），不引入日志聚合服务。

## 2. 级别与过滤

- 默认 `info`，只作用于 PlatPulse 自己的 target（`platpulse_server`/`platpulse_agent`/`platpulse_core`）；第三方 crate（SQLx 会把每条语句打成 info）默认 `warn`。
- `--log-level <error|warn|info|debug|trace>` 优先于 `RUST_LOG`；未给 `--log-level` 时 `RUST_LOG` 按 env-filter 语法生效（如 `RUST_LOG=debug`、`RUST_LOG=platpulse_server=debug,sqlx=info`）。不可解析的 `RUST_LOG` 回落默认并打一条 warn。
- 级别划分：
  - `error`：需要人工处理的失败（存储写失败、恢复回滚失败、优雅关闭不完整、fail-closed 退出）。
  - `warn`：后台循环推迟/失败（Retention 清理、Alert sweep、通知投递、Geo、容量保护、Validator Provider、Reward Wallet）、上报被拒、5xx。
  - `info`：生命周期（starting、listening、收到关闭信号、shutdown complete、Agent stopped）与部分接受的 Report。
  - `debug`：每个请求的完成行、被接受的 Report、收到的 Report。
- **不刷屏**：被完整接受的 Report 是例行事件，记 `debug`，因此采集周期在默认级别不产生逐轮 info。会逐 tick 重复的 Agent 条件（Inventory 声明被拒、上报被拒、采集推迟、投递推迟）经 `RepeatingMessage` 汇总：首次立即输出，之后同一条件至多每 60 秒一行并带累计次数，条件消失后重置。

## 3. 格式

- `--log-format text`（默认）：`<ts> <LEVEL> <target> <message> key=value …`，一行一事件。`ts` 为 UTC RFC 3339 毫秒。含空白、引号、`=` 或控制字符的值按 JSON 字符串转义加引号；message 中的控制字符被转义，任何值都不能伪造第二行。
- `--log-format json`：每行一个 JSON 对象，固定含 `ts`、`level`、`target`、`message`，其后是字段；数字/布尔保持类型。

```text
journalctl -u platpulse-server | grep 'request_id=0195…'
journalctl -u platpulse-server -o cat | jq 'select(.outcome == "rejected")'   # --log-format json
```

## 4. 字段词表

日志字段只用下列名字（`scripts/check-logging.py` 在 CI 中强制），与 `CONTEXT.md` 术语对齐：

| 字段 | 含义 |
| --- | --- |
| `request_id` | 请求关联 id，等于响应头 `x-request-id` 与错误包体 `request_id` |
| `route_group` | `agent`/`admin`/`public`/`health`/`api`/`web` |
| `route` | 请求路径（原始 path，不含 query；不是路由模板） |
| `status` | HTTP 状态码 |
| `duration_ms` | 请求耗时 |
| `agent_id`、`boot_id`、`report_id`、`report_sequence` | 已认证的 Agent 与已校验的 Report 身份 |
| `node_id` | Node 作用域（Observation 按 Node，不合并为 Agent 级） |
| `operation_id` | Admin Operation |
| `outcome` | 结果或原因码（`accepted`/`partially_accepted`/`rejected`、拒收错误码、shutdown 结果等） |
| `error` | 失败的文本（经出口脱敏） |
| `listen` | 监听地址（运维配置；只去凭证不去 IP） |

其余上下文写进 message。

## 5. 请求关联

`request_id_middleware` 为每个请求生成 id 并打开 `request` span（`request_id`、`route_group`、`route`；span 为 error 级，因此调低级别后 warn 仍带关联字段）。Report Ingestion 在认证后把 `agent_id` 记入 span，在 Report 校验并匹配身份后再记 `boot_id`/`report_id`/`report_sequence`：未校验的 Agent 输入不进入日志字段（Server 是信任边界）。该请求内的每一行日志自动携带这些字段；请求结束时中间件写一条完成行（`status`、`duration_ms`，Report 另带 `outcome`）。

## 6. 脱敏出口

脱敏由出口强制，而不是靠调用点自觉：

- 字段**名**是凭证键（`password`、`credential`、`token`、`cookie`、`authorization`、`rpc_url`、`secret`、`api_key`、`private_key`、`key` 等，大小写与分隔符无关）时，值整体替换为 `[REDACTED]`，不论类型。
- 所有字符串字段与 message 经该 crate 的 `redaction::redact_sensitive`：键值凭证、`Bearer`/`Basic` 后的令牌、URL userinfo（`wss://user:pass@host`）。Server 另外遮蔽 IP 字面量（与快照/备份同一规则）；Agent 日志只在本机，保留 RPC 主机以便诊断。
- 因此新写一条 `tracing::warn!(error = %e, "…")` 默认就是脱敏的；日志调用点不再手动调用 `redact_sensitive`。既有的手动 `redact_sensitive` 调用服务于 DTO、持久化诊断与备份，它们不是日志出口，保持不变。
- CI 闸门 `scripts/check-logging.py`：crate 源码（测试模块除外）禁止裸 `eprintln!`/`eprint!`/`dbg!`（仅 `main.rs` 在 flush 之后打印最终 CLI 错误）；`println!` 只允许在 `main.rs`/`cli.rs`；日志字段必须在词表内。

## 7. 关键路径诊断清单

| 路径 | 当前状态 |
| --- | --- |
| Report Ingestion：接受/部分接受/拒收 + 原因码 | 已实现（完成行 `outcome`；HTTP 级拒收 `report refused outcome=<code>` warn；receipt 级整单拒收 `report rejected outcome=<RejectionCode>` warn）。部分接受的逐 Node/样本原因仍只在 receipt 中，待补 |
| Report 事务存储失败 | 已实现（error，带 `request_id`） |
| Ingestion 后 Retention 清理推迟 | 已实现（warn） |
| 启动、监听、收到关闭信号、优雅关闭完成/不完整及原因 | 已实现 |
| 后台循环：Retention 启动清理、Alert sweep、通知投递、Operation worker、Geo、容量保护、Validator 发现/刷新、Reward Wallet | 已实现（失败 warn） |
| Agent：采集推迟、投递推迟、上报被拒、Inventory 声明被拒（汇总） | 已实现 |
| Agent：关闭期间 worker 超时/失败、fail-closed 退出 | 已实现 |
| Receipt 应用、重试决策、备份与恢复成功路径、SSE 连接生命周期、`operation_id` 字段 | 待补（`debug` 级，后续切片） |

## 8. 成本

默认 `info` 下稳态几乎静默：Server 每个被接受的 Report 与每个请求都是 `debug`，只在失败、拒收与生命周期时写行（Server 侧拒收不做汇总：整单被拒的 Report 由 receipt 终结、不会重试，HTTP 级拒收按 Agent 退避节奏重复）；Agent 的重复条件每 60 秒至多一行。量级：稳态每进程每天几十行以内；持续故障时每个重复条件每小时约 60 行（Agent）或每次失败一行（Server 后台循环按其 tick 周期，最短 60 秒级）。`RUST_LOG=debug` 下 Server 约每请求 1–3 行，用于短时排障，不建议长期开启。写入不阻塞运行时（见 §1）。
