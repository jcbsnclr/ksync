//! Method registry and the typed handler trait.
//!
//! A method's input and output types live in its [`Method`] impl. The host
//! registers built-ins and plugins through the same path; only the host knows
//! the namespace that a local name is registered under.

use std::{collections::BTreeMap, marker::PhantomData};

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Serialize, de::DeserializeOwned};

use crate::proto::{
    ErrorCode, Id, MAX_FRAME, MethodName, Namespace, RequestId, WireError, validate_local_name,
};

/// Per-request handler context.
pub struct Context {
    /// Authenticated client identity.
    pub peer: Id,
    pub request_id: RequestId,
}

/// Typed method implementation. `NAME` is local only; the host prefixes the
/// namespace at registration.
#[async_trait]
pub trait Method: Send + Sync + 'static {
    const NAME: &'static str;

    type Input: Serialize + DeserializeOwned + Send + 'static;
    type Output: Serialize + DeserializeOwned + Send + 'static;

    async fn call(ctx: &mut Context, input: Self::Input) -> Result<Self::Output, MethodError>;
}

/// Handler failures with structured wire mappings. `anyhow` stays inside the
/// handler; it must not leak onto the wire.
#[derive(Debug)]
pub enum MethodError {
    InvalidInput(String),
    NotFound,
    Conflict,
    TooLarge,
    UnknownMethod,
    Internal(String),
}

impl MethodError {
    pub fn into_wire(self) -> WireError {
        match self {
            Self::InvalidInput(detail) => WireError::new(ErrorCode::InvalidRequest, detail),
            Self::NotFound => WireError::new(ErrorCode::NotFound, ""),
            Self::Conflict => WireError::new(ErrorCode::Conflict, ""),
            Self::TooLarge => WireError::new(ErrorCode::TooLarge, "method payload too large"),
            Self::UnknownMethod => WireError::new(ErrorCode::UnknownRequest, "unknown method"),
            Self::Internal(detail) => WireError::new(ErrorCode::Internal, detail),
        }
    }
}

/// Object-safe boundary. Bytes in, bytes out; the concrete input/output types
/// remain in the [`Method`] impl.
#[async_trait]
pub trait ErasedMethod: Send + Sync {
    async fn call_bytes(&self, ctx: &mut Context, payload: &[u8]) -> Result<Vec<u8>, MethodError>;
}

struct Handler<M>(PhantomData<M>);

#[async_trait]
impl<M: Method> ErasedMethod for Handler<M> {
    async fn call_bytes(&self, ctx: &mut Context, payload: &[u8]) -> Result<Vec<u8>, MethodError> {
        let (input, trailing) = postcard::take_from_bytes(payload)
            .map_err(|err| MethodError::InvalidInput(err.to_string()))?;
        if !trailing.is_empty() {
            return Err(MethodError::InvalidInput(
                "trailing bytes after method input".into(),
            ));
        }

        let output = M::call(ctx, input).await?;

        postcard::to_stdvec(&output).map_err(|err| MethodError::Internal(err.to_string()))
    }
}

pub struct MethodSpec {
    pub max_input: usize,
    pub max_output: usize,
    handler: Box<dyn ErasedMethod>,
}

/// A staged set of methods for one namespace. Plugins only see local names;
/// the host merges the list under the namespace it assigned.
#[derive(Default)]
pub struct MethodList {
    entries: Vec<Entry>,
}

struct Entry {
    local: &'static str,
    spec: MethodSpec,
}

impl MethodList {
    pub fn add<M: Method>(&mut self) -> Result<()> {
        self.add_with_limits::<M>(MAX_FRAME, MAX_FRAME)
    }

