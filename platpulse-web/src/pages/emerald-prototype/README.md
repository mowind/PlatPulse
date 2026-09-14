# Emerald 免登录原型（THROWAWAY）

问题：哪一种 Emerald 信息层级最适合 PlatON Node 运行诊断？

## 已确认的选择

用户明确选择 **A · 连续阅读**（原话：“选择A”）。用户未补充选择理由，不推定额外偏好。

选定顺序：Node 名称、状态与最后报告 → 关键摘要 → Node / PlatON 进程与 Node Data / Host 共享资源分组 → 最近 60 秒六图 → 折叠 Peer 诊断 → 折叠标识与技术详情。

B、C 仅保留为探索对照，不作为正式实现方向。本次选择确定布局，不等于授权全站改造、创建 GitHub Issue 或改变鉴权。原型仍不是正式实现或新的设计规范。

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
- 顶部切换明暗主题、正常 / 报告过期 / CPU 采集失败 / 从未观测。主题和状态仅存内存。
- Node 概览是最小模拟导航入口，不是完整 Home 重设计；Admin、Login 与其他正式页面没有修改。
- 六图展示固定的模拟 60 秒窗口，不是真实实时数据，不接 API / SSE。保留 last-good，空窗口与未知不画曲线，CPU 失败不影响其他指标。
- 区分 Node、PlatON 进程与 Host 共享汇总，不包含敏感身份或 RPC 地址。

## 隔离与后续

独立 HTML 入口位于 platpulse-web/prototype/emerald/index.html，不导入正式 App 或 AuthProvider，不改鉴权规则。默认 npm run build 不打包原型入口；浮动切换条另有 DEV 门控。没有增加依赖。

原型与选择记录归档至独立分支 `prototype/emerald-a`，不合入 main，也不推送远端。正式实施与 GitHub 写操作待用户确认；获批后在实施 Issue 引用本分支与归档提交，正式分支只保留获认可的实现。

## 验证

- lint、typecheck、build 通过；正式包仍有大于 500 kB 的体积提示。
- 临时 Playwright 检查：360 / 390 / 768 / 1280 宽度 × 3 布局 × 2 主题 × 4 状态，共 96 组合，无整页水平溢出、脚本异常或 API 请求。
- 布局切换、刷新保留布局、诊断折叠通过。
- 未新增测试或运行正式全量测试套件（throwaway 原型范围）。
- 截图环境缺少中文系统字体，使用 /tmp/platpulse-prototype-fonts 中临时 Noto Sans CJK 与 Fontconfig；未修改项目字体依赖。普通浏览器使用自身中文字体回退。
