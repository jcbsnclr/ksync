//! Length-prefixed framing.
//!
//! `u32` little-endian length, then the postcard body. The prefix belongs to
//! us, not to postcard, so the framing can change independently of the schema.

use anyhow::{Context, Result, bail};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use super::MAX_FRAME;

pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    let mut header = [0u8; 4];
    reader
        .read_exact(&mut header)
        .await
        .context("reading frame header")?;

    let len = u32::from_le_bytes(header) as usize;

    if len == 0 {
        bail!("empty frame");
    }
    if len > MAX_FRAME {
        bail!("frame too large: {len} bytes");
    }

    // Allocation is bounded by the check above.
    let mut body = vec![0u8; len];
    reader
        .read_exact(&mut body)
        .await
        .context("reading frame body")?;

    Ok(body)
}

pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, body: &[u8]) -> Result<()> {
    if body.is_empty() {
        bail!("refusing to write empty frame");
    }
    if body.len() > MAX_FRAME {
        bail!("frame too large: {} bytes", body.len());
    }

    writer.write_all(&(body.len() as u32).to_le_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trips_a_frame() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"hello").await.unwrap();
        let body = read_frame(&mut &buffer[..]).await.unwrap();
        assert_eq!(body, b"hello");
    }

    #[tokio::test]
    async fn rejects_empty_and_oversized_frames() {
        assert!(write_frame(&mut Vec::new(), b"").await.is_err());

        let mut oversized = Vec::new();
        oversized.extend_from_slice(&((MAX_FRAME + 1) as u32).to_le_bytes());
        assert!(read_frame(&mut &oversized[..]).await.is_err());
    }

    #[tokio::test]
    async fn rejects_truncated_frames() {
        let mut truncated = Vec::new();
        truncated.extend_from_slice(&5u32.to_le_bytes());
        truncated.extend_from_slice(b"hi");
        assert!(read_frame(&mut &truncated[..]).await.is_err());
    }
}
