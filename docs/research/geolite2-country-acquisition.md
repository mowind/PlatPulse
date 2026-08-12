# Research: GeoLite2 Country 当前获取、许可与更新方式（仅 MaxMind 官方资料）

> 调研范围：GeoLite2 Country 免费可下载数据库，不讨论第三方镜像、非官方下载器或第三方容器。  
> 调研日期：2026-08-11（以引用页面当前内容为准）。许可条款可能更新；上线或再分发前应复核 MaxMind 当时生效的英文协议。本文不是法律意见。

## Summary

GeoLite2 Country 仍是 MaxMind 的免费 GeoLite 数据库，但**免费不等于匿名公开下载或无条件再分发**：应通过 GeoLite 专用入口注册并接受 GeoLite EULA；自动下载必须使用 MaxMind 账户 ID 与有效 license key。二进制 MMDB 官方首选由 `geoipupdate` 更新，CSV 或不能运行该工具的环境使用账户门户提供的 permalink 直接下载；容器可使用 MaxMind 官方 `ghcr.io/maxmind/geoipupdate` 镜像，把 `/usr/share/GeoIP` 挂载到持久卷。

普通 GeoLite EULA 要求 MaxMind 署名、及时更新，并在新版发布后 30 天内停止使用和销毁旧版。将数据库随产品交付给客户/用户属于 MaxMind 明确要求购买 **Commercial Redistribution License for GeoLite** 的场景；付费 GeoIP/GeoIP2 是精度、数据、支持、下载额度和许可体系均不同的商业产品，不能与免费 GeoLite2 混称或套用同一许可结论。

## Findings

### 1. 免费 GeoLite2 Country 的当前获取入口与账户要求

