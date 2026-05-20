//! TCP transport for the bridge — same protobuf surface as the BLE GATT
//! impl, length-prefixed on a stream socket.
//!
//! # Wire format
//!
//! ```text
//! [len:u32 BE][type:u8][protobuf body]
//! ```
//!
//! `len` counts the type byte plus the body. Frame types:
//!
//! | Code  | Meaning                                  | Direction        |
//! |-------|------------------------------------------|------------------|
//! | 0x01  | [`ControlRequest`] body                  | client → server  |
//! | 0x02  | [`ControlEvent`] body                    | server → client  |
//! | 0x03  | [`crate::Info`] body, sent once on connect | server → client |
//!
//! Multiple concurrent clients are supported; each gets an independent
//! [`broadcast::Receiver`] subscription so events fan out to all of them.

use std::net::SocketAddr;

use bytes::{BufMut, BytesMut};
use prost::Message;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{
    tcp::{OwnedReadHalf, OwnedWriteHalf},
    TcpListener, TcpStream,
};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use crate::{ControlEvent, ControlRequest, DeviceInfo};

const FRAME_REQUEST: u8 = 0x01;
const FRAME_EVENT: u8 = 0x02;
const FRAME_INFO: u8 = 0x03;

/// Sanity bound on a single frame's payload — guards against a runaway length
/// field. Real bridge messages are tens to a few hundred bytes.
const MAX_FRAME_SIZE: usize = 64 * 1024;

pub(crate) async fn run(
    addr: SocketAddr,
    device_info: DeviceInfo,
    cmd_tx: mpsc::Sender<ControlRequest>,
    evt_tx: broadcast::Sender<ControlEvent>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    info!(%addr, "bridge: tcp listening");

    // Info is static for the life of the process — encode once.
    let info_bytes = {
        let proto = device_info.to_proto();
        let mut buf = Vec::with_capacity(proto.encoded_len());
        proto.encode(&mut buf).expect("encode Info");
        buf
    };

    loop {
        let (sock, peer) = listener.accept().await?;
        info!(%peer, "bridge: tcp client connected");

        let cmd_tx = cmd_tx.clone();
        let evt_rx = evt_tx.subscribe();
        let info_bytes = info_bytes.clone();

        tokio::spawn(async move {
            match handle_client(sock, info_bytes, cmd_tx, evt_rx).await {
                Ok(()) => info!(%peer, "bridge: tcp client disconnected"),
                Err(e) => warn!(%peer, error = %e, "bridge: tcp client error"),
            }
        });
    }
}

async fn handle_client(
    sock: TcpStream,
    info_bytes: Vec<u8>,
    cmd_tx: mpsc::Sender<ControlRequest>,
    evt_rx: broadcast::Receiver<ControlEvent>,
) -> anyhow::Result<()> {
    let (mut rd, mut wr) = sock.into_split();

    // Greet the client with Info so it immediately knows who it's talking to.
    write_frame(&mut wr, FRAME_INFO, &info_bytes).await?;

    // Spawn the writer so reads and writes proceed independently.
    let writer = tokio::spawn(writer_loop(wr, evt_rx));

    let read_result = reader_loop(&mut rd, cmd_tx).await;

    // Closing the read half signals the peer; abort the writer so it stops
    // trying to send into a dead socket.
    writer.abort();
    read_result
}

async fn reader_loop(
    rd: &mut OwnedReadHalf,
    cmd_tx: mpsc::Sender<ControlRequest>,
) -> anyhow::Result<()> {
    loop {
        let Some((frame_type, body)) = read_frame(rd).await? else {
            return Ok(()); // clean EOF
        };
        match frame_type {
            FRAME_REQUEST => match ControlRequest::decode(body.as_slice()) {
                Ok(req) => {
                    debug!(?req, "bridge: command");
                    if cmd_tx.send(req).await.is_err() {
                        warn!("bridge: command receiver dropped");
                        return Ok(());
                    }
                }
                Err(e) => warn!(error = %e, "bridge: invalid ControlRequest"),
            },
            other => warn!(
                frame_type = other,
                "bridge: unexpected frame type from client"
            ),
        }
    }
}

async fn writer_loop(mut wr: OwnedWriteHalf, mut evt_rx: broadcast::Receiver<ControlEvent>) {
    loop {
        match evt_rx.recv().await {
            Ok(evt) => {
                let mut body = Vec::with_capacity(evt.encoded_len());
                if let Err(e) = evt.encode(&mut body) {
                    warn!(error = %e, "bridge: encode event failed");
                    continue;
                }
                if let Err(e) = write_frame(&mut wr, FRAME_EVENT, &body).await {
                    debug!(error = %e, "bridge: tcp write error — peer gone");
                    return;
                }
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(lagged = n, "bridge: event broadcast lagged");
            }
            Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn write_frame<W>(w: &mut W, frame_type: u8, body: &[u8]) -> std::io::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let len = u32::try_from(1 + body.len()).expect("frame fits in u32");
    let mut hdr = BytesMut::with_capacity(5);
    hdr.put_u32(len);
    hdr.put_u8(frame_type);
    w.write_all(&hdr).await?;
    w.write_all(body).await?;
    Ok(())
}

async fn read_frame<R>(r: &mut R) -> anyhow::Result<Option<(u8, Vec<u8>)>>
where
    R: AsyncReadExt + Unpin,
{
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 {
        anyhow::bail!("bridge: tcp frame with zero length");
    }
    if len > MAX_FRAME_SIZE {
        anyhow::bail!(
            "bridge: tcp frame length {} exceeds {}",
            len,
            MAX_FRAME_SIZE
        );
    }
    let mut frame = vec![0u8; len];
    r.read_exact(&mut frame).await?;
    let frame_type = frame[0];
    let body = frame.split_off(1);
    Ok(Some((frame_type, body)))
}
