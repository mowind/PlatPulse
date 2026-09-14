# Emerald 免登录原型（THROWAWAY）

问题：哪一种 Emerald 信息层级最适合 PlatON Node 运行诊断？

## 已确认的选择

用户明确选择 **A · 连续阅读**（原话：“选择A”）。用户未补充选择理由，不推定额外偏好。

选定顺序：Node 名称、状态与最后报告 → 关键摘要 → Node / PlatON 进程与 Node Data / Host 共享资源分组 → 最近 60 秒六图 → 折叠 Peer 诊断 → 折叠标识与技术详情。

B、C 仅保留为探索对照，不作为正式实现方向。本次选择确定布局，不等于授权全站改造、创建 GitHub Issue 或改变鉴权。原型仍不是正式实现或新的设计规范。

## A 双主题校准（2026-09-14）

用户确认仅继续 A 免登录原型，不启动正式改造、不创建 Issue。完成双主题校准、顶部 SVG 网格与详情卡片悬停修正后，用户回复“确认”：本轮原型视觉与交互已获确认，作为后续实施参考。此确认不扩大为正式全站改造或 GitHub 写操作授权；原型不是正式设计规范或生产验收。

参考固定为 [Emerald c2c5e88](https://github.com/Tokinx/komari-theme-emerald/tree/c2c5e88ea19c7cbe18d14a50414e10deca3cc66e)：main.css 的中性 zinc token、Background.vue 的顶部 emerald/lime 渐变及低对比网格、NodeCard.vue 的轻量交互。不是像素级复制：保留 A 排版；图表保持蓝/青区分；深色 canvas 使用 slate 叠底近似色 #141923；成功/警告小字按可读性调整。不增加自定义图片、视频或重阴影。

主题行为：默认自动，按钮循环自动 → 浅色 → 深色 → 自动；自动实时跟随系统，显式主题覆盖系统；独立 localStorage 键 `platpulse.emerald-prototype.themeMode` 只保存原型主题，不触碰正式主题键。非法值回自动，存储不可用时仍可切换。HTML head 同步预应用主题与底色，React 接续同步；页脚显示选择模式与实际主题。

A 布局、六图与模拟状态语义未变。B/C 布局保留为对照，共享原型主题也随之校准；校准前的原始三方案完整保存在归档提交 `40094aa`。

## 运行

在仓库根目录执行：

```bash
npm --prefix platpulse-web run prototype:emerald
```

打开 http://127.0.0.1:5174/prototype/emerald/?variant=A 。使用现有前端依赖；不需要 Server、账号或真实数据。默认仅监听本机，其他设备需要端口转发。

- A：连续阅读，最接近交接文档建议顺序。
- B：诊断侧栏，摘要在左，运行观测在右。
- C：共识高度带、紧凑信息行与图表，作为层级对照。
- 底部箭头或键盘左右键循环切换，variant URL 参数刷新保留；输入控件不拦截方向键。
- 顶部切换自动 / 浅色 / 深色主题、正常 / 报告过期 / CPU 采集失败 / 从未观测。仅主题偏好本地保存，模拟状态仍只存内存。
- Node 概览是最小模拟导航入口，不是完整 Home 重设计；Admin、Login 与其他正式页面没有修改。
- 六图展示固定的模拟 60 秒窗口，不是真实实时数据，不接 API / SSE。保留 last-good，空窗口与未知不画曲线，CPU 失败不影响其他指标。
- 区分 Node、PlatON 进程与 Host 共享汇总，不包含敏感身份或 RPC 地址。

## 隔离与后续

独立 HTML 入口位于 platpulse-web/prototype/emerald/index.html，不导入正式 App 或 AuthProvider，不改鉴权规则。默认 npm run build 不打包原型入口；浮动切换条另有 DEV 门控。没有增加依赖。

原型与选择记录归档至独立分支 `prototype/emerald-a`，不合入 main，也不推送远端。正式实施与 GitHub 写操作待用户确认；获批后在实施 Issue 引用本分支与归档提交，正式分支只保留获认可的实现。

## 网格与卡片反馈修正

用户指出顶部网格不正确、详情卡片无交互。复现发现原先用两层 CSS 渐变近似网格，且 hover 只覆盖概览入口。现改为 Emerald Background.vue 的 SVG：72×56 pattern、x=-12/y=4、整体 skewY(-18deg)、四块局部填色、1300×400 顶部区域及分层径向/深色纵向遮罩；固定背景跟随视口，与上游一致。

A 摘要和信息分组卡常态为 60% 表面，悬停变实色、增加淡绿色轮廓/光晕并上移 2px。图表和折叠诊断卡只增加轮廓/光晕，不移动绘图区。折叠卡支持 focus-within，触屏不启用 hover 抬升，reduced-motion 禁用位移。展示卡不添加虚假点击行为。

临时回归脚本 /tmp/platpulse-emerald-hover-check.mjs 已先复现失败再验证修复，覆盖四类卡片 × 双主题、SVG 几何、键盘焦点、减少动态效果与触屏。32 组合原型矩阵再次通过，lint/typecheck/diff 检查通过；此轮未重跑 build 或正式全量测试。网格及悬停截图：/tmp/platpulse-emerald-grid-hover-{light,dark}.png。

## 本次校准验证

- lint、typecheck、build 通过；正式构建未加入原型入口，原有 >500 kB 包体积警告仍在。
- 临时 Playwright 检查 A：360×800 / 390×844 / 768×1024 / 1280×800 × 浅/深 × 四种状态，共 32 组合，无整页横向溢出、脚本异常或 API 请求；每组保持六图，Unknown/Stale 窗口不画数据，CPU 失败保留 18.6%。
- 诊断折叠、三态循环、刷新记忆、系统实时切换、显式主题覆盖、非法值回退和禁用存储均通过。
- 阻断入口模块加载时，HTML 已有正确深色 class、color-scheme 与 canvas，root 尚未挂载，验证首屏预应用不依赖 React。
- 四张全页截图：/tmp/platpulse-emerald-a-{1280,390}-{light,dark}.png；临时检查脚本 /tmp/platpulse-emerald-calibration-check.mjs。不新增正式测试，不宣称正式全量验收。

## 初版归档验证（40094aa）

- lint、typecheck、build 通过；正式包仍有大于 500 kB 的体积提示。
- 临时 Playwright 检查：360 / 390 / 768 / 1280 宽度 × 3 布局 × 2 主题 × 4 状态，共 96 组合，无整页水平溢出、脚本异常或 API 请求。
- 布局切换、刷新保留布局、诊断折叠通过。
- 未新增测试或运行正式全量测试套件（throwaway 原型范围）。
- 截图环境缺少中文系统字体，使用 /tmp/platpulse-prototype-fonts 中临时 Noto Sans CJK 与 Fontconfig；未修改项目字体依赖。普通浏览器使用自身中文字体回退。
