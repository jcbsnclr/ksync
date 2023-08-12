use std::io;

use crate::proto::Method;
use crate::files::{Files, Path, Revision, Node, RootHistory};
use crate::server::Context;

/// The [Get] method resolves a virtual filesystem [Path] to it's respective object, loads it, and sends it back to the client
pub struct Get;

impl Method for Get {
    type Input<'a> = Path<'a>;
    type Output = Vec<u8>;

    const NAME: &'static str = "GET";

    fn call<'a>(files: &Files, ctx: &mut Context, path: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        let addr = ctx.addr();

        log::info!("client {addr} retrieving file {path}");

        let data = files.get(path, Revision::FromLatest(0))?;

        if let Some(data) = data {
            let data = &data[..];

            Ok(data.to_owned())
        } else {
            let err: io::Error = io::ErrorKind::NotFound.into();
            Err(err.into())
        }
    }
}

/// The [Insert] methods creates an object for a given piece of data, and inserts it into the filesystem at a given path
pub struct Insert;

impl Method for Insert {
    type Input<'a> = (Path<'a>, Vec<u8>);
    type Output = ();

    const NAME: &'static str = "INSERT";

    fn call<'a>(files: &Files, ctx: &mut Context, (path, data): Self::Input<'a>) -> anyhow::Result<Self::Output> {
        let addr = ctx.addr();

        log::info!("client {addr} storing file {path}");
        
        files.insert(path, &data)?;

        Ok(())
    }
}

pub struct Delete;

impl Method for Delete {
    type Input<'a> = Path<'a>;
    type Output = ();

    const NAME: &'static str = "DELETE";

    fn call<'a>(files: &Files, ctx: &mut Context, path: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        let addr = ctx.addr();

        log::info!("client {addr} deleting file {path}");

        files.delete(path)?;

        Ok(())
    }
}

/// Clear the files database
pub struct Clear;

impl Method for Clear {
    type Input<'a> = ();
    type Output = ();

    const NAME: &'static str = "CLEAR";

    fn call<'a>(files: &Files, ctx: &mut Context, _: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        let addr = ctx.addr();

        log::info!("client {addr} clearing database");
        files.clear()?;

        Ok(())
    }
}

pub struct Rollback;

impl Method for Rollback {
    type Input<'a> = Revision;
    type Output = ();

    const NAME: &'static str = "ROLLBACK";

    fn call<'a>(files: &Files, ctx: &mut Context, revision: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        let addr = ctx.addr();

        log::info!("client {addr} rolling back filesystem to revision {:?}", revision);

        files.rollback(revision)?;

        Ok(())
    }
}

pub struct GetNode;

impl Method for GetNode {
    type Input<'a> = (Path<'a>, Revision);
    type Output = Node;

    const NAME: &'static str = "GET_NODE";

    fn call<'a>(files: &Files, ctx: &mut Context, (path, revision): Self::Input<'a>) -> anyhow::Result<Self::Output> {
        let addr = ctx.addr();

        log::info!("client {addr} requested node {} @ {:?}", path.as_str(), revision);

        if let Some(node) = files.get_node(path, revision)? {
            Ok(node)
        } else {
            let err: io::Error = io::ErrorKind::NotFound.into();
            Err(err.into())
        }
    }
}

pub struct GetHistory;

impl Method for GetHistory {
    type Input<'a> = ();
    type Output = RootHistory;

    const NAME: &'static str = "GET_HISTORY";

    fn call<'a>(files: &Files, ctx: &mut Context, _: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        let addr = ctx.addr();

        log::info!("client {addr} requested history for root 'root'");

        let history = files.get_history()?;

        Ok(history)
    }
}

pub fn register(ctx: &mut Context) {
    ctx.register(&Get);
    ctx.register(&Insert);
    ctx.register(&Delete);
    ctx.register(&Clear);
    ctx.register(&Rollback);
    ctx.register(&GetNode);
    ctx.register(&GetHistory);
}