1. **必须从 GeoLite 专用注册流程取得 GeoLite 权限；普通“空账户”本身不授予 GeoLite。** MaxMind 官方账户文档把“sign up for free GeoLite services”列为创建账户的一种方式，并写明：

   > “Access IP geolocation databases and web services free of charge for development, personal, or community use. You can sign up for GeoLite on our main website.”

   同一页面特别说明，普通免费 MaxMind 账户：

   > “Signing up for a free MaxMind account will not give you access to GeoLite or require signing our product End User License Agreements (EULAs).”

   因而当前正确入口是 [GeoLite sign up](https://www.maxmind.com/en/geolite2/signup)，而不是只创建一个不带产品权限的通用账户。  
   来源：[Create a MaxMind account](https://support.maxmind.com/knowledge-base/articles/create-a-maxmind-account)

2. **账户门户可手工下载 MMDB/CSV；自动化下载需要账户 ID 和 license key。** 官方下载文档说明，登录账户门户的 `GeoIP / GeoLite → Download Files` 后，会显示账户已订阅的数据库，并提供 GZIP、ZIP 和 SHA256 下载链接：

   > “We offer direct downloads of both binary and CSV format databases through your account portal.”

   > “To download a database, click on the GZIP, ZIP, or SHA256 link...”

   自动化或 API 访问则不同。官方 license key 文档明确写道：

   > “You will need a license key and your account ID number if you want to query one of the web services or automate database downloads.”

   所以应明确区分：**门户手工下载依赖账户登录与产品权限；`geoipupdate`、脚本/API permalink 下载依赖 account ID + active license key。**  
   来源：[Download and update databases](https://support.maxmind.com/knowledge-base/articles/download-and-update-maxmind-databases)、[Using MaxMind license keys](https://support.maxmind.com/knowledge-base/articles/using-maxmind-license-keys)

3. **license key 只能由 MaxMind 账户生成，且应按密码管理。** 官方原文：

   > “You may only obtain a license key if you have an account with MaxMind.”

   > “the license key will only be displayed once”

   > “it should be treated with the same security as a password.”

   生成位置是账户门户 `Manage License Keys`；完整 key 生成后不可恢复，遗失需替换。部署时不应把 key 烘焙进应用镜像、提交到仓库或向下游用户分发。  
   来源：[Generate a license key](https://support.maxmind.com/knowledge-base/articles/generate-a-maxmind-license-key)、[Using MaxMind license keys](https://support.maxmind.com/knowledge-base/articles/using-maxmind-license-keys)

4. **GeoLite2 Country 当前发布频率为每周二、周五，GeoLite 账户每天最多 30 次直接下载。** MaxMind 当前更新表列出：

   > “GeoLite Country — Every Tuesday and Friday”

   并说明：

   > “Every account is limited to 1,000 total direct downloads (30 for GeoLite accounts) in a 24-hour period.”

   检查更新的 HEAD 请求不计入该限制；多服务器部署不应让每个节点独立重复下载。  
   来源：[Download and update databases](https://support.maxmind.com/knowledge-base/articles/download-and-update-maxmind-databases)

### 2. GeoLite2 EULA、署名和再分发边界

5. **GeoLite2 是免费数据库，但使用即接受 GeoLite EULA；“GeoLite”和“GeoLite2”在协议中同义。** EULA 原文：

   > “By downloading or using our GeoLite Database, you are accepting and agreeing to the terms and conditions...”

   > “Due to rebranding ‘GeoLite’ may be used with the same meaning as ‘GeoLite2’.”

   EULA 还将其定义为 “a line of free databases”，并在费用条款写明：

   > “The Services are made available to you free of charge.”

   但 MaxMind 保留未来停止免费提供更新的权利。  
   来源：[GeoLite End User License Agreement](https://www.maxmind.com/en/geolite/eula)

6. **普通 GeoLite 使用必须向 MaxMind 署名，且许可不是只有 CC BY-SA 4.0 一页。** 当前 EULA 将 Creative Commons Attribution-ShareAlike 4.0 纳入协议，同时规定冲突时 GeoLite EULA 优先。其授权条款明确要求：

   > “You must provide attribution of your use to MaxMind”

   并给出示例：

   > “This product includes GeoLite Data created by MaxMind, available from https://www.maxmind.com.”

   因此合规表述应是“受 GeoLite EULA（其中纳入 CC BY-SA 4.0）约束”，而不应简化为“数据库完全按 CC BY-SA 自由使用”。还不得移除或遮蔽数据库中的版权、使用条款等通知。  
   来源：[GeoLite End User License Agreement](https://www.maxmind.com/en/geolite/eula)

7. **必须及时更新，并在新版发布后 30 天内停止使用、销毁旧数据库及旧数据。** EULA 原文：

   > “you agree to promptly use the updated version”

   > “cease use of and destroy ... any old versions ... within thirty (30) days following the release of the updated GeoLite Databases”

   这对离线、容器和本地缓存部署同样重要：旧镜像层、离线安装包、备份/制品库中的可用副本也需要纳入生命周期控制，而不能无限保留并继续使用。  
   来源：[GeoLite End User License Agreement](https://www.maxmind.com/en/geolite/eula)、[Maintain up-to-date data](https://support.maxmind.com/knowledge-base/articles/maintain-up-to-date-data)

8. **普通 EULA 并非无条件允许把数据库交给第三方。** 当前 EULA 的披露条款是：

   > “Except as explicitly permitted by the Creative Commons License, you will not disclose the Services to any third party without notifying MaxMind ... and obtaining MaxMind's prior written consent...”

   若按协议许可向第三方披露，还必须施加相同或实质相似的合同义务，并对第三方行为负责。由此可见，不能把“应用可查询 GeoLite 结果”和“把完整 MMDB/CSV 数据库副本交付给客户”视为同一件事。

   对产品化交付，MaxMind 知识库给出了更直接的操作结论：

   > “If you would like to include data from MaxMind’s GeoLite databases in a product or service you provide to your users or customers, you will need a Commercial Redistribution License for GeoLite.”

   **风险等级：高。** 若产品安装包、容器镜像、设备固件或客户可访问卷中包含完整 `GeoLite2-Country.mmdb`，上线前应按商业再分发场景处理并向 MaxMind 确认，而不是仅添加署名就默认合规。  
   来源：[GeoLite End User License Agreement](https://www.maxmind.com/en/geolite/eula)、[Commercial License for GeoLite](https://support.maxmind.com/knowledge-base/articles/commercial-license-for-geolite)

9. **Commercial GeoLite License 是按“一个 Licensee Product”授权的年度再分发许可，不等于解除全部限制。** 商业协议原文允许：

   > “distribute an unlimited number of copies of the Named MaxMind Products within one Licensee Product, in return for an annual fee.”

   多于一个产品需额外年度费用。下游最终用户必须同意包含规定实质内容的 EULA，其中数据库仅限内部用途、权利不可转让且非独占；仍禁止用数据定位特定家庭、个人或街道地址；仍要求 30 天内销毁旧版。  
   来源：[MaxMind Commercial GeoLite License Agreement](https://www.maxmind.com/en/geolite-commercial-redistribution-license)

10. **无论普通还是商业 GeoLite，均不得用于定位特定个人、家庭或街道地址；还不得用于 FCRA 用途。** 普通 EULA 写明不得：

    > “use or encourage others to use the GeoLite Data for the purpose of identifying or locating a specific household, individual, or street address.”

    且列举禁止将其用于信用、保险、就业、政府许可/福利等 FCRA 决策。  
    来源：[GeoLite End User License Agreement](https://www.maxmind.com/en/geolite/eula)

### 3. 必须区分免费 GeoLite2 与付费 GeoIP2/GeoIP

11. **GeoLite 是免费层；GeoIP 是付费商业层，二者格式/集成接近但产品能力不同。** MaxMind 官方比较指出 GeoIP 数据可作为 GeoLite 的近似 drop-in replacement，但差异包括精度、可用数据、官方支持、价格和许可权利：

    > “the commercial GeoIP database is more accurate.”

    > “We do not provide official support for GeoLite databases and web services”

    GeoLite Country 提供免费国家级数据库；付费 GeoIP Country 使用更多数据源、工作日更新，并有官方支持及更高下载额度。GeoLite 页面还对比列出 GeoLite Country 30 downloads/day、GeoIP Country 1,000 downloads/day，并标示 GeoLite “Attribution required”。  
    来源：[Upgrade from GeoLite](https://support.maxmind.com/knowledge-base/articles/upgrade-from-geolite)、[GeoLite: free GeoIP data](https://www.maxmind.com/en/geolite-free-ip-geolocation-data)

12. **不要把免费 GeoLite EULA 套用于付费 GeoIP 数据库。** MaxMind 明确说明：

    > “GeoIP databases can only be used for internal restricted business purposes without a commercial license”

    而 GeoLite 可在遵守 GeoLite 许可和署名要求的前提下用于产品/服务；需要再分发数据库时仍需 Commercial GeoLite License。付费 GeoIP 数据库通常受 MaxMind Online EULA 或客户的定制合同约束，商业对外用途/再分发也需要相应商业许可。  
    来源：[Upgrade from GeoLite](https://support.maxmind.com/knowledge-base/articles/upgrade-from-geolite)、[MaxMind End User License Agreement](https://www.maxmind.com/en/end-user-license-agreement)

### 4. GeoIP Update 官方工具

13. **`geoipupdate` 是 MaxMind 官方二进制数据库自动更新工具；不支持 CSV。** 官方仓库 README：

    > “The GeoIP Update program performs automatic updates of GeoIP and GeoLite binary databases. CSV databases are not supported.”

    Developer Portal 也说明二进制格式“highly recommend using GeoIP Update”，CSV 必须直接下载。当前官方发行支持 Linux、macOS 和 Windows，并提供发行包、Ubuntu PPA、deb/rpm、Homebrew 与容器方式。  
    来源：[maxmind/geoipupdate](https://github.com/maxmind/geoipupdate)、[Updating GeoIP and GeoLite Databases](https://dev.maxmind.com/geoip/updating-databases/)

14. **GeoLite2 Country 的最小配置是 `AccountID`、`LicenseKey`、`EditionIDs GeoLite2-Country`。** 官方配置模板：

    ```text
    AccountID YOUR_ACCOUNT_ID_HERE
    LicenseKey YOUR_LICENSE_KEY_HERE
    EditionIDs YOUR_EDITION_IDS_HERE
    ```

    官方容器文档列出的 GeoLite edition IDs 包括 `GeoLite2-ASN GeoLite2-City GeoLite2-Country`。因此只更新 Country 时应配置：

    ```text
    EditionIDs GeoLite2-Country
    ```

    可从账户门户下载部分预填的 `GeoIP.conf`，再填入 active license key；这比手写配置更受官方推荐。Linux/Unix 可用 cron 周期运行。防火墙需允许 DNS 与 HTTPS/443。  
    来源：[Updating GeoIP and GeoLite Databases](https://dev.maxmind.com/geoip/updating-databases/)、[Official GeoIP Update Docker documentation](https://github.com/maxmind/geoipupdate/blob/main/doc/docker.md)

15. **下载端必须跟随 HTTPS 重定向并允许访问 MaxMind 的 R2 存储主机。** 自 2024 年起，数据库下载 permalink 会重定向至：

    ```text
    mm-prod-geoip-databases.a2649acb697e2c09b632799562c076f2.r2.cloudflarestorage.com
    ```

    若企业防火墙、代理或 allowlist 只放行 `download.maxmind.com` / `updates.maxmind.com`，更新仍可能失败。MaxMind 要求 HTTPS；旧版 `geoipupdate` 应升级到 4.x 或更高以满足 TLS 1.2+ 要求。  
    来源：[Updating GeoIP and GeoLite Databases](https://dev.maxmind.com/geoip/updating-databases/)

16. **不能运行 `geoipupdate` 时，使用门户生成的 permalink + Basic Authentication 直接下载；不要硬编码猜测 URL。** 官方步骤要求从账户门户 `Get Permalink(s)` 复制链接，并以 account ID / license key 作 Basic Auth；客户端必须跟随重定向。建议先用 HEAD 检查 `Last-Modified`，有新版本再下载；HEAD 不计每日下载额度。所有更新都是完整数据库，不是增量包。  
    来源：[Updating GeoIP and GeoLite Databases](https://dev.maxmind.com/geoip/updating-databases/)、[Download and update databases](https://support.maxmind.com/knowledge-base/articles/download-and-update-maxmind-databases)

### 5. 官方容器方式与离线/隔离网络部署

17. **容器使用 MaxMind 官方 GHCR 镜像，并把数据库目录持久化到宿主机卷。** 当前官方仓库文档给出的镜像是：

    ```text
    ghcr.io/maxmind/geoipupdate
    ```

    最小运行形式：

    ```sh
    docker run --env-file <file> \
      -v <database-directory>:/usr/share/GeoIP \
      ghcr.io/maxmind/geoipupdate
    ```

    Country-only 环境变量可配置为：

    ```text
    GEOIPUPDATE_ACCOUNT_ID=...
    GEOIPUPDATE_LICENSE_KEY=...
    GEOIPUPDATE_EDITION_IDS=GeoLite2-Country
    ```

    默认未设置 `GEOIPUPDATE_FREQUENCY`（或设为 `0`）时只运行一次后退出；设为大于 0 的小时数才会周期执行。数据库默认写入 `/usr/share/GeoIP`。  
    来源：[Official GeoIP Update Docker documentation](https://github.com/maxmind/geoipupdate/blob/main/doc/docker.md)

18. **容器凭据优先使用 secret 文件，而非明文环境变量或镜像层。** 官方镜像支持：

    ```text
    GEOIPUPDATE_ACCOUNT_ID_FILE=/run/secrets/GEOIPUPDATE_ACCOUNT_ID
    GEOIPUPDATE_LICENSE_KEY_FILE=/run/secrets/GEOIPUPDATE_LICENSE_KEY
    ```

    并给出 Docker Compose secrets 示例。鉴于 license key 被官方要求按密码保护，生产部署应让专门的更新容器持有凭据，业务容器只读挂载 MMDB，不应把 key 分发给每个业务实例。  
    来源：[Official GeoIP Update Docker documentation](https://github.com/maxmind/geoipupdate/blob/main/doc/docker.md)、[Using MaxMind license keys](https://support.maxmind.com/knowledge-base/articles/using-maxmind-license-keys)

19. **离线/air-gapped 环境没有“完全离线自动更新”；官方资料支持的模式是联网侧集中下载，再在内部网络分发完整数据库。** `geoipupdate` 本身要求当前更新访问权限和 HTTPS 网络；MaxMind 对多服务器场景明确建议：

    > “download databases to a local repository on your network, and distribute them to other servers from there.”

    因而可落地为：

    1. 在允许出网的受控更新主机或更新容器上，用 `geoipupdate` 获取 `GeoLite2-Country.mmdb`；或用门户 permalink 下载压缩包与 SHA256。
    2. 在联网侧验证官方 SHA256，记录数据库发布日期/制品摘要。
    3. 通过组织批准的离线介质或单向制品通道，把完整 MMDB 送入隔离区内部制品库。
    4. 在隔离区原子替换业务读取的文件/只读卷；保留回滚窗口时也必须确保旧版在新版本发布 30 天内停止使用并销毁。
    5. 由一个集中更新点向多个节点分发，避免触发 GeoLite 每日 30 次直接下载限制，也避免把 MaxMind license key 放进隔离区每个应用实例。

    上述第 2–4 步的具体传输机制由部署方决定；MaxMind 官方没有提供绕过联网下载的增量更新或离线授权服务器。**“联网下载后内部复制”是根据其本地 repository 建议、完整数据库更新模型、SHA256 下载能力及 EULA 更新义务形成的部署方案。**  
    来源：[Download and update databases](https://support.maxmind.com/knowledge-base/articles/download-and-update-maxmind-databases)、[Updating GeoIP and GeoLite Databases](https://dev.maxmind.com/geoip/updating-databases/)、[GeoLite End User License Agreement](https://www.maxmind.com/en/geolite/eula)

## 建议的 PlatPulse 决策

- 若 GeoLite2 Country 只由组织内部的服务读取、数据库文件不交付第三方：注册 GeoLite，使用专用更新账户/key，由官方 `geoipupdate` 容器集中下载，业务服务只读挂载 MMDB，并显示/文档化 MaxMind 署名。
- 若 PlatPulse 的发行容器、安装包或离线 bundle 要交给客户且其中包含 MMDB：**高风险许可事项**，在发布前购买/确认 Commercial Redistribution License for GeoLite；每个不同产品可能需要独立年度许可，并落实下游 EULA。
- 若不想承担 GeoLite 署名/再分发解释空间，或需要更高精度、工作日更新、官方支持和更高下载额度，应评估付费 GeoIP Country 及对应商业许可，而不是把 GeoIP2 当作“同一个免费库的新版”。
- 更新系统应监控数据库发布日期/文件摘要，并保证所有活动副本在新版发布后 30 天内淘汰；备份、旧容器层和离线安装介质也应纳入清理清单。

## Sources

### Kept（均为 MaxMind 官方一手资料）

- [GeoLite End User License Agreement](https://www.maxmind.com/en/geolite/eula) — 免费 GeoLite 的主许可文本、署名、披露、更新/销毁和禁止用途。
- [MaxMind Commercial GeoLite License Agreement](https://www.maxmind.com/en/geolite-commercial-redistribution-license) — 商业再分发权利、按产品收费、下游 EULA 和持续限制的主协议。
- [Commercial License for GeoLite](https://support.maxmind.com/knowledge-base/articles/commercial-license-for-geolite) — MaxMind 对“产品/服务中包含 GeoLite 数据库即需商业再分发许可”的直接说明。
- [Create a MaxMind account](https://support.maxmind.com/knowledge-base/articles/create-a-maxmind-account) — GeoLite 专用注册与普通空账户不授予 GeoLite 的区别。
- [Generate a license key](https://support.maxmind.com/knowledge-base/articles/generate-a-maxmind-license-key) — key 的账户前提、生成、一次性展示和保密要求。
- [Using MaxMind license keys](https://support.maxmind.com/knowledge-base/articles/using-maxmind-license-keys) — 自动下载需要 account ID + key，以及凭据安全要求。
- [Download and update databases](https://support.maxmind.com/knowledge-base/articles/download-and-update-maxmind-databases) — 门户下载、频率、额度、本地仓库分发、SHA256 与完整更新模型。
- [Updating GeoIP and GeoLite Databases](https://dev.maxmind.com/geoip/updating-databases/) — `geoipupdate`、配置、直接下载、HTTPS/R2 重定向和自动化官方指南。
- [maxmind/geoipupdate](https://github.com/maxmind/geoipupdate) — MaxMind 官方工具仓库、平台安装方式及 CSV 不支持说明。
- [Official GeoIP Update Docker documentation](https://github.com/maxmind/geoipupdate/blob/main/doc/docker.md) — 官方容器镜像、环境变量、secrets、卷与运行频率。
- [Upgrade from GeoLite](https://support.maxmind.com/knowledge-base/articles/upgrade-from-geolite) — GeoLite 与付费 GeoIP 在精度、数据、支持、价格和许可上的官方比较。
- [GeoLite: free GeoIP data](https://www.maxmind.com/en/geolite-free-ip-geolocation-data) — 免费/付费层、额度与能力对照。
- [Maintain up-to-date data](https://support.maxmind.com/knowledge-base/articles/maintain-up-to-date-data) — 30 天删除旧数据义务的官方解释。

### Dropped

- 第三方 Docker 镜像、镜像站、博客、Stack Overflow、发行版教程 — 非 MaxMind 一手资料，不用于任何结论。
- GitHub issues/非文档提交 — 虽位于官方仓库，但不如当前正式文档稳定，未作为规范依据。
- 搜索摘要中的历史 `support.maxmind.com/hc/...` 重复页面 — 与当前 Knowledge Base 页面内容重复，保留当前 canonical 页面。

## Gaps / Residual risks

1. **高：具体产品交付是否构成“再分发”取决于实际架构和合同事实。** 本文可确认 MaxMind 对“产品/服务中包含数据库”的官方立场，但不能替代 MaxMind 销售/法务对 PlatPulse 具体交付方式的书面确认。
2. **中：GeoLite EULA 可被 MaxMind 更新。** 当前页面显示的版本日期应在发布前再次核对，并保存当时接受的协议版本。
3. **中：离线制品中的备份/回滚副本如何“销毁”应由组织合规流程定义。** 官方明确 30 天期限，但未给出备份系统、不可变对象存储或旧镜像层的技术实施细则。
4. **低：官方 Developer Portal 仍链接 Docker Hub 镜像，而当前官方 GitHub 仓库文档指定 GHCR。** 本文采用当前官方仓库的 `ghcr.io/maxmind/geoipupdate`；部署时应固定受信任版本或 digest，并跟踪官方发布。

## Review findings

- **high:** 客户交付物若内含 `GeoLite2-Country.mmdb`，不得仅凭“GeoLite 免费 + 已署名”假定可再分发；需 Commercial Redistribution License 或 MaxMind 书面确认。
- **high:** 自动下载所用 license key 是账户级敏感凭据，不应烘焙进 PlatPulse 应用镜像、写入仓库或提供给客户。
- **medium:** 更新流程必须覆盖新版发布后 30 天内停止使用并销毁旧版，包括离线包、旧镜像和可恢复备份中的活动副本。
- **medium:** 网络 allowlist 必须允许 MaxMind 当前 R2 presigned URL 目标主机，否则只放行 `updates.maxmind.com` 仍会下载失败。
- **no blocker:** 已能用 MaxMind 官方一手资料回答获取、账户/key、EULA/再分发、官方更新工具、容器与离线更新方式，并明确区分 GeoLite2 与付费 GeoIP2。
