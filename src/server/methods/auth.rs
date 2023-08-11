use crate::proto::Method;
use crate::files::Files;
use crate::server::Context;

pub struct Identify;

impl Method for Identify {
    type Input<'a> = ();
    type Output = ();

    const NAME: &'static str = "IDENTIFY";

    fn call<'a>(_: &Files, ctx: &mut Context, _: Self::Input<'a>) -> anyhow::Result<Self::Output> {
        log::info!("client {} identified with server", ctx.addr());
        ctx.deregister(&Identify);

        log::info!("registering filesystem methods for client {}", ctx.addr());
        super::fs::register(ctx);

        Ok(())
    }
}