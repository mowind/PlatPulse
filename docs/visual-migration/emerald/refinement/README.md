# Emerald 差距修正：范围、证据和限制

## 范围

本轮从 `a1eb85e` 开始，仍在 `feat/webui-emerald-visual-migration`。
视觉对照是 `/tmp/emerald` 的固定 HEAD
`c2c5e88ea19c7cbe18d14a50414e10deca3cc66e`，不是最新版。
后端、API 客户端、DTO、权限、ECharts、地图几何资源、首页 37:61 布局均未修改。

## 修正对照

| 项目 | 修复前 | 修复后 |
|---|---|---|
| Tabs | Reka `data-active` / `group-data-horizontal/vertical` 未匹配 Radix | 使用 `data-state=active`、`data-orientation`；浅/深主题实际选中色、背景、焦点、方向键、Home/End 均有浏览器断言 |
| 卡片标题/正文 | 手写 link `p-3`，没有 CardX 标题区/正文区分隔 | 复用 medium CardX：标题 `12px 16px`，正文 `0 16px 16px`，标题 16px；正文分组 gap 12px，资源栅格列距12px、行距4px |
| 指标 | 普通信息行与资源行没有区别；shortLabel 永久 hidden | 普通信息行有淡点虚线连接；资源行只有进度；窄行显示 L/C，保留完整 aria-label/title；长数值换行，不缩小12px字号、不截断数值、不挤掉标签 |
| 未知 | 资源用破折号，缺少进度高度 | 明确 Unknown；缺测资源保留4px空白，不渲染空轨道/0%进度条；权威0不受影响 |
| 统计 | 使用设置、主题、太阳、月亮动作图标；四统计始终全局 | Server/HeartPulse/TriangleAlert/Network 业务 SVG；统计、地图、卡片同一筛选范围 |
| 控件 | Tabs外壳44px、触发器44px（导致26px声明无效）；排序着色区44px、页头图标按钮着色区42px（外框均44px） | 首页外壳32px、触发器26px；触发器真实44px伪元素点击范围；排序、页头图标按钮44px元素/32px着色区 |
| 地图 | 异常只在sr-only；保留国家数据时unknown未提示 | 正常无状态句；异常最小可见状态且绝对定位，不挤动canvas；未知状态也有提示；资源失败只一份提示 |
| 全站 | 字符导航/状态/排序/方向图标与不同SVG混用；Admin无共享背景 | 统一已安装的本地Lucide描边SVG，后台保留业务布局；Admin采用同一背景组件和透明表面，隔离背景堆叠，不改权限 |

CardX 的 medium 内边距、NodeCard 的 12px分组/列距、4px资源行距和淡点虚线均直接对照固定源码。
资源顺序、共识三列、网络名、诊断文案、业务统计、Peer地图语义属于 PlatPulse，不硬套 Emerald 的服务器数据。

## 测试方法

- 修改前先新增 Tabs selector 和多网络统计回归；实际为 **2 failed / 43 passed**，见 `logs/before-unit.log`。
- 浏览器新增测试不是只验证点击：读取 computed background/color，检查 focus-visible/ring、键盘自动激活、实际几何、elementFromPoint上下边界以及触摸点击可见框外的位置。
- 不删除断言。旧“全局统计”“异常必须sr-only”“后台无背景”“字符图标”断言按本轮明确需求改为对应的新行为；44px元素框断言改为44px真实命中范围，并逐个滚动到可视区域验证Tabs边界。
- 原“未知用破折号”的计数改为明确Unknown或逐个命名指标验证；资源未知仍断言没有progressbar，同时新增稳定占位断言。
- 截图是证据，不是 `toHaveScreenshot` 门禁；没有通过重录基线让测试变绿。

## 截图与场景

运行入口 `platpulse-web/e2e/emerald-refinement.spec.ts`。对每个固定视口记录
`environment.json`（提交、Chromium、Node、平台、视口，以及逐场景DOM几何）。

Home使用真实临时Server登录，拦截**仅Public networks读响应**建立确定性前端DTO场景：
Mainnet一节点、Testnet两节点；国家分别DE=4、FR=7；网络筛选应同步得到1或2节点。
CPU/Memory、数据目录、网络速率、uptime和共识数值固定。
正常、unknown、stale、error、disabled、partial、长名称、长数值（9007199254740991）、多网络分别独立导航。
正常另拍暗色；Login/Admin分别拍浅色和暗色。Admin使用真实临时Server种子，时间/新鲜度会随捕获时间变化，不作为逐像素基线。

等待字体和ECharts真实非透明像素后截图；unknown/disabled断言没有canvas，不强迫无数据状态画地图。
地图与旗帜资源仍来自同源，无新增CDN。

`before/` 使用 `git archive a1eb85e` 独立构建的生产前端；同一临时Server重启加载该bundle。
`after/` 使用本轮代码提交重新构建，所有after截图来自同一提交。证据随后单独提交，以免把未提交源码误记为提交截图。

## 验收边界 / 剩余差异

- 不声明像素级一致。这里验证的是明确的尺寸、颜色、布局、状态和交互契约；PNG仍供人工并排审阅。
- SVG统一为Lucide（现有本地依赖），不是Emerald原版混合IconPark/Tabler逐路径复刻；这是本轮“统一风格”的有意偏差。
- 极长精确整数允许换行，可能增加卡片高度；完整值优先于原版截断值，也没有进一步缩小字体。
- 未知进度保留空白而非原版0值条，属于业务语义要求。
- Admin保留侧栏、表格、诊断与页头业务信息，不改成Emerald纯服务器首页布局。
- 浏览器执行仅Chromium；没有声称已验证Safari/Firefox或完成全仓库全部e2e。
- 首轮完整矩阵暴露了既有地图尺寸断言与 `6b78964` 布局决定冲突；修正的是测试约束，不是地图或布局，详见交付记录。

- [逐场景并排对照（本地HTML）](comparison.html)
- [最终交付记录](DELIVERY.md)
- 示例：[390px 修复前](before/phone-390-touch/normal.png) / [修复后](after/phone-390-touch/normal.png)
- 示例：[1440px 长数值修复前](before/desktop-1440/long-value.png) / [修复后](after/desktop-1440/long-value.png)
