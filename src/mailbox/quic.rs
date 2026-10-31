//! Optional HTTP/3 (QUIC) carrier for the same mailbox requests. Experimental: `h3` is a
//! 0.0.x release. Off unless `http3 = true`. Every QUIC failure is a network failure; the
//! client then uses TCP (HTTP/2 or HTTP/1.1) for its remaining requests, so the session,
//! its keys and cursors are unaffected.
use super::http::Failure;
use anyhow::Result;
use axum::body::Bytes;
use bytes::Buf;
use http_body_util::BodyExt;
use rustls::pki_types::{CertificateDer, pem::PemObject};
use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::Semaphore, time::Instant};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// A network that silently drops UDP costs at most this once per client.
const HANDSHAKE: Duration = Duration::from_millis(1500);
/// After a QUIC failure the client stays on TCP this long.
const RETRY_QUIC: Duration = Duration::from_secs(300);
const IDLE: Duration = Duration::from_secs(30);
const KEEP_ALIVE: Duration = Duration::from_secs(10);
const STREAMS: u32 = 64;
const HEADER_BYTES: u64 = 16 * 1024;

/// Flow-control windows bound what quinn buffers before anything is read. The relay only
/// receives single records (a JSON-wrapped one is under 17 KiB); the client receives
/// whole answers of up to 32 records.
const RELAY_STREAM_WINDOW: u32 = 32 * 1024;
const RELAY_CONNECTION_WINDOW: u32 = 1024 * 1024;
const CLIENT_STREAM_WINDOW: u32 = 512 * 1024;
const CLIENT_CONNECTION_WINDOW: u32 = 2 * 1024 * 1024;

fn transport(stream_window: u32, connection_window: u32) -> Result<quinn::TransportConfig> {
    let mut transport = quinn::TransportConfig::default();
    transport
        .stream_receive_window(stream_window.into())
        .receive_window(connection_window.into())
        .send_window(u64::from(CLIENT_CONNECTION_WINDOW))
        .max_concurrent_bidi_streams(STREAMS.into())
        // HTTP/3 control and QPACK streams.
        .max_concurrent_uni_streams(8u32.into())
        .max_idle_timeout(Some(IDLE.try_into()?))
        .keep_alive_interval(Some(KEEP_ALIVE));
    Ok(transport)
}

pub(crate) type Sender = h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>;
pub type Stream = h3::client::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>;

pub struct Client {
    endpoint: quinn::Endpoint,
    host: String,
    port: u16,
    /// Where the relay is mounted, without a trailing slash.
    prefix: String,
    connection: tokio::sync::Mutex<Option<Sender>>,
    down_until: Mutex<Option<Instant>>,
}

impl Client {
    pub fn new(origin: &str, ca_pem: Option<&[u8]>) -> Result<Self> {
        let url = url::Url::parse(origin)?;
        let host = url.host_str().ok_or(Failure::Invalid)?.to_owned();
        let port = url.port_or_known_default().ok_or(Failure::Invalid)?;
        let prefix = url.path().trim_end_matches('/').to_owned();
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        for certificate in ca_pem.into_iter().flat_map(CertificateDer::pem_slice_iter) {
            roots.add(certificate.map_err(|_| Failure::Invalid)?)?;
        }
        let mut tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_root_certificates(roots)
        .with_no_client_auth();
        tls.alpn_protocols = vec![b"h3".to_vec()];
        let mut config = quinn::ClientConfig::new(Arc::new(
            quinn::crypto::rustls::QuicClientConfig::try_from(tls)?,
        ));
        config.transport_config(Arc::new(transport(
            CLIENT_STREAM_WINDOW,
            CLIENT_CONNECTION_WINDOW,
        )?));
        let mut endpoint = quinn::Endpoint::client((std::net::Ipv6Addr::UNSPECIFIED, 0).into())
            .or_else(|_| quinn::Endpoint::client((std::net::Ipv4Addr::UNSPECIFIED, 0).into()))?;
        endpoint.set_default_client_config(config);
        Ok(Self {
            endpoint,
            host,
            port,
            prefix,
            connection: tokio::sync::Mutex::new(None),
            down_until: Mutex::new(None),
        })
    }

    pub fn usable(&self) -> bool {
        self.down_until
            .lock()
            .unwrap()
            .is_none_or(|until| Instant::now() >= until)
    }

    /// The caller falls back to TCP; QUIC is tried again after `RETRY_QUIC`.
    pub fn failed(&self) {
        *self.down_until.lock().unwrap() = Some(Instant::now() + RETRY_QUIC);
        if let Ok(mut connection) = self.connection.try_lock() {
            *connection = None;
        }
    }

    pub(crate) async fn sender(&self) -> Result<Sender> {
        let mut connection = self.connection.lock().await;
        if let Some(sender) = connection.as_ref() {
            return Ok(sender.clone());
        }
        let established = tokio::time::timeout(HANDSHAKE, async {
            // A name often resolves to IPv6 and IPv4 while the relay listens for UDP on only
            // one of them: try every address at once, the first finished handshake wins.
            let attempts: Vec<_> = tokio::net::lookup_host((self.host.as_str(), self.port))
                .await?
                .map(|address| {
                    Box::pin(async move {
                        Ok::<_, anyhow::Error>(self.endpoint.connect(address, &self.host)?.await?)
                    })
                })
                .collect();
            if attempts.is_empty() {
                return Err(Failure::Network.into());
            }
            let (quic, _) = futures_util::future::select_ok(attempts).await?;
            let (mut driver, sender) = h3::client::builder()
                .max_field_section_size(HEADER_BYTES)
                .build(h3_quinn::Connection::new(quic))
                .await?;
            tokio::spawn(async move {
                let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
            });
            Ok::<_, anyhow::Error>(sender)
        })
        .await
        .map_err(|_| Failure::Network)?
        .map_err(|_| Failure::Network)?;
        *connection = Some(established.clone());
        Ok(established)
    }

