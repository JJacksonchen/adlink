# 插件化二次开发指南 (参考 DeepSeek Harness / Cordis 思路)

AdLink 的全部组件都是实现 `adlink_kernel::Plugin` 的可插拔模块。

## 新增自定义平台连接器或安全护栏插件示例

```rust
use std::sync::Arc;
use adlink_kernel::{
    HookDecision, HookHandler, HookPoint, KernelResult, Plugin, PluginCategory, PluginContext,
    PluginManifest,
};
use async_trait::async_trait;
use serde_json::{json, Value};

struct MaxDailySpendGuardrail;

#[async_trait]
impl HookHandler for MaxDailySpendGuardrail {
    fn id(&self) -> &str { "custom.weekend_guardrail" }
    fn priority(&self) -> i32 { 20 }

    async fn handle(&self, point: HookPoint, payload: Value) -> HookDecision {
        if point == HookPoint::BeforeAdMutate {
            // 自定义校验逻辑，支持 Continue / Modified(Value) / Reject { code, reason }
        }
        HookDecision::Continue
    }
}

pub struct WeekendGuardrailPlugin;

#[async_trait]
impl Plugin for WeekendGuardrailPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "guardrail.weekend".into(),
            name: "Weekend Spend Guardrail".into(),
            version: "0.1.0".into(),
            description: "Custom budget guardrail hook".into(),
            category: PluginCategory::Guardrail,
            provides: vec![],
            requires: vec!["storage".into()],
            config_schema: json!({"type": "object"}),
        }
    }

    async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
        ctx.register_hook(HookPoint::BeforeAdMutate, Arc::new(MaxDailySpendGuardrail));
        Ok(())
    }
}
```

当需要替换持久化后端（如从 `SqliteStoragePlugin` 切换为 PostgreSQL / ClickHouse），只需实现 `adlink_storage::StorageService` 并在插件 `mount` 时注册 `ctx.provide_service::<dyn StorageService>(...)` 即可，所有连接器、同步引擎与 MCP 工具无需修改一行代码。
