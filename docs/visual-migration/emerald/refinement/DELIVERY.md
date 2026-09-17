# 交付记录

## 提交与环境

- 分支：`feat/webui-emerald-visual-migration`。
- 修复前：`a1eb85ed220b5ec3760930cfbf4a167af80f0951`。
- 前端代码及全部 after 截图来源：`1bae37fcda1cdd06eda7e36fe8e1d23d8b5bcd08`。
- 随后的证据提交仅增加文档、截图、日志，以及一条既有地图测试的布局约束修正；没有再改变前端呈现源码。
- Arch Linux x86_64；Node `v26.8.2`；Chromium headless `151.0.7922.34`。
- 系统字体栈 `system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif`；没有引入Web字体/CDN。
- 真实临时 `platpulse-server`、临时 SQLite、生产 Vite bundle、同源地图；不是Vite开发模式截图。
- 360×800、390×844、768×1024（前三项 `hasTouch: true`），1280×800、1440×900。
  这是固定CSS视口/触摸模拟，不是真机Safari验证。
- Home场景使用 `emerald-refinement.spec.ts` 内确定性DTO夹具；Admin/登录用实际临时Server。每个视口的 `environment.json` 记录全部9个Home场景的控件几何。
- `before` 与 `after` 各70张：9种Home场景、正常暗色、Login浅/深色、Admin浅/深色 × 5个视口。全部after metadata的commit相同。

生产入口校验：

```text
before dist/index.html SHA256 b13063792807981ddcf51dfe8140f352dfeb1edf9db18c24e2782e452e74ffb4
after  dist/index.html SHA256 8ed9cf2bf1e78844cc6bf60b600816b3c78902ecc947c2ee31074698cce4413c
```

## 实际几何（CSS px）

以下取1440px视口 normal 场景；其余视口/场景原始记录见相应environment.json。
这里的“着色高”是border内的padding-box，不把透明点击边界当作可见外壳。

| 控件 | 修复前外框 宽×高 | 修复后外框 宽×高 | 修复后着色高 / 交互高 |
|---|---|---|---|
| Tabs外壳 | 241.22×44 | 241.22×32 | 32 / 非交互容器 |
| All Networks触发器 | 102.41×44 | 102.41×26 | 26 / 44，伪元素真实命中 |
| Mainnet、Testnet触发器 | 各66.41×44 | 各66.41×26 | 26 / 44，逐项滚动后命中上下边缘 |
| 排序select | 137×44 | 137×44 | 32 / 44 |
| 页头Theme/Admin图标按钮 | 44×44（着色42） | 44×44 | 32 / 44 |

卡片另有浏览器断言：标题区padding `12px 16px`、正文区 `0px 16px 16px`，标题16px；指标12px。
标题和值的基线/左右位置、无横向溢出仍沿用并执行已有 `expectMetricRowsAligned`。
超长值保持全值，允许换行；L/C按行容器宽度启用，不靠缩小字体。

## 已执行测试与失败记录

1. **修复前单元回归**：2 failed / 43 passed。证明Radix状态选择器不匹配、统计不跟随筛选。
2. **旧生产bundle上的浏览器复现**：390/1440两个项目均失败在选中背景仍透明，不是只观察数据变化。
3. **修复前截图生成**：10 passed；这是带 `REFINEMENT_BASELINE=1` 的证据模式，明确不作为新版契约通过证明。
4. **修复后单元**：26 files / **297 passed**。
5. **lint / typecheck**：通过。生产build由Playwright启动脚本重新执行并成功启动真实Server；交付前又单独执行 `npm run build`，退出码0，日志见 `logs/after-build.log`。
6. **首轮完整矩阵及after截图**：170项中 **145 passed / 24 skipped / 1 failed**。
   失败为旧测试要求canvas完整塞入200px内容带；该断言在修复前的测试源码中也存在，与既有 `6b78964` 的溢出2:1地图决定冲突。
   未改变地图来迎合断言：保留覆盖并替换为固定200px内容带、2:1 canvas、内容带与工具栏不重叠、真实点击命中控件四项约束。
7. **修正旧地图约束后的完整重跑**：新增执行 `map-size.spec.ts`，总计175项，**147 passed / 28 skipped / 0 failed**，耗时4.3分钟。跳过项均为原有按视口归属的条件跳过，相应项目上的用例实际执行。日志见 `logs/after-final-matrix.log`。

最终重跑命令：

```sh
cd platpulse-web
npx playwright test \
  e2e/emerald-refinement.spec.ts \
  e2e/home-convergence.spec.ts \
  e2e/home-geo-map.spec.ts \
  e2e/emerald-decisions.spec.ts \
  e2e/shell.spec.ts \
  e2e/admin-list-matrix.spec.ts \
  e2e/map-size.spec.ts
```

截图命令使用相同前六个spec并设置 `REFINEMENT_EVIDENCE=after`；首次完整矩阵日志完整保留，不用重录图片隐藏断言失败。

构建仍有现有的 >500kB chunk 警告；单元测试仍输出部分已有的React `act(...)` 环境警告。均没有当成失败隐藏，也没有在本轮扩展到构建拆包或测试基础设施改造。

## 场景解释与剩余差异

- normal是Geo完整且当前，节点包含Healthy与Unknown以覆盖Attention统计；不是把Unknown节点伪装Healthy。
- unknown保留已知Node data，但CPU/Memory、Head、Consensus缺测；未知资源无progressbar并保留占位。
- stale/error/disabled/partial截图聚焦地图状态；Node共识last-good和Stale、权威Peers=0、未观测节点P由真实Server种子的`home-convergence`回归独立覆盖。
- 长名称有可访问的完整文本；长数值使用JS可精确表示的最大安全整数，标签仍可见、数值不裁掉、不缩字。
- 多网络单独截图选择Testnet，统计、卡片、国家范围同步；键盘测试还往返Mainnet/全部网络，并验证实际选中背景和文字色。
- 正常另有暗色截图；Login和Admin浅/深色保留后台业务布局，不伪造为Emerald首页。
- 没有做截图像素差门禁，也没有宣称逐像素一致。并排PNG用于人工复核。
- Lucide统一图标路径、PlatPulse业务行、未知空白占位、长值换行、后台工作台布局是保留的明确差异。后台文字操作按钮仍使用共享Button原有44px边框高度；本轮没有全局压缩后台业务操作。
- 没有跑整个仓库所有e2e，也没有验证Firefox/Safari/真机触摸；未改后端，所以没有声称执行Rust工作区测试。
- 用户原有未跟踪文件 `increct-maps.jpg` 保持原样，未纳入提交。
