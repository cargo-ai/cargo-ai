//! Bounded capture for a generated executable's selected tool producer.
use std::{io, process::Output, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
const CHANNEL_BYTES: usize = 1024 * 1024;

async fn read<R: tokio::io::AsyncRead + Unpin>(stream: R) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    stream
        .take(CHANNEL_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .await?;
    if bytes.len() > CHANNEL_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Selected tool channel limit",
        ));
    }
    Ok(bytes)
}
pub(crate) async fn wait(
    mut child: tokio::process::Child,
    remaining: Duration,
    input: &[u8],
) -> io::Result<Output> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("Missing tool stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| io::Error::other("Missing tool stderr"))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("Missing tool stdin"))?;
    let write = async {
        stdin.write_all(input).await?;
        stdin.shutdown().await?;
        drop(stdin);
        Ok::<_, io::Error>(())
    };
    let capture = async {
        let (stdout, stderr, (), status) =
            tokio::try_join!(read(stdout), read(stderr), write, child.wait())?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    };
    tokio::time::timeout(remaining, capture)
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "Selected tool runtime limit"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn bounded_capture_keeps_exact_limit_and_rejects_limit_plus_one() {
        let bytes = vec![b'x'; CHANNEL_BYTES];
        assert_eq!(read(bytes.as_slice()).await.unwrap(), bytes);
        let oversized = vec![b'x'; CHANNEL_BYTES + 1];
        assert_eq!(
            read(oversized.as_slice()).await.unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
