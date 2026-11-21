//! Scoped plugin context, type-safe service registry, and reversible effect cleanup.

use std::any::{Any, TypeId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use serde_json::Value;

use crate::error::{KernelError, KernelResult};
use crate::events::{EventBus, KernelEvent, ScopedSubscription};
use crate::hooks::{HookHandler, HookPoint, HookRegistry};

type Disposer = Box<dyn FnOnce() + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ServiceKey {
    type_id: TypeId,
    name: Option<String>,
}

struct RegisteredService {
    plugin_id: String,
    type_name: &'static str,
    name: Option<String>,
    instance: Arc<dyn Any + Send + Sync>,
}

/// Type-safe dependency injection container holding singleton and named trait objects.
#[derive(Default)]
pub struct ServiceRegistry {
    services: DashMap<ServiceKey, RegisteredService>,
}

impl ServiceRegistry {
    /// Creates an empty service registry.
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn insert<T: Any + Send + Sync + ?Sized>(
        &self,
        plugin_id: &str,
        name: Option<String>,
        service: Arc<T>,
    ) {
        let key = ServiceKey {
            type_id: TypeId::of::<T>(),
            name: name.clone(),
        };
        let boxed: Arc<dyn Any + Send + Sync> = Arc::new(service);
        self.services.insert(
            key,
            RegisteredService {
                plugin_id: plugin_id.to_string(),
                type_name: std::any::type_name::<T>(),
                name,
                instance: boxed,
            },
        );
    }

    pub(crate) fn remove(&self, type_id: TypeId, name: Option<String>, expected_plugin: &str) {
        let key = ServiceKey { type_id, name };
        // Only remove if still owned by the plugin being unloaded (avoids clobbering overrides).
        self.services
            .remove_if(&key, |_, entry| entry.plugin_id == expected_plugin);
    }

    /// Resolves an unnamed singleton service `T`.
    pub fn get<T: Any + Send + Sync + ?Sized>(&self) -> Option<Arc<T>> {
        self.get_named::<T>(None)
    }

    /// Resolves a named service `T` (e.g. a platform connector keyed by `"tencent_ads"`).
    pub fn get_named<T: Any + Send + Sync + ?Sized>(&self, name: Option<&str>) -> Option<Arc<T>> {
        let key = ServiceKey {
            type_id: TypeId::of::<T>(),
            name: name.map(str::to_string),
        };
        let entry = self.services.get(&key)?;
        entry.instance.downcast_ref::<Arc<T>>().map(Arc::clone)
    }

    /// Returns all `(optional_name, Arc<T>)` implementations registered for interface `T`.
    pub fn list_of<T: Any + Send + Sync + ?Sized>(&self) -> Vec<(Option<String>, Arc<T>)> {
        let target = TypeId::of::<T>();
        let mut results = Vec::new();
        for entry in self.services.iter() {
            if entry.key().type_id == target {
                if let Some(svc) = entry.value().instance.downcast_ref::<Arc<T>>() {
                    results.push((entry.value().name.clone(), Arc::clone(svc)));
                }
            }
        }
        results.sort_by(|a, b| a.0.cmp(&b.0));
        results
    }

    /// Returns `(plugin_id, type_name, optional_name)` metadata for diagnostics.
    pub fn describe(&self) -> Vec<(String, &'static str, Option<String>)> {
        self.services
            .iter()
            .map(|entry| {
                let v = entry.value();
                (v.plugin_id.clone(), v.type_name, v.name.clone())
            })
            .collect()
    }
}

/// Reversible effect stack bound to a single mounted plugin instance.
#[derive(Default)]
pub struct EffectScope {
    disposers: Vec<Disposer>,
}

impl EffectScope {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push<F>(&mut self, disposer: F)
    where
        F: FnOnce() + Send + Sync + 'static,
    {
        self.disposers.push(Box::new(disposer));
    }

    pub fn len(&self) -> usize {
        self.disposers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.disposers.is_empty()
    }

    /// Executes disposers in LIFO order so later registrations unwind before dependencies.
    pub fn dispose_all(&mut self) {
        while let Some(disposer) = self.disposers.pop() {
            disposer();
        }
    }
}

/// Scoped context passed to [`crate::Plugin::mount`], mirroring Cordis `Context`.
///
/// Plugins interact with the rest of the system exclusively through this context:
/// reading configuration, resolving required services, registering provided services/hooks/events,
/// and registering cleanup callbacks.
pub struct PluginContext {
    plugin_id: String,
    config: Value,
    services: Arc<ServiceRegistry>,
    hooks: Arc<HookRegistry>,
    events: EventBus,
    next_id: Arc<AtomicU64>,
    scope: EffectScope,
}

impl PluginContext {
    pub(crate) fn new(
        plugin_id: String,
        config: Value,
        services: Arc<ServiceRegistry>,
        hooks: Arc<HookRegistry>,
        events: EventBus,
        next_id: Arc<AtomicU64>,
    ) -> Self {
        Self {
            plugin_id,
            config,
            services,
            hooks,
            events,
            next_id,
            scope: EffectScope::new(),
        }
    }

    /// Returns the owning plugin's ID.
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Returns the raw JSON configuration supplied for this plugin.
    pub fn config(&self) -> &Value {
        &self.config
    }

    /// Deserializes this plugin's configuration into a strongly-typed struct.
    pub fn config_as<C: serde::de::DeserializeOwned>(&self) -> KernelResult<C> {
        serde_json::from_value(self.config.clone()).map_err(|err| KernelError::InvalidConfig {
            plugin: self.plugin_id.clone(),
            message: err.to_string(),
        })
    }

    /// Resolves a singleton service previously provided by another mounted plugin.
    pub fn require_service<T: Any + Send + Sync + ?Sized>(&self) -> KernelResult<Arc<T>> {
        self.services
            .get::<T>()
            .ok_or_else(|| KernelError::ServiceNotFound {
                service: std::any::type_name::<T>().to_string(),
            })
    }

    /// Resolves an optional singleton service without failing if absent.
    pub fn get_service<T: Any + Send + Sync + ?Sized>(&self) -> Option<Arc<T>> {
        self.services.get::<T>()
    }

    /// Resolves a named service instance.
    pub fn get_named_service<T: Any + Send + Sync + ?Sized>(&self, name: &str) -> Option<Arc<T>> {
        self.services.get_named::<T>(Some(name))
    }

    /// Provides an unnamed singleton service, bound to this plugin's [`EffectScope`].
    pub fn provide_service<T: Any + Send + Sync + ?Sized>(&mut self, service: Arc<T>) {
        self.register_service_inner(None, service);
    }

    /// Provides a named service implementation, bound to this plugin's [`EffectScope`].
    pub fn provide_named_service<T: Any + Send + Sync + ?Sized>(
        &mut self,
        name: impl Into<String>,
        service: Arc<T>,
    ) {
        self.register_service_inner(Some(name.into()), service);
    }

    fn register_service_inner<T: Any + Send + Sync + ?Sized>(
        &mut self,
        name: Option<String>,
        service: Arc<T>,
    ) {
        let plugin_id = self.plugin_id.clone();
        self.services.insert(&plugin_id, name.clone(), service);

        let registry = Arc::clone(&self.services);
        let type_id = TypeId::of::<T>();
        self.scope.push(move || {
            registry.remove(type_id, name, &plugin_id);
        });
    }

    /// Registers a hook handler at `point`, automatically removed on plugin unload/rollback.
    pub fn register_hook(&mut self, point: HookPoint, handler: Arc<dyn HookHandler>) {
        let reg_id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.hooks.insert(&self.plugin_id, reg_id, point, handler);

        let hooks = Arc::clone(&self.hooks);
        self.scope.push(move || {
            hooks.remove(point, reg_id);
        });
    }

    /// Subscribes to kernel events matching an optional topic prefix.
    /// The subscription stops receiving immediately when the plugin unmounts.
    pub fn subscribe_event(&mut self, topic_prefix: Option<&str>) -> ScopedSubscription {
        let (sub, active) = self
            .events
            .subscribe_scoped(topic_prefix.map(str::to_string));
        self.scope.push(move || {
            active.store(false, Ordering::SeqCst);
        });
        sub
    }

    /// Emits an event stamped with this plugin's ID.
    pub fn emit_event(&self, topic: impl Into<String>, payload: Value) {
        self.events
            .emit(KernelEvent::new(topic, &self.plugin_id, payload));
    }

    /// Registers an arbitrary synchronous disposer to run when this plugin unloads.
    pub fn on_dispose<F>(&mut self, disposer: F)
    where
        F: FnOnce() + Send + Sync + 'static,
    {
        self.scope.push(disposer);
    }

    pub(crate) fn into_scope(self) -> EffectScope {
        self.scope
    }
}
