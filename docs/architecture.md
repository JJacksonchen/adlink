# AdLink 架构设计说明 (参考 DeepSeek Harness / Cordis 微内核范式)

AdLink 在 Rust 中实现了受 **DeepSeek Harness (`dsh` / Cordis)** 启发的微内核插件架构：**Everything is a Plugin**。

## 1. 微内核四大支柱 (`adlink-kernel`)

1. **无硬编码核心 (`Kernel`)**：内核本身不包含任何特定广告平台或数据库实现；所有能力均通过 `Plugin::mount(&self, ctx: &mut PluginContext)` 动态挂载。
2. **作用域依赖注入 (`PluginContext` + `ServiceRegistry`)**：
   - `ctx.provide_service::<dyn StorageService>(...)` 注册全局单例服务。
   - `ctx.provide_named_service::<dyn AdPlatformConnector>("tencent_ads", ...)` 注册按平台名路由的多实例能力。
   - 依赖声明通过 `PluginManifest.requires` 与 `provides` 拓扑校验，未满足依赖时禁止挂载，且存在依赖方时禁止卸载。
3. **可逆副作用栈 (`EffectScope`)**：
   - 插件在 `mount()` 中注册的所有服务、钩子（`HookPoint`）、事件订阅（`EventBus`）及自定义清理回调都会压入该插件专属的 LIFO `EffectScope`。
   - 一旦 `mount()` 中途失败或运行期调用 `kernel.unload(id)` / `kernel.reload(id, cfg)`，内核自动逐条逆序释放副作用，杜绝状态残留与内存泄漏。
4. **生命周期钩子管道 (`HookPoint` + `HookHandler`)**：
   - `BeforeApiRequest` / `AfterApiResponse` / `AfterNormalizeRow` / `BeforeAdMutate` / `AfterAdMutate` / `EnrichMetrics`。
   - 安全护栏插件 `AdSafetyGuardrailPlugin` 以高优先级挂载在 `BeforeAdMutate`，校验日预算封顶、单次批量数量与必填原因，异常时返回 `HookDecision::Reject` 拦截执行并自动记入不可变审计表 `audit_logs`。

## 2. Crate 分层无环依赖图

```
adlink-cli (统一二进制: serve / mcp-stdio / sync / onboard / doctor)
  ├── adlink-api (Axum HTTP REST + OpenAPI 3.1 + UI Schema + 静态Web托管)
  │     └── adlink-mcp (Stdio + Streamable HTTP /mcp + Legacy SSE /sse + Guardrail)
  │           ├── adlink-connectors (8大媒体10连接器 + 纯Rust RSA2 + Mock生成器)
  │           │     └── adlink-engine (切片器 + 分级令牌桶 + 重试退避 + 并发协调器)
  │           │           └── adlink-storage (SQLite WAL 事务批量写 + 归因查询)
  │           └──────────────────┴────────────────────┴──────────────────┘
  │                                  adlink-domain (Fen, 治理, 口径AST公式, 平台元数据)
  │                                  adlink-kernel (Cordis微内核, DI, EffectScope, Hooks, EventBus)
```
