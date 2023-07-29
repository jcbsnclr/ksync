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

/// A [Packet] represents a single message sent down the wire, from a client or server
#[derive(Debug)]
pub struct Packet {
    /// The method to be invoked on the server
    pub method: String,
    /// `bincode`-encoded arguments to the method
    pub data: Vec<u8>
}

impl Packet {
    /// Writes an individual [Packet] to a given `writer` and flushes the stream
    async fn write<W: AsyncWriteExt + Unpin>(&self, writer: &mut W) -> io::Result<()> {
        util::write_array(writer, *b"ksync\0\0\0").await?;
        util::write_data(writer, self.method.as_bytes()).await?;
        util::write_data(writer, &self.data).await?;

        writer.flush().await?;

        Ok(())
    }
}

/// Reads a [Packet] from a given `reader`
pub async fn read_packet<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Option<Packet>> {
    // read protocol magic
    let proto = util::read_array(reader).await;

    match proto {
        Ok(proto) => {
            // check magic
            if &proto != b"ksync\0\0\0" {
                return Err(io::Error::new(io::ErrorKind::InvalidData, Error::InvalidProtocol { proto: proto }))
            }

            // read method string
            let method = String::from_utf8(util::read_data(reader).await?)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            // read data bytes
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

/// Writes an individual [Packet] to a given `writer` and flushes the stream. Automatically serializes `data` using `bincode`
pub async fn write_packet<W: AsyncWriteExt + Unpin, T: Serialize>(writer: &mut W, method: &str, data: T) -> anyhow::Result<()> {
    Packet {
        method: method.to_owned(),
        data: bincode::serialize(&data)?
    }.write(writer).await?;

    Ok(())
}