    /// One mailbox request. Errors are always `Failure::Network`.
    pub async fn send(
        &self,
        method: &reqwest::Method,
        path: &str,
        token: &str,
        body: Option<Bytes>,
        content_type: &str,
    ) -> Result<(u16, http::HeaderMap, Stream)> {
        let work = async {
            let mut sender = self.sender().await?;
            let request = http::Request::builder()
                .method(method.as_str())
                .uri(format!(
                    "https://{}:{}{}{path}",
                    self.host, self.port, self.prefix
                ))
                .header("authorization", format!("Bearer {token}"))
                .header("cache-control", "no-store, no-transform")
                .header("content-type", content_type)
                .body(())?;
            let mut stream = sender.send_request(request).await?;
            if let Some(body) = body {
                stream.send_data(body).await?;
            }
            stream.finish().await?;
            let response = stream.recv_response().await?;
            let (parts, ()) = response.into_parts();
            Ok::<_, anyhow::Error>((parts.status.as_u16(), parts.headers, stream))
        };
        work.await.map_err(|_| Failure::Network.into())
    }
}

pub async fn chunk(stream: &mut Stream) -> Result<Option<Bytes>> {
    match stream.recv_data().await {
        Ok(Some(mut data)) => Ok(Some(data.copy_to_bytes(data.remaining()))),
        Ok(None) => Ok(None),
        Err(_) => Err(Failure::Network.into()),
    }
}

/// Binds the relay's UDP socket, so a start failure is reported instead of lost.
pub fn listen(address: SocketAddr, tls: &rustls::ServerConfig) -> Result<quinn::Endpoint> {
    let mut tls = tls.clone();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    tls.max_early_data_size = 0;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(tls)?,
    ));
    config.transport_config(Arc::new(transport(
        RELAY_STREAM_WINDOW,
        RELAY_CONNECTION_WINDOW,
    )?));
    Ok(quinn::Endpoint::server(config, address)?)
}

/// Serves the relay's router over HTTP/3. The same per-token quotas apply to every
/// request; QUIC has its own connection cap, address validation and no 0-RTT.
pub async fn serve(
    endpoint: quinn::Endpoint,
    router: axum::Router,
    maximum: usize,
    stop: CancellationToken,
) {
    let slots = Arc::new(Semaphore::new(maximum));
    loop {
        let incoming = tokio::select! {
            _ = stop.cancelled() => break,
            incoming = endpoint.accept() => match incoming { Some(incoming) => incoming, None => break },
        };
        // A spoofed source must first prove it receives our packets (no amplification).
        if !incoming.remote_address_validated() {
            let _ = incoming.retry();
            continue;
        }
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            incoming.refuse();
            continue;
        };
        let router = router.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let work = async {
                let quic = incoming.await?;
                let address = quic.remote_address();
                let mut connection = h3::server::builder()
                    .max_field_section_size(HEADER_BYTES)
                    .build::<_, Bytes>(h3_quinn::Connection::new(quic))
                    .await?;
                while let Some(resolver) = connection.accept().await? {
                    let router = router.clone();
                    tokio::spawn(async move {
                        if let Ok((request, stream)) = resolver.resolve_request().await {
                            let _ = respond(request, stream, router, address).await;
                        }
                    });
                }
                Ok::<_, anyhow::Error>(())
            };
            tokio::select! { _ = stop.cancelled() => {}, _ = work => {} }
        });
    }
    endpoint.close(0u32.into(), b"");
}

async fn respond(
    request: http::Request<()>,
    stream: h3::server::RequestStream<h3_quinn::BidiStream<Bytes>, Bytes>,
    router: axum::Router,
    address: std::net::SocketAddr,
) -> Result<()> {
    let (mut send, mut recv) = stream.split();
    // Hand the upload on as a stream: the router authorizes the token and reserves quota
    // before it reads anything, exactly as over TCP. Nothing is buffered here.
    let body = axum::body::Body::from_stream(async_stream::stream! {
        loop {
            match recv.recv_data().await {
                Ok(Some(mut data)) => yield Ok::<_, std::io::Error>(data.copy_to_bytes(data.remaining())),
                Ok(None) => break,
                Err(_) => {
                    yield Err(std::io::Error::other("HTTP/3 request stream failed"));
                    break;
                }
            }
        }
    });
    let (mut parts, ()) = request.into_parts();
    // The router matches the origin-form path; the authority stays with QUIC.
    parts.uri = parts
        .uri
        .path_and_query()
        .map_or("/", |path| path.as_str())
        .parse()?;
    parts.extensions.insert(axum::extract::ConnectInfo(address));
    let response = router
        .oneshot(http::Request::from_parts(parts, body))
        .await?;
    let (parts, mut body) = response.into_parts();
    send.send_response(http::Response::from_parts(parts, ()))
        .await?;
    while let Some(frame) = body.frame().await {
        if let Ok(data) = frame?.into_data() {
            send.send_data(data).await?;
        }
    }
    send.finish().await?;
    Ok(())
}
