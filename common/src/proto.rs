use std::io;

use tokio::io::{AsyncWriteExt, AsyncReadExt};
use serde::Serialize;

use crate::util;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("invalid protocol '{proto:?}'")]
    InvalidProtocol { proto: [u8; 8] },

    #[error("unknown method {method}")]
    UnknownMethod { method: String } 
}

#[derive(Debug)]
pub struct Packet {
    pub method: [u8; 8],
    pub data: Vec<u8>
}

impl Packet {
    pub async fn write<W: AsyncWriteExt + Unpin>(&self, writer: &mut W) -> io::Result<()> {
        util::write_array(writer, b"ksync\0\0\0".to_owned()).await?;
        util::write_array(writer, self.method).await?;
        util::write_data(writer, &self.data).await?;

        writer.flush().await?;

        Ok(())
    }
}

pub async fn read_packet<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Packet> {
    let proto = util::read_array(reader).await?;

    if &proto != b"ksync\0\0\0" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, Error::InvalidProtocol { proto: proto }))
    }

    let method = util::read_array(reader).await?;
    let data = util::read_data(reader).await?;

    Ok(Packet {
        method, data
    })
}