// Wire format

use crate::{
    types::{
        request::{RawRequestHeader, RequestHeader},
        response::{RawResponseHeader, ResponseHeader},
        Version,
    },
    Config, Request, Response, Result,
};
use anyhow::{anyhow, bail};
use bytes::{BufMut, Bytes, BytesMut};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::codec::{FramedRead, FramedWrite, LengthDelimitedCodec};

const ANEMO: &[u8; 5] = b"anemo";

/// Returns a fully configured length-delimited codec for writing/reading
/// serialized frames to/from a socket.
pub(crate) fn network_message_frame_codec() -> LengthDelimitedCodec {
    //TODO pipe through config
    let config = Config::default();

    LengthDelimitedCodec::builder()
        .max_frame_length(config.max_frame_size())
        .length_field_length(4)
        .big_endian()
        .new_codec()
}

pub(crate) async fn read_version_frame<T: AsyncRead + Unpin>(
    recv_stream: &mut T,
) -> Result<Version> {
    let mut buf: [u8; 8] = [0; 8];
    recv_stream.read_exact(&mut buf).await?;
    if &buf[0..=4] != ANEMO || buf[7] != 0 {
        bail!("Invalid Protocol Header");
    }
    let version_be_bytes = [buf[5], buf[6]];
    let version = u16::from_be_bytes(version_be_bytes);
    Version::new(version)
}

pub(crate) async fn write_version_frame<T: AsyncWrite + Unpin>(
    send_stream: &mut T,
    version: Version,
) -> Result<()> {
    let mut buf: [u8; 8] = [0; 8];
    buf[0..=4].copy_from_slice(ANEMO);
    buf[5..=6].copy_from_slice(&version.to_u16().to_be_bytes());

    send_stream.write_all(&buf).await?;

    Ok(())
}

pub(crate) async fn write_request<T: AsyncWrite + Unpin>(
    send_stream: &mut FramedWrite<T, LengthDelimitedCodec>,
    request: Request<Bytes>,
) -> Result<()> {
    // Write Version Frame
    write_version_frame(send_stream.get_mut(), request.version()).await?;

    let (parts, body) = request.into_parts();

    // Write Request Header
    let raw_header = RawRequestHeader::from_header(parts);
    let mut buf = BytesMut::new();
    bincode::serialize_into((&mut buf).writer(), &raw_header)
        .expect("serialization should not fail");
    send_stream.send(buf.freeze()).await?;

    // Write Body
    send_stream.send(body).await?;

    Ok(())
}

pub(crate) async fn write_response<T: AsyncWrite + Unpin>(
    send_stream: &mut FramedWrite<T, LengthDelimitedCodec>,
    response: Response<Bytes>,
) -> Result<()> {
    // Write Version Frame
    write_version_frame(send_stream.get_mut(), response.version()).await?;

    let (parts, body) = response.into_parts();

    // Write Request Header
    let raw_header = RawResponseHeader::from_header(parts);
    let mut buf = BytesMut::new();
    bincode::serialize_into((&mut buf).writer(), &raw_header)
        .expect("serialization should not fail");
    send_stream.send(buf.freeze()).await?;

    // Write Body
    send_stream.send(body).await?;

    Ok(())
}

pub(crate) async fn read_request<T: AsyncRead + Unpin>(
    recv_stream: &mut FramedRead<T, LengthDelimitedCodec>,
) -> Result<Request<Bytes>> {
    // Read Version Frame
    let version = read_version_frame(recv_stream.get_mut()).await?;

    // Read Request Header
    let header_buf = recv_stream
        .next()
        .await
        .ok_or_else(|| anyhow!("unexpected EOF"))??;
    let raw_header: RawRequestHeader = bincode::deserialize(&header_buf)?;
    let request_header = RequestHeader::from_raw(raw_header, version);

    // Read Body
    let body = recv_stream
        .next()
        .await
        .ok_or_else(|| anyhow!("unexpected EOF"))??;

    let request = Request::from_parts(request_header, body.freeze());

    Ok(request)
}

pub(crate) async fn read_response<T: AsyncRead + Unpin>(
    recv_stream: &mut FramedRead<T, LengthDelimitedCodec>,
) -> Result<Response<Bytes>> {
    // Read Version Frame
    let version = read_version_frame(recv_stream.get_mut()).await?;

    // Read Request Header
    let header_buf = recv_stream
        .next()
        .await
        .ok_or_else(|| anyhow!("unexpected EOF"))??;
    let raw_header: RawResponseHeader = bincode::deserialize(&header_buf)?;
    let response_header = ResponseHeader::from_raw(raw_header, version)?;

    // Read Body
    let body = recv_stream
        .next()
        .await
        .ok_or_else(|| anyhow!("unexpected EOF"))??;

    let response = Response::from_parts(response_header, body.freeze());

    Ok(response)
}

#[cfg(test)]
mod test {
    use super::{read_version_frame, write_version_frame, Version};

    const HEADER: [u8; 8] = [b'a', b'n', b'e', b'm', b'o', 0, 1, 0];

    #[tokio::test]
    async fn read_version_header() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&HEADER);

        let version = read_version_frame(&mut buf.as_ref()).await.unwrap();
        assert_eq!(Version::V1, version);
    }

    #[tokio::test]
    async fn read_incorrect_version_header() {
        // ANEMO header incorrect
        let header = [b'h', b't', b't', b'p', b'3', 0, 1, 0];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header);

        read_version_frame(&mut buf.as_ref()).await.unwrap_err();

        // Reserved byte not 0
        let header = [b'a', b'n', b'e', b'm', b'o', 0, 1, 1];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header);

        read_version_frame(&mut buf.as_ref()).await.unwrap_err();

        // Version is not 1
        let header = [b'a', b'n', b'e', b'm', b'o', 1, 0, 0];
        let mut buf = Vec::new();
        buf.extend_from_slice(&header);

        read_version_frame(&mut buf.as_ref()).await.unwrap_err();
    }

    #[tokio::test]
    async fn write_version_header() {
        let mut buf = Vec::new();

        write_version_frame(&mut buf, Version::V1).await.unwrap();
        assert_eq!(HEADER.as_ref(), buf);
    }
}
