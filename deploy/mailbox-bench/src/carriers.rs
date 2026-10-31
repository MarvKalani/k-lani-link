//! Comparison carriers only. Not production APIs or a reconnect protocol.
use super::*;
use crate::retention::Cursor;
use axum::extract::{
    Query,
    ws::{Message, WebSocketUpgrade},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::SinkExt;
use rustls::pki_types::{CertificateDer, pem::PemObject};
use tokio_tungstenite::{
    Connector, MaybeTlsStream, WebSocketStream,
    tungstenite::{client::IntoClientRequest, protocol::WebSocketConfig},
};

pub const MODES: &[&str] = &[
    "mailbox",
    "stream",
    "post-stream",
    "sse",
    "longpoll",
    "h2-stream",
    "wss",
];
pub fn finite(mode: &str) -> bool {
    matches!(mode, "mailbox" | "post-stream" | "sse" | "longpoll")
}

fn sse(packet: &[u8]) -> Bytes {
    Bytes::from(format!("data: {}\n\n", STANDARD.encode(packet)))
}
pub fn take_sse(buffer: &mut BytesMut) -> Result<Option<Bytes>> {
    let maximum = MAX_PACKET_BYTES.div_ceil(3) * 4 + 8;
    let Some(end) = buffer.windows(2).position(|w| w == b"\n\n") else {
        ensure!(buffer.len() <= maximum, "SSE record too large");
        return Ok(None);
    };
    ensure!(end + 2 <= maximum, "SSE record too large");
    let frame = buffer.split_to(end + 2);
    ensure!(frame.starts_with(b"data: "), "invalid SSE envelope");
    let packet = STANDARD.decode(&frame[6..end])?;
    ensure!(
        (28..=MAX_PACKET_BYTES).contains(&packet.len()),
        "invalid SSE payload length"
    );
    Ok(Some(Bytes::from(packet)))
}
pub async fn stream_get(
    state: State<Arc<App>>,
    Path(room): Path<String>,
    query: Query<Cursor>,
    headers: HeaderMap,
) -> Response {
    read(state, Path((room, "stream".into())), query, headers).await
}
pub async fn read(
    State(app): State<Arc<App>>,
    Path((room, kind)): Path<(String, String)>,
    Query(cursor): Query<Cursor>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !matches!(kind.as_str(), "mailbox" | "poll" | "stream" | "sse") {
        return StatusCode::BAD_REQUEST.into_response();
    }
    app.gets.fetch_add(1, Relaxed);
    let Some(queue) = app.queue(room) else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    if let Err(status) = retention::acknowledge(&app, &queue, cursor.from) {
        return status.into_response();
    }
    let mut next = cursor.from;
    if matches!(kind.as_str(), "mailbox" | "poll") {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(25);
        loop {
            let packets = match retention::read(&app, &queue, &mut next, kind == "poll") {
                Ok(p) => p,
                Err(status) => return status.into_response(),
            };
            if !packets.is_empty() {
                let mut body = Vec::new();
                for packet in packets {
                    body.extend_from_slice(&prefixed(&packet));
                }
                return (
                    [
                        ("cache-control", "no-store"),
                        ("content-type", "application/octet-stream"),
                    ],
                    body,
                )
                    .into_response();
            }
            if tokio::time::timeout_at(deadline, queue.notify.notified())
                .await
                .is_err()
            {
                return StatusCode::NO_CONTENT.into_response();
            }
        }
    }
    let is_sse = kind == "sse";
    let output = async_stream::stream! {
        loop {
            let packets=match retention::read(&app,&queue,&mut next,false) { Ok(p)=>p, Err(_)=>break };
            if packets.is_empty() {
                if tokio::time::timeout(Duration::from_secs(90),queue.notify.notified()).await.is_err() { break; }
            } else {
                for packet in packets { yield Ok::<Bytes,Infallible>(if is_sse { sse(&packet) } else { prefixed(&packet) }); }
            }
        }
    };
    Response::builder()
        .header("cache-control", "no-store")
        .header(
            "content-type",
            if is_sse {
                "text/event-stream"
            } else {
                "application/octet-stream"
            },
        )
        .body(Body::from_stream(output))
        .unwrap()
}
pub async fn websocket(
    State(app): State<Arc<App>>,
    Path((room, direction)): Path<(String, String)>,
    Query(cursor): Query<Cursor>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if direction != "send" && direction != "receive" {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let Some(queue) = app.queue(room) else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    if direction == "receive" {
        app.gets.fetch_add(1, Relaxed);
        if let Err(status) = retention::acknowledge(&app, &queue, cursor.from) {
            return status.into_response();
        }
    } else {
        app.puts.fetch_add(1, Relaxed);
    }
    ws.read_buffer_size(16 * 1024)
        .max_message_size(MAX_PACKET_BYTES)
        .max_frame_size(MAX_PACKET_BYTES)
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_PACKET_BYTES * 2)
        .on_upgrade(move |mut socket| async move {
            if direction == "receive" {
                let mut next = cursor.from;
                loop {
                    let packets = match retention::read(&app, &queue, &mut next, false) {
                        Ok(p) => p,
                        Err(_) => break,
                    };
                    if packets.is_empty() {
                        if tokio::time::timeout(Duration::from_secs(90), queue.notify.notified())
                            .await
                            .is_err()
                        {
                            break;
                        }
                    } else {
                        for packet in packets {
                            if socket.send(Message::Binary(packet)).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            } else {
                while let Some(Ok(message)) = socket.recv().await {
                    match message {
                        Message::Binary(packet) => {
                            let Ok(seq) = retention::index(&packet) else {
                                break;
                            };
                            if retention::insert_wait(&app, &queue, seq, packet).await
                                != StatusCode::NO_CONTENT
                            {
                                break;
                            }
                        }
                        Message::Ping(_) | Message::Pong(_) => {}
                        _ => break,
                    }
                }
            }
        })
        .into_response()
}
pub type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
pub async fn connect(base: &str, room: &str, direction: &str, from: u64) -> Result<Ws> {
    let ca = std::fs::read(std::env::var("BENCH_CA")?)?;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(CertificateDer::from_pem_slice(&ca)?)?;
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut request = format!(
        "{}/ws/{room}/{direction}?from={from}",
        base.replacen("https://", "wss://", 1)
    )
    .into_client_request()?;
    request.headers_mut().insert("authorization", AUTH.parse()?);
    let ws_config = WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .max_message_size(Some(MAX_PACKET_BYTES))
        .max_frame_size(Some(MAX_PACKET_BYTES))
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_PACKET_BYTES * 2);
    let (socket, _) = tokio_tungstenite::connect_async_tls_with_config(
        request,
        Some(ws_config),
        true,
        Some(Connector::Rustls(Arc::new(config))),
    )
    .await?;
    Ok(socket)
}
pub async fn ws_send(base: &str, room: &str, rx: &mut mpsc::Receiver<Bytes>) -> Result<()> {
    let mut socket = connect(base, room, "send", 1).await?;
    while let Some(packet) = rx.recv().await {
        socket
            .send(tokio_tungstenite::tungstenite::Message::Binary(packet))
            .await?;
    }
    socket.close(None).await?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmented_sse_keeps_record_boundaries_and_rejects_oversized_or_invalid_data() {
        let packet = vec![7; MAX_PACKET_BYTES];
        let wire = sse(&packet);
        let mut buf = BytesMut::new();
        for chunk in wire[..wire.len() - 1].chunks(37) {
            buf.extend_from_slice(chunk);
            assert!(take_sse(&mut buf).unwrap().is_none());
        }
        buf.extend_from_slice(&wire[wire.len() - 1..]);
        assert_eq!(take_sse(&mut buf).unwrap().unwrap().as_ref(), packet);
        assert!(buf.is_empty());
        assert!(take_sse(&mut BytesMut::from(b"data: invalid!\n\n".as_slice())).is_err());
        assert!(
            take_sse(&mut BytesMut::from(
                sse(&vec![0; MAX_PACKET_BYTES + 1]).as_ref()
            ))
            .is_err()
        );
    }
    #[test]
    fn a_poll_batch_waits_for_a_gap_and_then_returns_contiguous_records() {
        let app = App::default();
        let queue = app.queue("synthetic".into()).unwrap();
        for index in [2, 3] {
            queue
                .records
                .lock()
                .unwrap()
                .insert(index, (Instant::now(), Bytes::from(vec![1; 28])));
        }
        app.buffered.store(56, Relaxed);
        let mut next = 1;
        assert!(
            retention::read(&app, &queue, &mut next, true)
                .unwrap()
                .is_empty()
        );
        queue
            .records
            .lock()
            .unwrap()
            .insert(1, (Instant::now(), Bytes::from(vec![1; 28])));
        app.buffered.fetch_add(28, Relaxed);
        assert_eq!(
            retention::read(&app, &queue, &mut next, true)
                .unwrap()
                .len(),
            3
        );
        assert_eq!(next, 4);
        assert_eq!(app.buffered.load(Relaxed), 0);
    }
}
