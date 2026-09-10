# Research: Komari 如何获取、保存和刷新地理位置

> 调研日期：2026-09-10（UTC）。只研究，不实施、不修改设计或 Issue。  
> 范围：当前 Komari Server、Agent、默认 WebUI 分支源码及官方文档。以下源码链接固定 SHA；当前默认分支不等于用户部署版本或经过联调的发布组合。未运行 Komari，未将真实 Host / Peer IP 发送到第三方测试。  
> 已阅读本仓库 AGENTS.md、CONTEXT.md、docs/agents/domain.md、issue-tracker.md、相关设计和实际 Geo / Peer 代码。沿用现有 docs/research/ 笔记约定；本文不是 ADR 或实施规格。

## Summary

1. **Komari 定位的是注册 Client 所代表的受监控机器，近似 PlatPulse 的 Host，不是 PlatON Node 的 Peer 集合。** Client 用 UUID / token 标识，IP 和 region 是可变属性，不是身份。
2. **默认开启 GeoIP，默认 provider 为 ipinfo。** 当前实现访问旧式、未携带 token 的 `https://ipinfo.io/{目标IP}/json`，不是 IPinfo 当前的 Lite API。查询由 Komari Server 发起，目标 IP 来自 Agent 基础信息上报。
3. **“服务端本地保存”有两层：** Client 的 IPv4 / IPv6 / region 持久化到主数据库；provider 查询结果另有 48 小时进程内缓存。region 是国家/地区旗帜 emoji，不是完整 IPinfo JSON，更不是精确经纬度。数据库保留则 region 跨重启，查询缓存不跨重启。
4. **Settings 的“更新 GeoIP 数据库”不是立即刷新全部机器位置。** 它更新当前 provider，成功后清空内存缓存。对 ipinfo，provider 更新本身是 no-op，实际只有清缓存；之后 Client 再上报 basicInfo 才重新查询并保存 region。离线 Client 不会因点击而被批量重算。
5. **当前 v2 的关键细节：** helper 虽有“上报 IP 都为空时用 fallback IP”的代码，但真实 basicInfo 调用传入空 fallback，不能描述成“Server 总会使用 HTTP 请求来源 IP 补齐”。
6. **不能直接照搬到 PlatPulse：** 外部查询 Peer IP 会改变现有本地 GeoLite2 Country 的隐私边界；改为展示 Host 又会改变地图主体。本文不推定用户已从 Peer 地图转向 Host 地图，后续必须明确选择。

## Sources and revision boundary

