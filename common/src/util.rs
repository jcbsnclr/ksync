use std::io;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub async fn read_array<const N: usize, R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<[u8; N]> {
    let mut buf = [0; N];

    reader.read(&mut buf).await?;

    Ok(buf)
}

pub async fn read_int<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<u64> {
    let bytes = read_array(reader).await?;

    Ok(u64::from_le_bytes(bytes))
}

pub async fn read_data<R: AsyncReadExt + Unpin>(reader: &mut R) -> io::Result<Vec<u8>> {
    let len = read_int(reader).await? as usize;

    let mut buf = vec![0; len];
    
    reader.read_exact(&mut buf).await?;

    Ok(buf)
}

pub async fn write_array<const N: usize, W: AsyncWriteExt + Unpin>(writer: &mut W, array: [u8; N]) -> io::Result<()> {
    match writer.write(&array).await? {
        n if n == N => Ok(()),
        n => Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("tried to write {N} bytes, actually wrote {n}")))
    }
}

pub async fn write_int<W: AsyncWriteExt + Unpin>(writer: &mut W, val: u64) -> io::Result<()> {
    write_array(writer, val.to_le_bytes()).await
}

pub async fn write_data<W: AsyncWriteExt + Unpin>(writer: &mut W, data: &[u8]) -> io::Result<()> {
    write_int(writer, data.len() as u64).await?;
    writer.write(&data).await?;

    Ok(())
}