//! Bounded PROXY v2 parsing. Never consumes any byte of the following TLS record.
use std::{
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncReadExt};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyHeader {
    pub source: SocketAddr,
    pub destination: SocketAddr,
}
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("invalid PROXY v2 signature")]
    BadSignature,
    #[error("unsupported PROXY v2 {0}")]
    Unsupported(&'static str),
    #[error("PROXY v2 payload too long: {0}")]
    TooLong(u16),
    #[error("truncated PROXY v2 header")]
    Truncated,
    #[error("PROXY v2 deadline exceeded")]
    Timeout,
    #[error("PROXY v2 I/O error")]
    Io(#[source] io::Error),
}
async fn exact<R: AsyncRead + Unpin>(io: &mut R, bytes: &mut [u8]) -> Result<(), ProxyError> {
    io.read_exact(bytes).await.map(|_| ()).map_err(|e| {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            ProxyError::Truncated
        } else {
            ProxyError::Io(e)
        }
    })
}
pub async fn read_proxy_v2<R: AsyncRead + Unpin>(
    io: &mut R,
    timeout: Duration,
) -> Result<ProxyHeader, ProxyError> {
    tokio::time::timeout(timeout, async {
        let mut fixed = [0; 16];
        exact(io, &mut fixed).await?;
        if &fixed[..12] != b"\r\n\r\n\0\r\nQUIT\n" {
            return Err(ProxyError::BadSignature);
        }
        if fixed[12] != 0x21 {
            return Err(ProxyError::Unsupported("version/command"));
        }
        let min = match fixed[13] {
            0x11 => 12,
            0x21 => 36,
            _ => return Err(ProxyError::Unsupported("family/protocol")),
        };
        let len = u16::from_be_bytes([fixed[14], fixed[15]]);
        if len > 512 {
            return Err(ProxyError::TooLong(len));
        }
        if usize::from(len) < min {
            return Err(ProxyError::Truncated);
        }
        let mut payload = vec![0; usize::from(len)];
        exact(io, &mut payload).await?;
        let (src, dst, ports) = if fixed[13] == 0x11 {
            (
                IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(&payload[..4]).unwrap())),
                IpAddr::V4(Ipv4Addr::from(<[u8; 4]>::try_from(&payload[4..8]).unwrap())),
                8,
            )
        } else {
            (
                IpAddr::V6(Ipv6Addr::from(
                    <[u8; 16]>::try_from(&payload[..16]).unwrap(),
                )),
                IpAddr::V6(Ipv6Addr::from(
                    <[u8; 16]>::try_from(&payload[16..32]).unwrap(),
                )),
                32,
            )
        };
        Ok(ProxyHeader {
            source: SocketAddr::new(
                src,
                u16::from_be_bytes([payload[ports], payload[ports + 1]]),
            ),
            destination: SocketAddr::new(
                dst,
                u16::from_be_bytes([payload[ports + 2], payload[ports + 3]]),
            ),
        })
    })
    .await
    .map_err(|_| ProxyError::Timeout)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::proxy_v2_header;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn parse(bytes: &[u8]) -> Result<ProxyHeader, ProxyError> {
        let (mut tx, mut rx) = tokio::io::duplex(1024);
        tx.write_all(bytes).await.unwrap();
        tx.shutdown().await.unwrap();
        read_proxy_v2(&mut rx, Duration::from_millis(100)).await
    }
    fn header() -> Vec<u8> {
        proxy_v2_header(
            "203.0.113.7:4444".parse().unwrap(),
            "127.0.0.1:9443".parse().unwrap(),
        )
    }
    #[tokio::test]
    async fn parses_tcp4_and_tcp6() {
        for (src, dst) in [
            ("203.0.113.7:4444", "127.0.0.1:9443"),
            ("[2001:db8::7]:4444", "[::1]:9443"),
        ] {
            let source = src.parse().unwrap();
            let destination = dst.parse().unwrap();
            assert_eq!(
                parse(&proxy_v2_header(source, destination)).await.unwrap(),
                ProxyHeader {
                    source,
                    destination
                }
            );
        }
    }
    #[tokio::test]
    async fn skips_tlvs() {
        let mut h = header();
        h[15] += 6;
        h.extend_from_slice(&[0x01, 0, 3, 1, 2, 3]);
        h.extend_from_slice(b"TLS-after-TLVs");
        let mut input = h.as_slice();
        assert_eq!(
            read_proxy_v2(&mut input, Duration::from_millis(100))
                .await
                .unwrap(),
            parse(&header()).await.unwrap()
        );
        assert_eq!(input, b"TLS-after-TLVs");
    }
    #[tokio::test]
    async fn leaves_following_bytes_unread() {
        let mut h = header();
        h.extend_from_slice(b"TLS");
        let mut input = h.as_slice();
        read_proxy_v2(&mut input, Duration::from_millis(100))
            .await
            .unwrap();
        assert_eq!(input, b"TLS");
    }
    #[tokio::test]
    async fn fragmented_one_byte_at_a_time() {
        let (mut tx, mut rx) = tokio::io::duplex(64);
        tokio::spawn(async move {
            for byte in header() {
                tx.write_all(&[byte]).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        assert_eq!(
            read_proxy_v2(&mut rx, Duration::from_secs(1))
                .await
                .unwrap(),
            parse(&header()).await.unwrap()
        );
    }
    #[tokio::test]
    async fn rejects_v1_text_header() {
        assert!(matches!(
            parse(b"PROXY TCP4 1.2.3.4 5.6.7.8 123 456\r\n").await,
            Err(ProxyError::BadSignature)
        ));
    }
    #[tokio::test]
    async fn rejects_bad_signature() {
        let mut h = header();
        h[0] = 0;
        assert!(matches!(parse(&h).await, Err(ProxyError::BadSignature)));
    }
    #[tokio::test]
    async fn rejects_local_command() {
        let mut h = header();
        h[12] = 0x20;
        assert!(matches!(parse(&h).await, Err(ProxyError::Unsupported(_))));
    }
    #[tokio::test]
    async fn rejects_udp_family() {
        let mut h = header();
        h[13] = 0x12;
        assert!(matches!(parse(&h).await, Err(ProxyError::Unsupported(_))));
    }
    #[tokio::test]
    async fn rejects_length_over_512() {
        let mut h = header();
        h[14..16].copy_from_slice(&513u16.to_be_bytes());
        assert!(matches!(parse(&h).await, Err(ProxyError::TooLong(513))));
    }
    #[tokio::test]
    async fn truncated_then_eof() {
        for end in [0, 8, 16, 20, 27] {
            assert!(matches!(
                parse(&header()[..end]).await,
                Err(ProxyError::Truncated)
            ));
        }
    }
    #[tokio::test]
    async fn slow_header_times_out() {
        let (mut tx, mut rx) = tokio::io::duplex(64);
        tx.write_all(&header()[..8]).await.unwrap();
        let start = std::time::Instant::now();
        assert!(matches!(
            read_proxy_v2(&mut rx, Duration::from_millis(30)).await,
            Err(ProxyError::Timeout)
        ));
        assert!(start.elapsed() < Duration::from_secs(1));
        let mut b = [0];
        assert!(
            tokio::time::timeout(Duration::from_millis(5), rx.read(&mut b))
                .await
                .is_err()
        );
    }
}
