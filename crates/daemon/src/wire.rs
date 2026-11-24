//! The daemon's wire format, the same over a Unix socket and (later) a Windows named pipe.
//!
//! Every message is `u32 length (big-endian) | u8 kind | payload`, `length` counting the kind
//! byte and the payload:
//!
//! - kind 0: one JSON object (a request, a response or an event).
//! - kind 1: a video frame: `u32 header length | JSON header | frame bytes`. The bytes are one
//!   access unit as length-prefixed NAL units without parameter sets, the layout a WebCodecs
//!   `VideoDecoder` configured with the `hvcC` description takes.

use anyhow::{Result, bail};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const KIND_JSON: u8 = 0;
const KIND_FRAME: u8 = 1;
/// Larger messages are a broken stream, not data: a keyframe is well under this.
const MAX_MESSAGE: u32 = 64 * 1024 * 1024;

/// One message off the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Json(Value),
    Frame { header: Value, data: Vec<u8> },
}

impl Message {
    pub fn encode(&self) -> Vec<u8> {
        let (kind, payload) = match self {
            Message::Json(v) => (KIND_JSON, serde_json::to_vec(v).expect("JSON values serialize")),
            Message::Frame { header, data } => {
                let h = serde_json::to_vec(header).expect("JSON values serialize");
                let mut p = Vec::with_capacity(4 + h.len() + data.len());
                p.extend_from_slice(&(h.len() as u32).to_be_bytes());
                p.extend_from_slice(&h);
                p.extend_from_slice(data);
                (KIND_FRAME, p)
            }
        };
        let mut out = Vec::with_capacity(5 + payload.len());
        out.extend_from_slice(&((payload.len() + 1) as u32).to_be_bytes());
        out.push(kind);
        out.extend_from_slice(&payload);
        out
    }

    pub fn decode(kind: u8, payload: &[u8]) -> Result<Self> {
        match kind {
            KIND_JSON => Ok(Message::Json(serde_json::from_slice(payload)?)),
            KIND_FRAME => {
                if payload.len() < 4 {
                    bail!("frame message too short");
                }
                let hl = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) as usize;
                let Some(h) = payload.get(4..4 + hl) else { bail!("frame header cut off") };
                Ok(Message::Frame { header: serde_json::from_slice(h)?, data: payload[4 + hl..].to_vec() })
            }
            other => bail!("unknown message kind {other}"),
        }
    }
}

pub async fn read<R: AsyncRead + Unpin>(r: &mut R) -> Result<Option<Message>> {
    let mut len = [0u8; 4];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len);
    if len == 0 || len > MAX_MESSAGE {
        bail!("bad message length {len} from the daemon connection");
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).await?;
    Message::decode(buf[0], &buf[1..]).map(Some)
}

pub async fn write<W: AsyncWrite + Unpin>(w: &mut W, m: &Message) -> Result<()> {
    w.write_all(&m.encode()).await?;
    w.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn messages_round_trip() {
        let msgs = [
            Message::Json(json!({ "id": 1, "cmd": "tap", "args": { "x": 1.5 } })),
            Message::Frame { header: json!({ "pts_us": 5, "keyframe": true }), data: vec![0, 0, 0, 2, 0x26, 1] },
        ];
        let mut buf = Vec::new();
        for m in &msgs {
            write(&mut buf, m).await.unwrap();
        }
        let mut r = &buf[..];
        for m in &msgs {
            assert_eq!(read(&mut r).await.unwrap().as_ref(), Some(m));
        }
        assert_eq!(read(&mut r).await.unwrap(), None);
    }
}
