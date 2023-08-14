use crate::proto::Method;
use crate::files::{Files, crypto};
use crate::server::Context;

use std::io;

pub struct Identify;

impl Method for Identify {
    type Input<'a> = crypto::Key;
    type Output = ();

    const NAME: &'static str = "IDENTIFY";

    fn call<'a>(files: &Files, ctx: &mut Context, key: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        log::info!("client {} identified with server", ctx.addr());
        ctx.deregister(&Identify);

        if files.verify_client(&key)? {
            log::info!("registering filesystem methods for client {}", ctx.addr());
            super::fs::register(ctx);
            Ok(())
        } else {
            log::error!("client {} failed to identify; untrusted certificate", ctx.addr());
            
            let err: io::Error = io::ErrorKind::InvalidData.into();
            Err(err.into())
        }
    }
}