//! Built-in methods.

use anyhow::Result;
use async_trait::async_trait;

use crate::{
    method::{Context, Method, MethodError, MethodList, Registry},
    proto::Namespace,
};

/// Liveness check.
pub struct Ping;

#[async_trait]
impl Method for Ping {
    const NAME: &'static str = "ping";

    type Input = ();
    type Output = ();

    async fn call(
        _ctx: &mut Context,
        (): Self::Input,
    ) -> std::result::Result<Self::Output, MethodError> {
        Ok(())
    }
}

/// The core method list, before it is attached to the `core` namespace.
pub fn core_methods() -> Result<MethodList> {
    let mut list = MethodList::default();
    list.add::<Ping>()?;
    Ok(list)
}

/// A registry containing only built-in methods.
pub fn core_registry() -> Result<Registry> {
    let mut registry = Registry::new();
    registry.register_all(&Namespace::core(), core_methods()?)?;
    Ok(registry)
}
