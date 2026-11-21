//! Core microkernel runtime orchestrating plugin mount, dependency validation,
//! hot-reload, hook execution, and scoped teardown.

pub mod context;
pub mod error;
pub mod events;
pub mod hooks;
pub mod plugin;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicU64;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::RwLock;

pub use context::{EffectScope, PluginContext};
pub use error::{KernelError, KernelResult};
pub use events::{EventBus, KernelEvent, ScopedSubscription};
pub use hooks::{HookDecision, HookHandler, HookPoint, HookRegistry};
pub use plugin::{KernelHealthReport, Plugin, PluginCategory, PluginHealth, PluginManifest, PluginStatus};

use context::ServiceRegistry;

struct MountedPlugin {
    plugin: Arc<dyn Plugin>,
    manifest: PluginManifest,
    mounted_at: DateTime<Utc>,
    scope: EffectScope,
}

/// DeepSeek-Harness / Cordis-inspired plugin microkernel.
///
/// There is no hardwired business logic inside `Kernel` itself: connectors, storage engines,
/// metric calibers, attribution calculators, guardrails, MCP tool packs, and transports all
/// register into this kernel through [`Plugin::mount`] and unwind atomically via [`EffectScope`].
pub struct Kernel {
    plugins: RwLock<HashMap<String, MountedPlugin>>,
    mount_order: RwLock<Vec<String>>,
    services: Arc<ServiceRegistry>,
    hooks: Arc<HookRegistry>,
    events: EventBus,
    next_id: Arc<AtomicU64>,
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel {
    /// Constructs a fresh empty microkernel instance.
    pub fn new() -> Self {
        Self {
            plugins: RwLock::new(HashMap::new()),
            mount_order: RwLock::new(Vec::new()),
            services: Arc::new(ServiceRegistry::new()),
            hooks: Arc::new(HookRegistry::new()),
            events: EventBus::default(),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    /// Mounts `plugin` with JSON `config`, validating dependencies first and rolling back all
    /// partial effects if `mount` returns an error.
    pub async fn mount(&self, plugin: Arc<dyn Plugin>, config: Value) -> KernelResult<()> {
        let manifest = plugin.manifest();
        let plugin_id = manifest.id.clone();

        {
            let guard = self.plugins.read().await;
            if guard.contains_key(&plugin_id) {
                return Err(KernelError::AlreadyMounted(plugin_id));
            }
            self.verify_requirements(&manifest, &guard)?;
        }

        let mut ctx = PluginContext::new(
            plugin_id.clone(),
            config,
            Arc::clone(&self.services),
            Arc::clone(&self.hooks),
            self.events.clone(),
            Arc::clone(&self.next_id),
        );

        if let Err(err) = plugin.mount(&mut ctx).await {
            // Rollback any services/hooks/subscriptions registered prior to failure.
            let mut partial_scope = ctx.into_scope();
            partial_scope.dispose_all();
            return Err(match err {
                KernelError::InvalidConfig { .. } | KernelError::MissingDependency { .. } => err,
                other => KernelError::MountFailed {
                    plugin: plugin_id,
                    message: other.to_string(),
                },
            });
        }

        let scope = ctx.into_scope();
        let effect_count = scope.len();
        {
            let mut guard = self.plugins.write().await;
            if guard.contains_key(&plugin_id) {
                let mut dup_scope = scope;
                dup_scope.dispose_all();
                return Err(KernelError::AlreadyMounted(plugin_id));
            }
            guard.insert(
                plugin_id.clone(),
                MountedPlugin {
                    plugin,
                    manifest: manifest.clone(),
                    mounted_at: Utc::now(),
                    scope,
                },
            );
        }
        self.mount_order.write().await.push(plugin_id.clone());

        tracing::info!(
            plugin_id = %plugin_id,
            category = ?manifest.category,
            effects = effect_count,
            "mounted kernel plugin"
        );
        self.events.emit(KernelEvent::new(
            "plugin.mounted",
            &plugin_id,
            serde_json::json!({ "name": manifest.name, "version": manifest.version }),
        ));
        Ok(())
    }

    fn verify_requirements(
        &self,
        manifest: &PluginManifest,
        mounted: &HashMap<String, MountedPlugin>,
    ) -> KernelResult<()> {
        let mut provided_tokens: HashSet<&str> = HashSet::new();
        for (id, entry) in mounted {
            provided_tokens.insert(id.as_str());
            for token in &entry.manifest.provides {
                provided_tokens.insert(token.as_str());
            }
        }
        for req in &manifest.requires {
            if !provided_tokens.contains(req.as_str()) {
                return Err(KernelError::MissingDependency {
                    plugin: manifest.id.clone(),
                    dependency: req.clone(),
                });
            }
        }
        Ok(())
    }

    /// Unloads a mounted plugin after verifying no active plugin requires it.
    pub async fn unload(&self, plugin_id: &str) -> KernelResult<()> {
        let mut entry = {
            let mut guard = self.plugins.write().await;
            let target = guard
                .get(plugin_id)
                .ok_or_else(|| KernelError::PluginNotFound(plugin_id.to_string()))?;
            let target_tokens: HashSet<&str> = std::iter::once(plugin_id)
                .chain(target.manifest.provides.iter().map(String::as_str))
                .collect();

            let dependents: Vec<String> = guard
                .iter()
                .filter(|(other_id, _)| other_id.as_str() != plugin_id)
                .filter(|(_, other)| {
                    other
                        .manifest
                        .requires
                        .iter()
                        .any(|req| target_tokens.contains(req.as_str()))
                })
                .map(|(other_id, _)| other_id.clone())
                .collect();

            if !dependents.is_empty() {
                return Err(KernelError::HasActiveDependents {
                    plugin: plugin_id.to_string(),
                    dependents,
                });
            }

            guard
                .remove(plugin_id)
                .ok_or_else(|| KernelError::PluginNotFound(plugin_id.to_string()))?
        };

        let unmount_res = entry.plugin.unmount().await;
        // Always execute reversible scope cleanup even if unmount errored.
        entry.scope.dispose_all();
        self.mount_order
            .write()
            .await
            .retain(|id| id != plugin_id);

        if let Err(err) = unmount_res {
            return Err(KernelError::UnmountFailed {
                plugin: plugin_id.to_string(),
                message: err.to_string(),
            });
        }

        tracing::info!(plugin_id = %plugin_id, "unloaded kernel plugin");
        self.events.emit(KernelEvent::new(
            "plugin.unloaded",
            plugin_id,
            Value::Null,
        ));
        Ok(())
    }

    /// Hot-reloads an existing plugin with new configuration while preserving its Arc instance.
    pub async fn reload(&self, plugin_id: &str, new_config: Value) -> KernelResult<()> {
        let plugin = {
            let guard = self.plugins.read().await;
            guard
                .get(plugin_id)
                .map(|m| Arc::clone(&m.plugin))
                .ok_or_else(|| KernelError::PluginNotFound(plugin_id.to_string()))?
        };
        self.unload(plugin_id).await?;
        self.mount(plugin, new_config).await
    }

    /// Unloads all mounted plugins in reverse mount order for clean shutdown.
    pub async fn shutdown(&self) {
        let order: Vec<String> = {
            let guard = self.mount_order.read().await;
            guard.iter().rev().cloned().collect()
        };
        for id in order {
            if let Err(err) = self.unload(&id).await {
                tracing::warn!(plugin_id = %id, error = %err, "plugin unload during shutdown");
            }
        }
    }

    /// Resolves a singleton service `T` from the kernel registry.
    pub fn get_service<T: Any + Send + Sync + ?Sized>(&self) -> Option<Arc<T>> {
        self.services.get::<T>()
    }

    /// Resolves a required singleton service `T`, returning a structured error if missing.
    pub fn require_service<T: Any + Send + Sync + ?Sized>(&self) -> KernelResult<Arc<T>> {
        self.get_service::<T>()
            .ok_or_else(|| KernelError::ServiceNotFound {
                service: std::any::type_name::<T>().to_string(),
            })
    }

    /// Resolves a named service implementation `T`.
    pub fn get_named_service<T: Any + Send + Sync + ?Sized>(&self, name: &str) -> Option<Arc<T>> {
        self.services.get_named::<T>(Some(name))
    }

    /// Lists all named/unnamed services implementing `T`.
    pub fn list_services_of<T: Any + Send + Sync + ?Sized>(&self) -> Vec<(Option<String>, Arc<T>)> {
        self.services.list_of::<T>()
    }

    /// Lists registered services (`(plugin_id, type_name, optional_name)`).
    pub fn describe_services(&self) -> Vec<(String, &'static str, Option<String>)> {
        self.services.describe()
    }

    /// Returns true when at least one hook handler is registered at `point`.
    pub fn has_hooks(&self, point: HookPoint) -> bool {
        self.hooks.has_hooks(point)
    }

    /// Executes all hook handlers registered at `point`.
    pub async fn run_hooks(&self, point: HookPoint, payload: Value) -> KernelResult<Value> {
        self.hooks.run(point, payload).await
    }

    /// Lists registered hook points & handlers.
    pub fn list_hooks(&self) -> Vec<(HookPoint, String, String, i32)> {
        self.hooks.list()
    }

    /// Emits a kernel event on the shared event bus.
    pub fn emit(&self, event: KernelEvent) {
        self.events.emit(event);
    }

    /// Subscribes to kernel events outside a plugin lifecycle (caller manages lifetime).
    pub fn subscribe_events(&self, topic_prefix: Option<&str>) -> ScopedSubscription {
        let (sub, _) = self
            .events
            .subscribe_scoped(topic_prefix.map(str::to_string));
        sub
    }

    /// Lists currently mounted plugins along with live health evaluation.
    pub async fn list_plugins(&self) -> Vec<PluginStatus> {
        let snapshots: Vec<(Arc<dyn Plugin>, PluginManifest, DateTime<Utc>, usize)> = {
            let guard = self.plugins.read().await;
            let order = self.mount_order.read().await;
            order
                .iter()
                .filter_map(|id| guard.get(id))
                .map(|m| {
                    (
                        Arc::clone(&m.plugin),
                        m.manifest.clone(),
                        m.mounted_at,
                        m.scope.len(),
                    )
                })
                .collect()
        };

        let mut statuses = Vec::with_capacity(snapshots.len());
        for (plugin, manifest, mounted_at, effect_count) in snapshots {
            let health = plugin.health().await;
            statuses.push(PluginStatus {
                manifest,
                mounted_at,
                effect_count,
                health,
            });
        }
        statuses
    }

    /// Produces an aggregated kernel health report across all mounted plugins.
    pub async fn health_report(&self) -> KernelHealthReport {
        let plugins = self.list_plugins().await;
        let healthy = plugins.iter().all(|p| p.health.healthy);
        KernelHealthReport {
            healthy,
            plugin_count: plugins.len(),
            plugins,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use async_trait::async_trait;

    use super::*;

    trait Greeter: Send + Sync {
        fn greet(&self) -> String;
    }

    struct GreeterImpl(String);
    impl Greeter for GreeterImpl {
        fn greet(&self) -> String {
            self.0.clone()
        }
    }

    struct ProviderPlugin {
        id: String,
        requires: Vec<String>,
        fail_mount: bool,
        unmounted: Arc<AtomicBool>,
    }

    #[async_trait]
    impl Plugin for ProviderPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: self.id.clone(),
                name: format!("Test {}", self.id),
                version: "0.1.0".into(),
                description: "test".into(),
                category: PluginCategory::Custom("test".into()),
                provides: vec!["greeter".into()],
                requires: self.requires.clone(),
                config_schema: Value::Null,
            }
        }

        async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
            let greeting = ctx
                .config()
                .get("greeting")
                .and_then(Value::as_str)
                .unwrap_or("hello")
                .to_string();
            let svc: Arc<dyn Greeter> = Arc::new(GreeterImpl(greeting));
            ctx.provide_service(svc);
            if self.fail_mount {
                return Err(KernelError::InvalidConfig {
                    plugin: self.id.clone(),
                    message: "forced failure".into(),
                });
            }
            Ok(())
        }

        async fn unmount(&self) -> KernelResult<()> {
            self.unmounted.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    struct GuardrailHook {
        id: String,
        priority: i32,
        reject_above: Option<i64>,
    }

    #[async_trait]
    impl HookHandler for GuardrailHook {
        fn id(&self) -> &str {
            &self.id
        }
        fn priority(&self) -> i32 {
            self.priority
        }
        async fn handle(&self, _point: HookPoint, mut payload: Value) -> HookDecision {
            if let Some(limit) = self.reject_above {
                let amt = payload.get("amount").and_then(Value::as_i64).unwrap_or(0);
                if amt > limit {
                    return HookDecision::Reject {
                        code: "BUDGET_EXCEEDED".into(),
                        reason: format!("amount {amt} > limit {limit}"),
                    };
                }
            }
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("checked_by".into(), Value::String(self.id.clone()));
            }
            HookDecision::Modified(payload)
        }
    }

    struct HookPlugin {
        id: String,
        requires: Vec<String>,
        reject_above: Option<i64>,
    }

    #[async_trait]
    impl Plugin for HookPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: self.id.clone(),
                name: self.id.clone(),
                version: "0.1.0".into(),
                description: "hook".into(),
                category: PluginCategory::Guardrail,
                provides: vec![],
                requires: self.requires.clone(),
                config_schema: Value::Null,
            }
        }