    pub fn add_with_limits<M: Method>(
        &mut self,
        max_input: usize,
        max_output: usize,
    ) -> Result<()> {
        validate_local_name(M::NAME)?;

        if self.entries.iter().any(|entry| entry.local == M::NAME) {
            bail!("duplicate local method name {:?}", M::NAME);
        }

        self.entries.push(Entry {
            local: M::NAME,
            spec: MethodSpec {
                max_input,
                max_output,
                handler: Box::new(Handler::<M>(PhantomData)),
            },
        });

        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Immutable method table. Registration is atomic per namespace.
#[derive(Default)]
pub struct Registry {
    methods: BTreeMap<MethodName, MethodSpec>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate and merge a method list under `namespace`.
    ///
    /// Nothing is inserted if any name is invalid or collides, so a failed
    /// plugin load leaves no partial namespace behind.
    pub fn register_all(&mut self, namespace: &Namespace, list: MethodList) -> Result<()> {
        let mut pending = Vec::with_capacity(list.entries.len());

        for entry in list.entries {
            let name = MethodName::new(namespace, entry.local)?;

            if self.methods.contains_key(&name)
                || pending.iter().any(|(existing, _)| existing == &name)
            {
                bail!("duplicate method {name}");
            }

            pending.push((name, entry.spec));
        }

        for (name, spec) in pending {
            self.methods.insert(name, spec);
        }

        Ok(())
    }

    /// Register a plugin's methods under a non-reserved namespace.
    ///
    /// The host assigns the namespace; plugins never choose it themselves.
    /// Reserved namespaces are rejected so a plugin cannot shadow core or a
    /// future built-in namespace.
    pub fn register_plugin(&mut self, namespace: &Namespace, list: MethodList) -> Result<()> {
        if namespace.is_reserved() {
            bail!("namespace {namespace:?} is reserved");
        }
        self.register_all(namespace, list)
    }

    pub async fn call(
        &self,
        ctx: &mut Context,
        method: &MethodName,
        payload: &[u8],
    ) -> Result<Vec<u8>, MethodError> {
        let spec = self.methods.get(method).ok_or(MethodError::UnknownMethod)?;

        if payload.len() > spec.max_input {
            return Err(MethodError::TooLarge);
        }

        let output = spec.handler.call_bytes(ctx, payload).await?;

        if output.len() > spec.max_output {
            return Err(MethodError::Internal(
                "method output exceeds its limit".into(),
            ));
        }

        Ok(output)
    }

    pub fn names(&self) -> impl Iterator<Item = &MethodName> {
        self.methods.keys()
    }

    pub fn contains(&self, method: &MethodName) -> bool {
        self.methods.contains_key(method)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{methods::Ping, proto::Namespace};

    #[derive(Debug)]
    struct Echo;

    #[async_trait]
    impl Method for Echo {
        const NAME: &'static str = "echo";

        type Input = Vec<u8>;
        type Output = Vec<u8>;

        async fn call(_ctx: &mut Context, input: Self::Input) -> Result<Self::Output, MethodError> {
            Ok(input)
        }
    }

    fn ctx() -> Context {
        Context {
            peer: Id([1; 32]),
            request_id: RequestId(1),
        }
    }

    fn core_registry() -> Registry {
        let mut registry = Registry::new();
        let mut list = MethodList::default();
        list.add::<Ping>().unwrap();
        list.add::<Echo>().unwrap();
        registry.register_all(&Namespace::core(), list).unwrap();
        registry
    }

    #[tokio::test]
    async fn dispatches_a_registered_method() {
        let registry = core_registry();
        let output = registry
            .call(&mut ctx(), &MethodName::core("ping").unwrap(), &[])
            .await
            .unwrap();
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn rejects_unknown_methods() {
        let registry = core_registry();
        let err = registry
            .call(&mut ctx(), &MethodName::core("nope").unwrap(), &[])
            .await
            .unwrap_err();
        assert!(matches!(err, MethodError::UnknownMethod));
    }

    #[tokio::test]
    async fn rejects_oversized_input_before_decoding() {
        let mut registry = Registry::new();
        let mut list = MethodList::default();
        list.add_with_limits::<Echo>(4, 64).unwrap();
        registry.register_all(&Namespace::core(), list).unwrap();

        let err = registry
            .call(&mut ctx(), &MethodName::core("echo").unwrap(), b"too long")
            .await
            .unwrap_err();
        assert!(matches!(err, MethodError::TooLarge));
    }

    #[test]
    fn registration_is_atomic_on_collision() {
        let mut registry = Registry::new();
        let mut list = MethodList::default();
        list.add::<Ping>().unwrap();
        registry.register_all(&Namespace::core(), list).unwrap();

        let mut duplicate = MethodList::default();
        duplicate.add::<Ping>().unwrap();
        assert!(
            registry
                .register_all(&Namespace::core(), duplicate)
                .is_err()
        );

        assert!(registry.contains(&MethodName::core("ping").unwrap()));
    }

    #[test]
    fn rejects_invalid_local_names() {
        #[derive(Debug)]
        struct Bad;

        #[async_trait]
        impl Method for Bad {
            const NAME: &'static str = "Bad:Name";

            type Input = ();
            type Output = ();

            async fn call(
                _ctx: &mut Context,
                _input: Self::Input,
            ) -> Result<Self::Output, MethodError> {
                Ok(())
            }
        }

        let mut list = MethodList::default();
        assert!(list.add::<Bad>().is_err());
    }

    #[test]
    fn rejects_duplicate_local_names() {
        let mut list = MethodList::default();
        list.add::<Ping>().unwrap();
        assert!(list.add::<Ping>().is_err());
    }

    #[tokio::test]
    async fn a_plugin_namespace_prefixes_the_local_name() {
        let mut registry = Registry::new();
        let mut list = MethodList::default();
        list.add::<Ping>().unwrap();

        let plugin = Namespace::parse("foobar").unwrap();
        registry.register_plugin(&plugin, list).unwrap();

        let name = MethodName::parse("foobar:ping").unwrap();
        registry.call(&mut ctx(), &name, &[]).await.unwrap();
        assert!(
            registry
                .call(&mut ctx(), &MethodName::core("ping").unwrap(), &[])
                .await
                .is_err()
        );
    }

    #[test]
    fn plugins_cannot_use_reserved_namespaces() {
        for reserved in ["core", "sys", "admin"] {
            let mut registry = Registry::new();
            let mut list = MethodList::default();
            list.add::<Ping>().unwrap();

            let namespace = Namespace::parse(reserved).unwrap();
            assert!(
                registry.register_plugin(&namespace, list).is_err(),
                "{reserved} must be reserved"
            );
        }
    }
}