| 组件 | 当前分支与固定提交 | 核验 |
| --- | --- | --- |
| Server | main · `b11ffd3aa7cca03502a75eb64ecbe827d6831d3a`，2026-09-04 | [仓库 API](https://api.github.com/repos/komari-monitor/komari)、[提交](https://github.com/komari-monitor/komari/commit/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a) |
| Agent | main · `cafd4b6590b17ec3a59c9d12360e9f58f8576b61`，2026-09-09 | [仓库 API](https://api.github.com/repos/komari-monitor/komari-agent)、[提交](https://github.com/komari-monitor/komari-agent/commit/cafd4b6590b17ec3a59c9d12360e9f58f8576b61) |
| Web | **radix** · `83b42c3c4b2a52b546005ecf4cd534a3bd9d4c5b`，2026-09-09 | [仓库 API](https://api.github.com/repos/komari-monitor/komari-web)、[提交](https://github.com/komari-monitor/komari-web/commit/83b42c3c4b2a52b546005ecf4cd534a3bd9d4c5b)；main 是旧分支 |
| 官方文档仓库 | main · `bb98bf6a8955b7d1efd8d239e7a68819b47b9850` | [文档仓库](https://github.com/komari-monitor/komari-document/tree/bb98bf6a8955b7d1efd8d239e7a68819b47b9850) |
| PlatPulse 对照 | 本地 HEAD `b43a078aeb944fdc566c6ba9c5af3b08853e39c2` | 直接读取本地设计、领域与实现；以下对照引用固定该提交 |

[Komari README 第 10–14 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/README.md#L10-L14)将官方文档指向 [komari.wiki](https://www.komari.wiki/)。其“自托管”定位不能用来推导“无第三方 IP 查询”，实际外发以源码为准。官方 [API 文档](https://www.komari.wiki/dev/api)给出 region 为 🇸🇬 的例子，并说明“地区（通常使用国旗表情符号）”；[固定文档第 216–252 行](https://github.com/komari-monitor/komari-document/blob/bb98bf6a8955b7d1efd8d239e7a68819b47b9850/dev/api.md#L216-L252)。官方 [RPC 文档](https://www.komari.wiki/dev/rpc)记录了 admin:testGeoip 的可选 ip 参数及 GeoIPRecord 返回值：[固定文档第 1293–1307 行](https://github.com/komari-monitor/komari-document/blob/bb98bf6a8955b7d1efd8d239e7a68819b47b9850/dev/rpc.md#L1293-L1307)。未找到单独解释刷新语义的官方设置指南，故以下以真实调用链补足。

## Findings

### 1. 身份与完整数据流

```text
受监控机器上的 Komari Agent
  → NIC / 自定义值 / 外部 IP 回显服务取得 IPv4、IPv6
  → basicInfo 上报（Agent token 绑定 Client UUID）
Komari Server
  → ingestBasicInfo → saveClientBasicInfo
  → 先 IPv4 后 IPv6；provider + IP 的内存缓存
  → 未命中调用 GeoIP provider，返回国家 ISO 两字母代码
  → 转旗帜 emoji → UPDATE 主库 Client.region / ipv4 / ipv6
WebUI
  → 从 Client.region 渲染旗帜
  → Settings 可切 provider、更新 provider/清缓存、测试指定 IP
```

Client 模型以 UUID 为主键、token 唯一，IP / region 与 CPU、内存等机器属性在同一行；不是以 IP 为主键，也不是远端 Peer 模型。[schema 第 10–45 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/database/models/models.go#L10-L45)。保存时绑定身份的 uuid 覆盖 info 中的 uuid：[第 23–28 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/client/uploadBasicInfo.go#L23-L28)。Komari UI 的“节点”不能自动翻译成 PlatPulse 的 PlatON Node；PlatPulse 的一个 Agent/Host 可关联多个独立 Node，这不是这里的 Client 模型。

### 2. Agent 如何获取 IP：优先级、双栈、外部回显

[GetIPAddress 第 123–157 行](https://github.com/komari-monitor/komari-agent/blob/cafd4b6590b17ec3a59c9d12360e9f58f8576b61/monitoring/unit/ip.go#L123-L157)的真实顺序：

1. 启用 `--get-ip-addr-from-nic` 时先查允许的网卡；只要任一地址族取得值，就立即返回整个结果，**不再用自定义或外部服务补另一个缺失地址族**。
2. NIC 模式未取得任何地址或未开启时，IPv4 / IPv6 分别处理：该地址族有 `--custom-ipv4` / `--custom-ipv6` 则使用，否则请求相应回显服务。
3. 网卡路径跳过未启用网卡、loopback，IPv6 排除 link-local；没有保证取到的是公网出口，例如私网 IPv4 仍可能入选。[网卡遍历第 160–220 行](https://github.com/komari-monitor/komari-agent/blob/cafd4b6590b17ec3a59c9d12360e9f58f8576b61/monitoring/unit/ip.go#L160-L220)。NIC 模式默认 false；`--prefer-ip-version` 是连接面板时的地址族偏好，不是 Geo 选国家的优先级。[参数第 168–183 行](https://github.com/komari-monitor/komari-agent/blob/cafd4b6590b17ec3a59c9d12360e9f58f8576b61/cmd/root.go#L168-L183)。

外部回显服务按顺序尝试，首个正则匹配到地址即返回：

| 地址族 | URL 的尝试顺序 |
| --- | --- |
| IPv4 | https://www.visa.cn/cdn-cgi/trace → https://www.qualcomm.cn/cdn-cgi/trace → https://www.toutiao.com/stream/widget/local_weather/data/ → https://edge-ip.html.zone/geo → https://vercel-ip.html.zone/geo → **http://ipv4.ip.sb** → https://api.ipify.org?format=json |
| IPv6 | https://v6.ip.zxinc.org/info.php?type=json → https://api6.ipify.org?format=json → https://ipv6.icanhazip.com → **http://api-ipv6.ip.sb/geoip** |

依据：[IPv4 第 48–84 行](https://github.com/komari-monitor/komari-agent/blob/cafd4b6590b17ec3a59c9d12360e9f58f8576b61/monitoring/unit/ip.go#L48-L84)、[IPv6 第 86–121 行](https://github.com/komari-monitor/komari-agent/blob/cafd4b6590b17ec3a59c9d12360e9f58f8576b61/monitoring/unit/ip.go#L86-L121)。这一步**不是用 ipinfo 取得机器自己的 IP**。回显请求接收者看到请求出口；代理/NAT 可影响结果，不能保证等于物理网卡或实际机房地址。客户端使用环境代理、强制 tcp4 / tcp6、每次请求 15s 超时、User-Agent 为 curl/8.0.1：[第 15–45 行](https://github.com/komari-monitor/komari-agent/blob/cafd4b6590b17ec3a59c9d12360e9f58f8576b61/monitoring/unit/ip.go#L15-L45)。解析代码不检查 HTTP 状态，仅在响应体找 IP；全部失败返回空字符串且 error 为 nil，未维护 IP last-good 缓存。

基础信息负载带 ipv4、ipv6、CPU、OS、内存等，使用 Agent token 向自建 Server 的 `POST /api/clients/v2/rpc` 发送，发送客户端超时 30s。[basicInfo 第 39–96 行](https://github.com/komari-monitor/komari-agent/blob/cafd4b6590b17ec3a59c9d12360e9f58f8576b61/server/basicInfo.go#L39-L96)。**这不表示所有机器指标都发给 IPinfo**：Server 的 IPinfo provider 仅把目标 IP 放在第三方请求 URL；第三方另能看到 Server 的连接出口和普通请求元数据。

### 3. Server 使用上报 IP 还是请求来源 IP？

- helper 的 hasClientIP 仅判断 ipv4 / ipv6 是否为非空字符串。任一非空则跳过 fallback，**不先验证地址是否合法**；两者都空且 fallbackIP 可解析时才补对应地址族。[第 30–51 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/client/uploadBasicInfo.go#L30-L51)。
- **真实 v2 分发传的是 `ingestBasicInfo(uuid, params.Info, "")`。** HTTP / WebSocket 使用的该分发路径没有把 RemoteAddr / ClientIP 传给 helper；所以不能把潜在 fallback 能力说成当前正常路径的行为。[report_v2 第 46–71 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/client/report_v2.go#L46-L71)。
- Geo 遍历 ipv4 后 ipv6，逐个 net.ParseIP；无记录、错误或无法生成旗帜则继续。IPv4 有有效 flag 即返回，不同时存两份国家。此段只 ParseIP，没有按字段名再次验证地址族，没有 PlatPulse 那样的公网 IP 筛选。[第 54–79 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/client/uploadBasicInfo.go#L54-L79)。

因此双栈国家不同则 IPv4 可用结果优先；IPv4 不可用才可能用 IPv6。Agent 两个地址都取不到时，当前 v2 不会可靠地以请求来源补齐。**管理员测试**未指定 IP 时用 meta.RemoteIP 是另一条路径，不能误解为 Host IP 获取机制。[测试第 171–194 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/rpc/jsonrpc/admin.system.go#L171-L194)。

### 4. Geo provider：默认、抽象、实际 IPinfo API 与 token

GeoIPService 只有 Name、GetGeoInfo(net.IP)、UpdateDatabase、Close；通用 GeoInfo 只有 ISOCode、Name。[第 14–37 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/geoip.go#L14-L37)。默认 `geo_ip_enabled=true`、`geo_ip_provider=ipinfo`；构造失败降为 EmptyProvider，并不按供应商列表逐个 failover。[设置第 23–25 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/internal/config/settings.go#L23-L25)、[初始化第 54–115 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/geoip.go#L54-L115)。

| 配置值 | 来源和外发 | 证据 |
| --- | --- | --- |
| ipinfo（默认） | `https://ipinfo.io/{ip}/json`，5s 超时，**没有 token** | [ipinfo 第 11–78 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/ipinfo.go#L11-L78) |
| geojs | `https://get.geojs.io/v1/ip/geo/{ip}.json`，5s | [geojs 第 26–69 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/geojs.go#L26-L69) |
| ip-api | **明文 HTTP** `http://ip-api.com/json/{ip}?fields=status,message,country,countryCode`，5s | [ipapi 第 39–71 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/ipapi.go#L39-L71) |
| mmdb | 本地 Country MMDB；缺失时从 Loyalsoldier/geoip 的 GitHub release 分支下载 | [mmdb 第 17–75 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/mmdb.go#L17-L75) |
| empty / 未识别值 | EmptyProvider 返回错误，不能提供位置 | [分支](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/geoip.go#L113-L115)、[EmptyProvider 第 17–24 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/emptyProvider.go#L17-L24) |

**当前 IPinfo token 配置链没有实现。** APIToken 是被注释掉的字段，请求没有 query token / Basic Auth / Bearer header；GeoIP Settings 只有开关、provider、更新与测试，未提供 IPinfo token 控件。[provider 第 11–16、48–53 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/ipinfo.go#L11-L53)、[UI 第 40–120 行](https://github.com/komari-monitor/komari-web/blob/83b42c3c4b2a52b546005ecf4cd534a3bd9d4c5b/src/pages/admin/settings/general.tsx#L40-L120)。auto_discovery_key、管理 API key、Agent token 均不是 IPinfo token。

**不要混用旧 API 与当前 IPinfo Lite。** 本次读取的 [IPinfo 官方 Lite 文档](https://ipinfo.io/developers/lite-api)规定 `https://api.ipinfo.io/lite/{ip}?token=$TOKEN`，country_code 为 ISO 两字母，country 为全名，另有 continent、ASN；该 Lite 服务官方称无每日/每月请求上限。Komari 读取旧 endpoint 的 country 作为 ISO 代码，两者 URL、鉴权和字段含义不同，不能仅替换 URL。[官方开发者文档](https://ipinfo.io/developers)解释 query token、Basic Auth、Bearer 三种鉴权，并区分 Lite 与付费额度。

源码“每天 1000 次 / 无需 token”的注释不是当前供应商承诺；本次没有验证旧无 token 入口当日限额或持续可用性。`/lite/me` 查询调用方出口：若 Server 调用，得到的是 Server，不会自动变成受监控 Host。

### 5. 本地持久化、缓存、国家旗帜与精度

| 层次 | 保存内容 | 生命周期 |
| --- | --- | --- |
| 主数据库 Client | uuid、ipv4、ipv6、region 等；三个 IP/region 字段为 varchar(100) | 默认主库 `./data/komari.db`，不是 metrics.db；持久文件保留则跨重启 |
| Geo 查询缓存 | providerName + ":" + ip.String() → GeoInfo{ISOCode, Name} | go-cache，默认 48h 过期、1h 清理；进程内，无磁盘序列化，不因命中滑动续期 |
| mmdb 文件 | `./data/GeoLite2-Country.mmdb` | 已存在时启动加载，缺失才下载；独立于 Client.region 和查询缓存 |

依据：[Client schema](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/database/models/models.go#L10-L45)、[主库路径第 274–277 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/database/dbcore/dbcore.go#L274-L277)、[主库初始化/迁移第 400–450 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/database/dbcore/dbcore.go#L400-L450)、[cache 第 14–25 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/geoip.go#L14-L25)、[cache 读写第 126–147 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/geoip.go#L126-L147)。保存按 uuid 执行 Updates(map)，设置通用 updated_at；没有 geo 专用查询时间、来源、错误或过期列：[SaveClientInfo 第 27–39 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/database/clients/client.go#L27-L39)、[更新第 113–121 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/database/clients/client.go#L113-L121)。

缓存只在 err==nil 且 info!=nil 时写；同 provider 同 IP 的不同 Client 可共享结果。到期不会主动发请求，下次 basicInfo / 管理员测试才查询；重启丢失 cache，不丢已有 region。IPinfo 虽解析 city、region、loc、postal、org、timezone，但返回通用 GeoInfo 时仅用 country 填 ISOCode/Name，没有持久化完整地理 JSON。[ipinfo 第 18–29、68–78 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/ipinfo.go#L18-L78)。

自动写入的 region 是两字母国家/地区代码转 Unicode flag，而非 IPinfo 的州/省 region 文本：[转换第 39–51 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/geoip.go#L39-L51)、[赋值第 69–78 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/client/uploadBasicInfo.go#L69-L78)。当前卡片把 basic.region 传给 Flag；Flag 接受 emoji 或两字母文本，渲染 /assets/flags/{CODE}.svg，非法值用 UN：[Node 第 83–86 行](https://github.com/komari-monitor/komari-web/blob/83b42c3c4b2a52b546005ecf4cd534a3bd9d4c5b/src/components/Node.tsx#L83-L86)、[Flag 第 48–78 行](https://github.com/komari-monitor/komari-web/blob/83b42c3c4b2a52b546005ecf4cd534a3bd9d4c5b/src/components/Flag.tsx#L48-L78)。

**已证明通用 GeoInfo / Client schema 没有经纬度字段，本链路不是城市级或精确坐标定位。** 不推导所有第三方主题都没有地图；即使在国家代表点放标记，也不代表测得 Host 实际坐标。

### 6. 更新时机与 Settings 的真实含义

| 事件 | 实际动作 | 是否立即重算全部 Client.region |
| --- | --- | --- |
| Agent 启动 / 连接循环再建立 WebSocket 前 | UpdateBasicInfo，同时启动基础信息定时器 | 否，仅当前 Client |
| 基础信息定时上报 | 默认 **5 分钟**；每次 Server 尝试 geo，可能命中 cache | 否 |
| 普通 CPU/内存等指标 report | ingestReport 写监控数据，不调用 geo | 否 |
| IP 改变 | 没有旧/新 IP 比较门槛；下次 basicInfo 自然换 cache key | 否 |
| Server 启动 | 异步 InitGeoIp，只初始化 provider | 否 |
| Settings 改 provider | 配置事件异步 InitGeoIp，不同 provider 名称隔离 cache；不统一清除同名旧 cache | 否 |
| Settings 只改 enabled | basicInfo 每次读开关；已读热重载回调只监听 provider key | 否；不能承诺开启即正确初始化 provider |
| Settings 点击更新 | 当前 provider.UpdateDatabase 成功后 Flush cache | **否** |
| Settings 测试 IP | 查询指定 IP，可命中 cache；返回 GeoInfo，不写 Client | 否 |

依据：[Agent 主循环第 126–130 行](https://github.com/komari-monitor/komari-agent/blob/cafd4b6590b17ec3a59c9d12360e9f58f8576b61/cmd/root.go#L126-L130)、[定时器第 22–37 行](https://github.com/komari-monitor/komari-agent/blob/cafd4b6590b17ec3a59c9d12360e9f58f8576b61/server/basicInfo.go#L22-L37)、[默认间隔第 168 行](https://github.com/komari-monitor/komari-agent/blob/cafd4b6590b17ec3a59c9d12360e9f58f8576b61/cmd/root.go#L168)、[两类 ingest 第 19–45 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/client/ingest.go#L19-L45)、[启动第 14–23 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/internal/server/providers.go#L14-L23)、[配置回调第 74–78 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/internal/server/runtime.go#L74-L78)。已检查的 [scheduler 注册第 187–210 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/internal/server/runtime.go#L187-L210)没有 Geo 定时任务；这是该注册表事实，不代表所有历史版本或插件。

Settings provider 选项为 None / MaxMind / ip-api.com / geojs.io / ipinfo.io；保存走 POST /api/admin/settings：[UI 第 40–64 行](https://github.com/komari-monitor/komari-web/blob/83b42c3c4b2a52b546005ecf4cd534a3bd9d4c5b/src/pages/admin/settings/general.tsx#L40-L64)、[保存 API 第 117–125 行](https://github.com/komari-monitor/komari-web/blob/83b42c3c4b2a52b546005ecf4cd534a3bd9d4c5b/src/lib/api.ts#L117-L125)。

**手动更新链：** [UI 第 65–83 行](https://github.com/komari-monitor/komari-web/blob/83b42c3c4b2a52b546005ecf4cd534a3bd9d4c5b/src/pages/admin/settings/general.tsx#L65-L83) → `POST /api/admin/update/mmdb` → [handler 第 60–68 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/admin/update.go#L60-L68) → [geoip.UpdateDatabase 第 141–147 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/geoip.go#L141-L147)。路由名虽带 mmdb，却调用当前 provider；[IPinfo UpdateDatabase 第 81–90 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/ipinfo.go#L81-L90)是 no-op，因此只有清缓存。**success toast 不表示全部机器已重新定位，也不表示 IPinfo 数据库已下载到本机。**

**测试链：** [UI 第 84–120 行](https://github.com/komari-monitor/komari-web/blob/83b42c3c4b2a52b546005ecf4cd534a3bd9d4c5b/src/pages/admin/settings/general.tsx#L84-L120) GET /api/admin/test/geoip?ip= → admin:testGeoip；未给 IP 取 meta.RemoteIP，读 enabled，调用 GetGeoInfo，错误包装 InternalError，成功返回 GeoInfo，不 bypass cache、不更新 Client。[handler 第 171–194 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/rpc/jsonrpc/admin.system.go#L171-L194)。

**手工改 region 与刷新也不同：** admin editClient 直接 SaveClient(update)，没有 geo lookup，因此 API 能修改 region/IP，但没有独立手工覆盖来源或锁定字段，下次成功 basicInfo 可覆盖。[admin.client 第 105–119 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/rpc/jsonrpc/admin.client.go#L105-L119)。未验证专门的 region 编辑 UI：已检查的 NodeEditDialog payload 只有 name/token/remark/public_remark，不应声称当前网页有“手动选择国家”的控件。[第 13–20、128–137 行](https://github.com/komari-monitor/komari-web/blob/83b42c3c4b2a52b546005ecf4cd534a3bd9d4c5b/src/components/admin/NodeTable/NodeEditDialog.tsx#L128-L137)。

### 7. 错误、last-good、限流、超时与鉴权

- **IPinfo** 5s 超时；网络失败、非 200（含 401/403/429）、JSON 解码错误返回 error。查询函数没有专门的 Retry-After、限流队列、退避、供应商 failover 或 token refresh。缓存降低调用量，不是配额控制。[完整查询第 32–78 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/ipinfo.go#L32-L78)。GeoJS 还拒绝空 country_code；ip-api 检查响应 JSON status，但没有单独 HTTP status 检查：[geojs](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/geojs.go#L26-L69)、[ipapi](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/ipapi.go#L39-L71)。
- **空结果可能被缓存**：IPinfo 不校验 country 非空，仍可返回非 nil 的空 GeoInfo，从而缓存 48h；旗帜转换才拒绝它。不能笼统说“所有空/失败结果都不缓存”。[返回](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/ipinfo.go#L68-L78)、[缓存条件](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/geoip.go#L126-L138)。
- **只有条件性旧值保留，不是严格 last-good**：append 中 `record, _ := geoip.GetGeoInfo(ip)` 丢弃错误；无可用 flag 不注入 region，正常 Agent 未提交 region 则 DB 旧值保留。新 IP 可已更新、旧国家仍保留，且没有 geo 错误/更新时间。原始 info map 的 region 并未清除，Agent 若自行提交 region 或空值，在 geo 关闭/失败时仍可能落库；成功查询才以服务端 flag 覆盖。因此不能声称严格 Server-authoritative geo。[保存/append 第 23–79 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/client/uploadBasicInfo.go#L23-L79)。
- **Geo 查询失败本身不会让 basicInfo RPC 失败**；DB/保存错误才返回 failed to save basic info。[v2 第 63–71 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/client/report_v2.go#L63-L71)。管理员 update 失败返回 HTTP 500，成功审计+success；测试失败是 RPC InternalError：[update](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/admin/update.go#L60-L68)、[test](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/rpc/jsonrpc/admin.system.go#L171-L194)。
- **管理鉴权**：/api/admin 要求 Admin，update/test 位于此组；/api/clients/v2/rpc 要求 Admin 或 Client。[路由第 64–99 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/router/router.go#L64-L99)。身份解析优先级为管理 Bearer API key → session_token cookie → Client token → anonymous；Client token 可从查询参数提供。[principal 第 17–40 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/principal.go#L17-L40)、[Auth 第 168–176 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/web/api/Auth.go#L168-L176)。Komari 管理 key 与 IPinfo token 是不同凭据。

### 8. MMDB 和实际 LICENSE：不可混同软件许可与数据许可

Komari mmdb 默认下载 **Loyalsoldier/geoip** 的 GitHub 分发，不是凭 MaxMind 账户/license key 从 MaxMind 官方获取；这是上游现状，不是本文推荐的获取方式。[mmdb 第 17–29 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/mmdb.go#L17-L29)。文件存在时启动只加载，缺失才下载；手动更新用 http.Get（该模块无显式下载超时）、os.Create 直接覆盖，不是原子替换。initialize 先关闭旧 reader 再打开新文件；UpdateDatabase 加锁后多个错误返回没有配对 Unlock，存在失败后锁住 provider 的静态风险。以上未做运行故障注入：[初始化第 50–97 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/mmdb.go#L50-L97)、[更新第 134–164 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/utils/geoip/mmdb.go#L134-L164)。

实际读取 [Server LICENSE 第 1–21 行](https://github.com/komari-monitor/komari/blob/b11ffd3aa7cca03502a75eb64ecbe827d6831d3a/LICENSE#L1-L21)：**MIT**，Copyright (c) 2025 Komari Moniter，要求在复制品或软件重要部分保留版权/许可声明，并有 AS IS 免责声明。没有仅凭 GitHub metadata 猜许可。软件 MIT 不授予 IPinfo 服务额度，也不替代 GeoLite 数据许可；不能由 Server LICENSE 推断独立前端旗帜资产的全部许可来源。MaxMind 官方获取、署名、更新与再分发边界见已有 [GeoLite2 Country 研究](geolite2-country-acquisition.md)。本文不是法律意见，也不推荐借第三方镜像规避账户/许可。

### 9. 与 PlatPulse 的明确对照：不擅自换地图主体

| 维度 | 当前 PlatPulse | 当前 Komari |
| --- | --- | --- |
| 地理对象 | 每个 PlatON Node 的 Peer Snapshot 观察 | 受监控 Client 所代表的机器，近似 Host |
| 身份 / 去重 | 当前表主键 (node_id, peer_id) | Client UUID；IP 为属性 |
| 计数 | Network 下按 Peer 观察行 COUNT(*)，不是全网唯一 Peer/IP/Host 数 | 本链路保存 Client.region，不提供 Peer 国家观察统计 |
| 隐私 | Server 本地读取运营者提供的 GeoLite2 Country，不外发 Peer IP 做查询 | 默认 Server 向 IPinfo 发送目标机器 IP；Agent 另向回显服务联网 |
| 地理保存 | 国家代码及有限生命周期的 Peer IP cache；Public 仅国家聚合 | Client.region 旗帜 + 原始 v4/v6，无 geo 专用 freshness |
| 坐标含义 | country_centroid 静态国家代表点，不是 Peer 精确位置 | 已检查核心接口/schema 没有经纬度 |

本地依据（固定 PlatPulse HEAD）：

- [CONTEXT 第 187–205 行](https://github.com/mowind/PlatPulse/blob/b43a078aeb944fdc566c6ba9c5af3b08853e39c2/CONTEXT.md#L187-L205)：Peer Count / Peer Snapshot 按 Node、Peer ID，不按 IP 去重；Geo Database 明确是运营者提供的 local GeoLite2 Country MMDB，不捆绑、不下载、不持有 MaxMind 凭据。
- [当前 Peer 表第 1–17 行](https://github.com/mowind/PlatPulse/blob/b43a078aeb944fdc566c6ba9c5af3b08853e39c2/crates/platpulse-server/migrations/0024_peer_current.sql#L1-L17)与 [Public SQL 第 1632–1671 行](https://github.com/mowind/PlatPulse/blob/b43a078aeb944fdc566c6ba9c5af3b08853e39c2/crates/platpulse-server/src/http/public.rs#L1632-L1671)：同一个 Peer ID 被两个受监控 Node 观察，可能贡献**两条观察**；同 IP 的不同 Peer ID 也不合并成一个。当前国家计数不能称为“独立服务器数”。
- [GeoLoader 第 1–6 行](https://github.com/mowind/PlatPulse/blob/b43a078aeb944fdc566c6ba9c5af3b08853e39c2/crates/platpulse-server/src/geo.rs#L1-L6)、[国家 lookup 第 156–178 行](https://github.com/mowind/PlatPulse/blob/b43a078aeb944fdc566c6ba9c5af3b08853e39c2/crates/platpulse-server/src/geo.rs#L156-L178)只在本地 MMDB 读 country.iso_code；[centroid 第 276–299 行](https://github.com/mowind/PlatPulse/blob/b43a078aeb944fdc566c6ba9c5af3b08853e39c2/crates/platpulse-server/src/geo.rs#L276-L299)是国家代表点，不能解释成 Peer 实际经纬度。
- [录入第 307–394 行](https://github.com/mowind/PlatPulse/blob/b43a078aeb944fdc566c6ba9c5af3b08853e39c2/crates/platpulse-server/src/http/report_ingestion.rs#L307-L394)保存规范化公网 IP 与国家缓存，查询失败不延长旧国家过期，并删除无当前 Peer 引用的缓存；[常量第 17–21 行](https://github.com/mowind/PlatPulse/blob/b43a078aeb944fdc566c6ba9c5af3b08853e39c2/crates/platpulse-server/src/geo.rs#L17-L21)定义 24h cache、30d rebuild/数据库年龄边界。语义不同于无时间标记的 region 留存。

**后续实现讨论前，必须分开回答：**

1. 保留“Peer 国家观察分布”，还是增加/切换为“受监控 Host 国家分布”？一个 Agent/Host 下多个 Node 的 Host 位置不能复制计成多台物理机器，Peer IP 也不能代替 Host 出口。
2. 是否接受向第三方发送对应对象的 IP？对 Peer 使用 IPinfo 会改变现有明确的本地查询隐私设计，需要独立领域/设计决策；加缓存和 Settings 按钮不会消除外发。若选择 Host，也需单独定义 IP 来源、授权、unknown/stale、更新策略与公开投影。

本文不替用户作上述选择，不修改当前地图主体，不将 Komari 的 remote-control、自动升级等无关功能引入 PlatPulse。

## Gaps and confidence

**已由完整路径证实的不执行/未实现**（限固定版本）：IPinfo provider 没有有效 token 使用；GeoInfo / Client schema 无坐标和 geo 专用状态/时间；当前 v2 传空 fallback；手动更新不遍历 Client；metrics report 不 geo；IPinfo UpdateDatabase 不下载数据库。这些不是仅靠关键词没搜到。

**仍未验证，不能升级为保证：**

- 用户实际部署版本、打包 Web 资产及第三方主题；本文固定的是各组件当前默认分支，未核对某个具体 release manifest 或联调组合。
- 旧 ipinfo.io/{ip}/json 无 token 的当前可用性、真实限额、服务条款和结果持久化许可；未以真实 IP/凭据查询。官方 IPinfo HTML 获取有长度截断，但 endpoint、schema、鉴权与额度相关正文已读；网页没有固定提交，检索日期是边界。
- 代理/NAT/多网卡、双栈不同出口、429/超时、provider 切换并发等实际集成情况；静态源码行为不构成实测 SLA。
- 已读配置回调仅监听 provider key：若 Server 在 disabled 状态启动，之后只切 enabled 是否立即获得正确 provider，应实测，不承诺需要或不需要重启。
- 专门的每 Client 国家编辑 UI 未验证；已读 dialog 不含该字段，不等于证明全部后台/第三方主题没有此功能。
- MMDB 覆盖/锁风险是静态推断，未做故障注入；不将其扩大成对整个项目的安全审计。
- IP 国家归属精度、供应商数据时效与真实机房位置一致性不由源码保证；旗帜不是 GPS / 城市坐标。

**完成范围：**源 IP → basicInfo → provider → cache → Client.region → Settings 更新/测试已追踪。唯一新增产物为本文；未实施 IPinfo 接入、地图、schema、设置页面，也未更改 PlatPulse 现有设计。