        async fn mount(&self, ctx: &mut PluginContext) -> KernelResult<()> {
            ctx.register_hook(
                HookPoint::BeforeAdMutate,
                Arc::new(GuardrailHook {
                    id: format!("{}-hook", self.id),
                    priority: 10,
                    reject_above: self.reject_above,
                }),
            );
            Ok(())
        }
    }

    #[tokio::test]
    async fn mount_requires_dependency_and_unloads_cleanly() {
        let kernel = Kernel::new();
        let unmounted = Arc::new(AtomicBool::new(false));

        let consumer = Arc::new(HookPlugin {
            id: "consumer".into(),
            requires: vec!["greeter".into()],
            reject_above: Some(100),
        });
        let err = kernel.mount(consumer.clone(), Value::Null).await.unwrap_err();
        assert!(matches!(err, KernelError::MissingDependency { .. }));

        let provider = Arc::new(ProviderPlugin {
            id: "provider".into(),
            requires: vec![],
            fail_mount: false,
            unmounted: Arc::clone(&unmounted),
        });
        kernel
            .mount(provider, serde_json::json!({"greeting": "nihao"}))
            .await
            .unwrap();
        assert_eq!(kernel.get_service::<dyn Greeter>().unwrap().greet(), "nihao");

        kernel.mount(consumer, Value::Null).await.unwrap();

        // Active dependent prevents premature provider unload.
        let dep_err = kernel.unload("provider").await.unwrap_err();
        assert!(matches!(dep_err, KernelError::HasActiveDependents { .. }));

        // Hook pipeline enforces guardrail rejection and mutation.
        let ok_res = kernel
            .run_hooks(HookPoint::BeforeAdMutate, serde_json::json!({"amount": 50}))
            .await
            .unwrap();
        assert_eq!(ok_res["checked_by"], "consumer-hook");

        let rej_err = kernel
            .run_hooks(HookPoint::BeforeAdMutate, serde_json::json!({"amount": 250}))
            .await
            .unwrap_err();
        assert!(matches!(rej_err, KernelError::HookRejected { .. }));

        // Unloading consumer removes its hook; unloading provider removes its service.
        kernel.unload("consumer").await.unwrap();
        assert!(kernel.list_hooks().is_empty());

        kernel.unload("provider").await.unwrap();
        assert!(unmounted.load(Ordering::SeqCst));
        assert!(kernel.get_service::<dyn Greeter>().is_none());
    }

    #[tokio::test]
    async fn failed_mount_rolls_back_partial_registrations() {
        let kernel = Kernel::new();
        let unmounted = Arc::new(AtomicBool::new(false));
        let failing = Arc::new(ProviderPlugin {
            id: "broken".into(),
            requires: vec![],
            fail_mount: true,
            unmounted,
        });
        let err = kernel.mount(failing, Value::Null).await.unwrap_err();
        assert!(matches!(err, KernelError::InvalidConfig { .. }));
        // Service registered before failure must have been rolled back.
        assert!(kernel.get_service::<dyn Greeter>().is_none());
        assert!(kernel.list_plugins().await.is_empty());
    }
}
