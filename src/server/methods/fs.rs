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

        let object = files.with_root("root", Revision::FromLatest(0), |node| {
            if let Some(&mut object) = node.traverse(path)?.and_then(|n| n.file()) {
                Ok(object)
            } else {
                let err: io::Error = io::ErrorKind::InvalidInput.into();
                Err(err.into())
            }
        })?;

        log::info!("got object {}; returning to {addr}", object.hex());

        let data = files.get(&object)?;

        Ok((&data[..]).to_owned())
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
        
        let (parent, _) = path.parent_child();

        let object = files.create_object(&data)?;

        files.with_root_mut("root", |node| {
            node.make_dir_recursive(parent)?;
            node.insert(path, object)?;
            Ok(())
        })?;
        log::info!("stored {path} (object {})", object.hex());

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

        files.with_root_mut("root", |node| {
            node.delete(path)?;

            Ok(())
        })?;

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

        // the node to roll back to
        let mut old_root = files.get_root("root", revision)?;
        // the current node to merge with the root
        let new_root = files.get_root("root", Revision::FromLatest(0))?;

        // merge nodes to mark any files that don't exist in the old node as deleted
        old_root.merge(new_root)?;

        files.set_root("root", old_root)?;

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

        files.with_root("root", revision, |node| {
            // find the node at the given path
            if let Some(node) = node.traverse(path)? {
                // node found; return
                Ok(node.clone())
            } else {
                // note not found; error
                let err: io::Error = io::ErrorKind::NotFound.into();
                Err(err.into())
            }
        })
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

        let history = files.get_root_history("root")?;

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