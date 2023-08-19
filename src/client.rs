use std::io;
use std::net::SocketAddr;

use tokio::net::TcpStream;

use crate::proto::{self, Method};

pub struct Client {
    peer: TcpStream,
}

impl Client {
    pub async fn connect(addr: SocketAddr) -> io::Result<Client> {
        let peer = TcpStream::connect(addr).await?;

        Ok(Client { peer })
    }

    pub async fn invoke<'a, M: Method>(
        &mut self,
        method: M,
        args: M::Input<'a>,
    ) -> anyhow::Result<M::Output> {
        proto::invoke(&mut self.peer, method, args).await
    }
}
