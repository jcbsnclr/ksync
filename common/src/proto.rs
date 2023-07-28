use std::io;

use serde::Serialize;
use tokio::io::{AsyncWriteExt, AsyncReadExt};

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
    pub method: String,
    pub data: Vec<u8>
}

impl Packet {
    async fn write<W: AsyncWriteExt + Unpin>(&self, writer: &mut W) -> io::Result<()> {
        util::write_array(writer, *b"ksync\0\0\0").await?;
        util::write_data(writer, self.method.as_bytes()).await?;
        util::write_data(writer, &self.data).await?;

        writer.flush().await?;

        Ok(())
    }
}

pub async fn read_packet<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Option<Packet>> {
    let proto = util::read_array(reader).await;
    match proto {
        Ok(proto) => {
            if &proto != b"ksync\0\0\0" {
                return Err(io::Error::new(io::ErrorKind::InvalidData, Error::InvalidProtocol { proto: proto }))
            }

            let method = String::from_utf8(util::read_data(reader).await?)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            let data = util::read_data(reader).await?;

            Ok(Some(Packet {
                method, data
            }))
        },

        // eof
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => Ok(None),

        Err(e) => Err(e)
    }
}

pub async fn write_packet<W: AsyncWriteExt + Unpin, T: Serialize>(writer: &mut W, method: &str, data: T) -> anyhow::Result<()> {
    Packet {
        method: method.to_owned(),
        data: bincode::serialize(&data)?
    }.write(writer).await?;

    Ok(())
}