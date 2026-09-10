# GeoJS as an External Geo Provider: endpoint, limits, and licensing

调研日期：2026-09-10。用于 PlatPulse Server 的 GeoJS provider（issue #135）。所有结论以一手来源为准；未能证实的一律标注为"未确认"，不以推测代替事实。本文只记录事实与不变量，不扩大实现范围。

## 1. 端点与路径

| 事实 | 来源 |
|---|---|
| 单 IP JSON：`https://get.geojs.io/v1/ip/geo/{ip}.json` | [Geo 端点文档](https://www.geojs.io/docs/v1/endpoints/geo/) |
| 调用者自身：`https://get.geojs.io/v1/ip/geo.json` | 同上 |
| JSONP：`.../geo.js` 与 `.../geo/{ip}.js`（默认 callback `geoip`） | 同上 |
| **Geo 端点没有 text 版本** | 同上 |
| `ip` 查询参数支持多 IP（`?ip=a,b`），返回体变成 **JSON 数组** | 同上 |
| **没有文档化的 `fields` 参数**；实测 `?fields=country_code`、`?fields=country`、`?foo=bar` 都返回完全相同的完整对象，未知参数被静默忽略 | 实测（curl），文档中不存在该参数 |
| **HTTPS 必需**：HTTP 版本 301 重定向到 HTTPS，"a non-HTTPS version is not available" | [General 文档](https://www.geojs.io/docs/general/) |
| IPv6 字面量可直接放在路径中：`/v1/ip/geo/2001:4860:4860::8888.json` → 200 | 实测 |
| `/v1/ip/geo/{ip}`（无扩展名）→ 301 到 `{ip}.json` | 实测 |
| 响应带 `access-control-allow-origin: *` | 实测 |

**PlatPulse 不变量：** 只使用 `https://get.geojs.io/v1/ip/geo/{ip}.json` 这一固定 HTTPS 目的地，不带任何查询参数。多 IP 形式会让响应类型改变，绝不使用。

## 2. 响应字段与"无国家"的真实形状

字段集合（文档 Properties 表 + 实测）：`accuracy`、`area_code`、`asn`、`continent_code`、`country`、`country_code`、`country_code3`、`ip`、`latitude`、`longitude`、`organization`、`organization_name`、`timezone`。

- `country_code` = **两字母 ISO 3166-1 alpha-2**，文档示例 `AU`，并链接 MaxMind 的 ISO 3166 列表。
- `country` = **英文国家全名**，示例 `Australia`。
- `latitude` / `longitude` 是**字符串**（"due to historic reasons"），未知时为字面量 `"nil"`，不是 null。

**未知 / 无国家地址的真实响应（关键）：HTTP 200，且国家相关键整体缺失——不是 null，也不是空字符串。**

```json
{"area_code":"0","asn":64512,"ip":"192.0.2.1","latitude":"nil","longitude":"nil","organization":"AS64512 Unknown","organization_name":"Unknown"}
```

实测 `192.0.2.1`、`10.0.0.1`、`0.0.0.0`、`::1`、`2001:db8::1` 以及真实的 `1.1.1.1` 都是这个形状。未知时 `asn` = `64512`、`organization_name` = `"Unknown"`。

**畸形 IP → HTTP 404，返回 openresty HTML 错误页（不是 JSON）**：`/v1/ip/geo/notanip.json`。PlatPulse 只发送经 Server 校验的规范公网字面量，所以这条路径在实现中不可达；即使出现也按失败处理。

### 陷阱：同族的 `/v1/ip/country/` 端点字段名完全不同

[`/v1/ip/country/{ip}.json`](https://www.geojs.io/docs/v1/endpoints/country/) 中 `country` **才是** alpha-2 代码，`name` 是国家名，`country_3` 是 alpha-3；未知时返回**空字符串**：`{"country":"","country_3":"","ip":"192.0.2.1","name":""}`。与 `/geo/` 端点语义相反。两个端点的字段拼写与空值语义不可互换。

**PlatPulse 不变量：** `/geo/` 端点读 `country_code`；对象缺少该键（或为 null / 空串）是权威的 NoCountry；该键存在但不是两字母 ASCII 大写码是畸形结果，按失败处理。GeoJS 的 `country` 全名在任何情况下都不当国家码读取。

## 3. 限流、公平使用与鉴权

- "As it currently stands GeoJS has no rate limits and doesn't require any to be implemented. Should rate limits be required advanced warning will be given where possible."（[General 文档](https://www.geojs.io/docs/general/)）；主页写 "No rate limits (yet)"。
- [TOS](https://www.geojs.io/tos/)："You must not use an excessive amount of API requests and querying. What is considered excessive is decided at the sole discretion of GeoJS."，并保留限流（throttling）与封禁特定账号/IP 的权利。
- **无 API key、无 token、无任何鉴权**；匿名请求成功。实测响应中没有限流相关响应头。
- 响应经 Cloudflare 缓存（`cf-cache-status: HIT`、`cache-control: max-age=0, must-revalidate`、`geojs-backend: ash-01`）。

**PlatPulse 不变量：** 无凭据；沿用共享外发边界的固定目的地、有界超时、有界响应体、有限并发与按 provider 隔离的有界 429 退避。本地按 provider+IP 持久缓存是主要的用量控制手段，不是配额承诺。文档没有承诺任何额度，实现也不把它当作承诺。

## 4. 许可与署名

- **GeoJS 自身 TOS 没有署名条款。** 全文（acceptable use / termination / API usage / changes / disclaimer / liability / indemnification / 澳大利亚法）不含 "Powered by GeoJS" 或任何形式的 attribution 要求。
- GeoJS 主页的 "Notes & Acknowledgements" 写明其 GeoIP 数据来源：*"I'd like to thank Telize for inspiring me to create this and MaxMind, as all GeoIP data is sourced from their GeoLite database, available at maxmind.com."*（[geojs.io](https://www.geojs.io/)）
- 上游 MaxMind 的义务（[GeoLite2 EULA](https://www.maxmind.com/en/geolite2/eula) §3）："You must provide attribution of your use to MaxMind (an example of attribution: **"This product includes GeoLite Data created by MaxMind, available from https://www.maxmind.com."**)"；可版权元素按 CC BY-SA 4.0 提供。这是唯一找到的、逐字要求的署名句子。
- **未确认：** MaxMind 的署名义务是否从 GeoJS（MaxMind 的被许可方）传递到 GeoJS API 的下游消费者。GeoJS TOS 与 MaxMind EULA 都没有直接规定第三方 API 消费者。
- GeoJS 网站源码为 MIT（[jloh/geojs-io](https://github.com/jloh/geojs-io)，"Copyright (c) 2017 James Loh"），**只覆盖网站代码，不覆盖 API 或其数据**。

**PlatPulse 决策：** 采取保守读法。GeoJS 被选为 provider 时，Public 署名同时给出 GeoJS 来源与 MaxMind 要求的原句（`geo_geojs::GEOJS_ATTRIBUTION`）。GeoJS 的 TOS 不要求署名这一事实不改变"数据来自第三方、必须如实标注来源"的既有产品规则。

## 5. 可用性与免费使用

- **免费、无定价页、无付费档、无商用限制。** TOS 明确考虑公司主体："If you are entering into this agreement on behalf of a company or other legal entity, you represent that you have the authority to bind such entity"；同时保留"随时更改服务及关联附加服务价格"的权利。
- **单一后端、无负载均衡。** 运营者博客 [GeoJS has moved to Hetzner](https://jloh.co/posts/geojs-moving-to-hetzner/)（2025-01-08）：自 2024-09 起 GeoJS 运行在**一台** Hetzner CPX11（Ashburn, VA），移除了 Route53/地理路由，"A small amount of downtime in the event of an outage for a free platform is more than fair"。这与文档中 2018 年"Highly available and geo routed"的说法**矛盾**，以博客为当前事实。
- 规模：博客 [GeoJS 2024 update](https://jloh.co/posts/geojs-2024/) 称 2024-12 处理 86 亿请求、全年 82 TB。状态页 <https://status.geojs.io/>（updown.io，1 分钟间隔）监控 get.geojs.io / ipv4 / ipv6；调研时全部 200。**无 SLA。**
- TOS：服务按 "as is"/"as available" 提供，"GeoJS gives no warranties regarding the correctness of the data"，并可在邮件通知后随时终止。
- 另有仅 IPv4 / 仅 IPv6 的主机：`https://ipv4.geojs.io/`、`https://ipv6.geojs.io/`（[General 文档](https://www.geojs.io/docs/general/)）。

**未确认：** 没有任何一手来源指明 GeoJS 背后的法律实体。网站只署名个人 "jloh"（James Loh）与 contact@geojs.io，GitHub 仓库属主为 jloh。"Jingcha Inc" 无一手来源支持，不得作为事实陈述。

**PlatPulse 不变量：** 单一后端、无 SLA，因此 GeoJS 只能被当作有界、可失败的外部依赖：慢或失败不得影响 Report Ingestion、Node 健康或 Home 浏览；失败只记录尝试、保留 last-good；绝不自动回退到另一个 provider。

## 6. 未实测 / 未确认项

- 未对真实服务发起联调（需另行授权）：本次只做了文档读取与少量匿名探测，未发送任何真实 Peer IP，未验证真实限流行为与响应头。
- GeoJS 当前实际服务的 GeoLite 数据版本未确认。响应中仍带 GeoLite **Legacy** 专有字段 `area_code`、`country_code3`，而 MaxMind 已于 2019-01-02 停止 GeoLite Legacy（[MaxMind 公告](https://blog.maxmind.com/discontinuation-of-the-geolite-legacy-databases/)），GeoLite2 schema 中也没有这两个字段（[GeoLite2 字段文档](https://dev.maxmind.com/geoip/docs/databases/city-and-country/)）。据此**推测**（非确认）GeoJS 可能仍在提供旧格式、甚至已冻结的数据。PlatPulse 不依赖数据新鲜度，也不缓存超过自身保留窗口的结果。
- MaxMind 署名义务对下游 API 消费者的适用性未确认（见 §4）。
- GeoJS 的 `country_code` 取值集合未逐一校验；PlatPulse 只做形状校验（两个大写 ASCII 字母），不固化一份会过期的国家清单。

## 7. 实现对照

| 要求 | 实现位置 |
|---|---|
| 固定 HTTPS `/v1/ip/geo/{ip}.json` | `geo_geojs::GEOJS_ENDPOINT_BASE` + `path_prefix`/`path_suffix`（编译期 profile） |
| 读 `country_code` 并校验国家码 | `ExternalProfile::country_field` + `geo::is_country_code` |
| 其他文本/空字段/HTML/失败响应都不成为有效国家 | `geo_external::ExternalGeoClient::request` |
| 有界并发、超时、429 退避、不跟随重定向 | 共享 `geo_external` 边界 + `GEOJS_MAX_CONCURRENCY` 等常量 |
| 按 provider 隔离缓存与迟到结果 | `geo_location_cache` 的 provider 主键 + generation 校验 |
| 署名 | `geo_geojs::GEOJS_ATTRIBUTION` |
