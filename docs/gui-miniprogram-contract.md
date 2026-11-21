# 网页端与小程序多端自定义契约 (Headless + Schema-Driven UI)

AdLink 将数据底座与展示层彻底解耦，方便企业自由替换为自有 React / Vue / Next.js 管理后台或微信 / UniApp / Taro 小程序：

1. **Schema-Driven UI 契约 (`GET /api/v1/ui/schema`)**
   - 返回权威导航模块、支持的消耗/业绩口径选项、10 个连接器元信息与支持的 Agent 客户端清单，前端无需硬编码枚举。
2. **完整 REST API + OpenAPI 3.1 (`GET /api/v1/openapi.json`)**
   - 所有 MCP 工具能力均有 1:1 对应的 REST API（`/api/v1/analytics/reconcile`、`/api/v1/attribution/creatives`、`/api/v1/bindings`、`/api/v1/sync/reports`、`/api/v1/ad-ops/mutate`）。
3. **快速替换 Web GUI**
   - 直接运行：`adlink serve --web-dir /path/to/your-frontend-dist`
   - 或直接修改仓库中的 `web-starter/` 目录（零构建依赖，纯 HTML/CSS/ES6）。
4. **小程序接入 SDK**
   - 引入 `web-starter/miniprogram-sdk/adlink-mp.js`，支持 `wx.request` / `uni.request` 开箱调用。
