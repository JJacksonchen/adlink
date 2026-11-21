//! Error types for the AdLink microkernel plugin runtime.

use thiserror::Error;

/// Result alias used across `adlink-kernel`.
pub type KernelResult<T> = Result<T, KernelError>;

/// Errors returned by plugin lifecycle, dependency injection, hook, and event operations.
#[derive(Debug, Error)]
pub enum KernelError {
    #[error("plugin '{0}' is already mounted")]
    AlreadyMounted(String),

    #[error("plugin '{0}' is not mounted")]
    PluginNotFound(String),

    #[error("plugin '{plugin}' requires missing dependency '{dependency}'")]
    MissingDependency { plugin: String, dependency: String },

    #[error("cannot unload plugin '{plugin}' because active plugins depend on it: {dependents:?}")]
    HasActiveDependents {
        plugin: String,
        dependents: Vec<String>,
    },

    #[error("service '{service}' not found in kernel registry")]
    ServiceNotFound { service: String },

    #[error("plugin '{plugin}' mount failed: {message}")]
    MountFailed { plugin: String, message: String },

    #[error("plugin '{plugin}' unmount failed: {message}")]
    UnmountFailed { plugin: String, message: String },

    #[error("hook '{hook_id}' rejected operation at {point}: [{code}] {reason}")]
    HookRejected {
        hook_id: String,
        point: String,
        code: String,
        reason: String,
    },

    #[error("invalid plugin configuration for '{plugin}': {message}")]
    InvalidConfig { plugin: String, message: String },
}
