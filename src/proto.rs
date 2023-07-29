use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt, AsyncRead};

use serde::{Serialize, Deserialize, de::DeserializeOwned};

use crate::files::Files;

/// Reads exactly `N` bytes from a given `reader`, and returns it as a static array
pub async fn read_array<const N: usize, R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<[u8; N]> {
    let mut buf = [0; N];

    reader.read_exact(&mut buf).await?;

    Ok(buf)
}

/// Reads a 64-bit unsigned integer from a given `reader`
pub async fn read_int<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<u64> {
    let bytes = read_array(reader).await?;

    Ok(u64::from_le_bytes(bytes))
}

/// Reads a list of bytes from a given `reader`, encoded as `length, bytes...`
pub async fn read_data<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let len = read_int(reader).await? as usize;

    let mut buf = vec![0; len];
    
    reader.read_exact(&mut buf).await?;

    Ok(buf)
}

/// Writes a static array of bytes to a given `writer`
pub async fn write_array<const N: usize, W: AsyncWriteExt + Unpin>(writer: &mut W, array: [u8; N]) -> io::Result<()> {
    match writer.write(&array).await? {
        n if n == N => Ok(()),
        n => Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("tried to write {N} bytes, actually wrote {n}")))
    }
}

/// Writes a 64-bit unsigned integer to a given `writer`
pub async fn write_int<W: AsyncWriteExt + Unpin>(writer: &mut W, val: u64) -> io::Result<()> {
    write_array(writer, val.to_le_bytes()).await
}

/// Writes a `length, bytes...` encoded list of bytes to a given `writer`
pub async fn write_data<W: AsyncWriteExt + Unpin>(writer: &mut W, data: &[u8]) -> io::Result<()> {
    write_int(writer, data.len() as u64).await?;
    writer.write(&data).await?;

    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("invalid protocol '{proto:?}'")]
    InvalidProtocol { proto: [u8; 8] },

    // #[error("unknown method {method}")]
    // UnknownMethod { method: String } 
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
        write_array(writer, *b"ksync\0\0\0").await?;
        write_data(writer, self.method.as_bytes()).await?;
        write_data(writer, &self.data).await?;

        writer.flush().await?;

        Ok(())
    }
}

/// Reads a [Packet] from a given `reader`
pub async fn read_packet<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Option<Packet>> {
    // read protocol magic
    let proto = read_array(reader).await;

    match proto {
        Ok(proto) => {
            // check magic
            if &proto != b"ksync\0\0\0" {
                return Err(io::Error::new(io::ErrorKind::InvalidData, Error::InvalidProtocol { proto: proto }))
            }

            // read method string
            let method = String::from_utf8(read_data(reader).await?)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            // read data bytes
            let data = read_data(reader).await?;

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

/// A type signature representing an implementation of [Method::call_bytes]
pub type MethodFn = fn(&Files, bytes: Vec<u8>) -> anyhow::Result<Vec<u8>>;

/// The [Method] trait is used to implement different methods of the protocol (e.g. `GET`, `INSERT`, etc.)
pub trait Method {
    type Input<'a>: Serialize + Deserialize<'a>;
    type Output: Serialize + DeserializeOwned;

    /// UTF-8 string identifier of the method. Used to dynamically dispatch a request to it's responder
    const NAME: &'static str;

    /// A wrapper over [Method::call] that deserialises input, and serialises output, automatically
    fn call_bytes(files: &Files, bytes: Vec<u8>) -> anyhow::Result<Vec<u8>> {
        let input = bincode::deserialize(&bytes)?;
        let output = Self::call(files, input)?;
        let output_bytes = bincode::serialize(&output)?;

        Ok(output_bytes)
    }

    /// The functionality to be invoked when a method is called
    fn call<'a>(files: &Files, input: Self::Input<'a>) -> anyhow::Result<Self::Output>;
}

pub async fn invoke<'a, M: Method, S: AsyncReadExt + AsyncWriteExt + Unpin>(stream: &mut S, method: M, input: M::Input<'a>) -> anyhow::Result<M::Output> {
    write_packet(stream, M::NAME, input).await?;

    let response = read_packet(stream).await?
        .ok_or({
            let err: io::Error = io::ErrorKind::UnexpectedEof.into();
            err
        })?;

    if response.method == "OK" {
        let result = bincode::deserialize(&response.data)?;
        Ok(result)
    } else {
        let err: &str = bincode::deserialize(&response.data)?;
        Err(io::Error::new(io::ErrorKind::Other, err).into())
    }
}