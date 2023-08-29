use crate::files::{crypto, Files};
use crate::proto::Method;
use crate::server::{methods::auth, Context};

pub struct Configure;

impl Method for Configure {
    type Input<'a> = [crypto::Key; 3];
    type Output = ();

    const NAME: &'static str = "CONFIGURE";

    fn call<'a>(
        files: &Files,
        ctx: &mut Context,
        [admin, server, client]: Self::Input<'a>,
    ) -> anyhow::Result<()> {
        log::info!("client {} configuring server", ctx.addr());
        files.set_admin_key(admin)?;
        files.set_server_key(server)?;
        files.trust_client(client)?;

        ctx.register(&auth::Identify);
        ctx.deregister(&Configure);

        Ok(())
    }
}
