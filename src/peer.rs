use crate::{
    config::{Config, RelayPolicy, random_id},
    signal::{Signaling, seal},
};
use anyhow::{Context, Result, bail, ensure};
use bytes::BytesMut;
use k_lani_link_protocol::{EndpointRole, Frame, SecureSession, Signal};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use webrtc::{
    data_channel::{DataChannel, DataChannelEvent},
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCConfigurationBuilder,
        RTCIceGatheringState, RTCIceServer, RTCIceTransportPolicy, RTCPeerConnectionState,
        RTCSessionDescription, SettingEngineBuilder,
    },
};

pub struct Peer {
    pub pc: Arc<dyn PeerConnection>,
    pub tx: mpsc::Sender<Frame>,
    pub rx: mpsc::Receiver<Frame>,
    pub closed: CancellationToken,
    pub opened: CancellationToken,
    pub transport_opened: CancellationToken,
    pub recovering: Arc<AtomicBool>,
    rejected: CancellationToken,
    crypto: Arc<Mutex<Option<SecureSession>>>,
    gathered: CancellationToken,
}

#[derive(Clone)]
struct Handler {
    incoming: mpsc::Sender<Frame>,
    outgoing: Arc<Mutex<Option<mpsc::Receiver<Frame>>>>,
    closed: CancellationToken,
    opened: CancellationToken,
    transport_opened: CancellationToken,
    ready_sent: CancellationToken,
    recovering: Arc<AtomicBool>,
    rejected: CancellationToken,
    crypto: Arc<Mutex<Option<SecureSession>>>,
    gathered: CancellationToken,
}
#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            self.gathered.cancel();
        }
    }
    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        match state {
            RTCPeerConnectionState::Disconnected => {
                // ICE may recover the same DTLS/SCTP session. Keep keys, counters and
                // pending frames until ICE declares failure; never re-pair here.
                self.recovering.store(true, Ordering::SeqCst);
            }
            RTCPeerConnectionState::Connected => {
                self.recovering.store(false, Ordering::SeqCst);
            }
            RTCPeerConnectionState::Failed | RTCPeerConnectionState::Closed => {
                self.recovering.store(false, Ordering::SeqCst);
                self.closed.cancel();
            }
            _ => {}
        }
    }
    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        self.attach(dc);
    }
}
impl Handler {
    fn encrypt(&self, frame: &Frame) -> Result<Vec<u8>> {
        self.crypto
            .lock()
            .unwrap()
            .as_mut()
            .ok_or(crate::pairing::PairingRejected)?
            .seal(frame)
            .map_err(|_| crate::pairing::PairingRejected.into())
    }
    fn attach(&self, dc: Arc<dyn DataChannel>) {
        let Some(mut outgoing) = self.outgoing.lock().unwrap().take() else {
            self.closed.cancel();
            return;
        };
        let read = self.clone();
        let reader = dc.clone();
        tokio::spawn(async move {
            let work = async {
                ensure!(
                    reader.label().await? == "link-v1"
                        && reader.ordered().await?
                        && reader.max_retransmits().await?.is_none()
                        && reader.max_packet_life_time().await?.is_none(),
                    "unexpected DataChannel"
                );
                while let Some(event) = reader.poll().await {
                    match event {
                        DataChannelEvent::OnOpen => {
                            ensure!(
                                read.crypto.lock().unwrap().is_some(),
                                "missing application keys"
                            );
                            read.transport_opened.cancel();
                        }
                        DataChannelEvent::OnMessage(message) => {
                            let frame = {
                                read.crypto
                                    .lock()
                                    .unwrap()
                                    .as_mut()
                                    .ok_or(crate::pairing::PairingRejected)?
                                    .open(&message.data)
                                    .map_err(|_| crate::pairing::PairingRejected)?
                            };
                            if matches!(frame, Frame::Ready) {
                                ensure!(!read.opened.is_cancelled(), "duplicate key confirmation");
                                read.ready_sent.cancelled().await;
                                read.opened.cancel();
                            } else {
                                ensure!(read.opened.is_cancelled(), "data before key confirmation");
                                read.incoming.send(frame).await?;
                            }
                        }
                        DataChannelEvent::OnClose
                        | DataChannelEvent::OnClosing
                        | DataChannelEvent::OnError => break,
                        _ => {}
                    }
                }
                Ok::<_, anyhow::Error>(())
            };
            tokio::select! { _ = read.closed.cancelled() => {}, result = work => {
                if result.is_err() { read.rejected.cancel(); }
            } }
            read.closed.cancel();
        });
        let write = self.clone();
        tokio::spawn(async move {
            let work = async {
                write.transport_opened.cancelled().await;
                send_key_confirmation(&*dc, &write).await?;
                write.opened.cancelled().await;
                while let Some(frame) = outgoing.recv().await {
                    ensure!(
                        !matches!(frame, Frame::Ready),
                        "key confirmation is transport-owned"
                    );
                    let bytes = write.encrypt(&frame)?;
                    // The WebRTC driver enforces the configured outstanding-byte limit.
                    dc.send(BytesMut::from(bytes.as_slice())).await?;
                }
                Ok::<_, anyhow::Error>(())
            };
            tokio::select! { _ = write.closed.cancelled() => {}, _ = work => {} }
            write.closed.cancel();
        });
    }
}

async fn send_key_confirmation(channel: &dyn DataChannel, handler: &Handler) -> Result<()> {
    let packet = handler.encrypt(&Frame::Ready)?;
    channel.send(BytesMut::from(packet.as_slice())).await?;
    handler.ready_sent.cancel();
    Ok(())
}

impl Peer {
    pub async fn new(config: &Config, offerer: bool) -> Result<Self> {
        Self::build(config, offerer, PeerConnectionBuilder::new()).await
    }
    async fn build(
        config: &Config,
        offerer: bool,
        builder: PeerConnectionBuilder<&'static str>,
    ) -> Result<Self> {
        config.validate()?;
        let (tx, out) = mpsc::channel(32);
        let (incoming, rx) = mpsc::channel(32);
        let closed = CancellationToken::new();
        let opened = CancellationToken::new();
        let transport_opened = CancellationToken::new();
        let ready_sent = CancellationToken::new();
        let recovering = Arc::new(AtomicBool::new(false));
        let rejected = CancellationToken::new();
        let crypto = Arc::new(Mutex::new(None));
        let gathered = CancellationToken::new();
        let handler = Arc::new(Handler {
            incoming,
            outgoing: Arc::new(Mutex::new(Some(out))),
            closed: closed.clone(),
            opened: opened.clone(),
            transport_opened: transport_opened.clone(),
            ready_sent,
            recovering: recovering.clone(),
            rejected: rejected.clone(),
            crypto: crypto.clone(),
            gathered: gathered.clone(),
        });
        let settings = SettingEngineBuilder::default()
            .with_include_loopback_candidate(config.include_loopback_candidate)
            .with_ice_timeouts(
                // 8 seconds without peer activity, then 15 more seconds to recover.
                Some(Duration::from_secs(8)),
                Some(Duration::from_secs(15)),
                Some(Duration::from_secs(2)),
            )
            .build();
        let rtc_config = RTCConfigurationBuilder::default()
            .with_ice_servers(
                config
                    .ice_servers
                    .iter()
                    .map(|s| RTCIceServer {
                        urls: s.urls.clone(),
                        username: s.username.clone(),
                        credential: s.credential.clone(),
                    })
                    .collect(),
            )
            .with_ice_transport_policy(if config.relay == RelayPolicy::Required {
                RTCIceTransportPolicy::Relay
            } else {
                RTCIceTransportPolicy::All
            })
            .build();
        let pc: Arc<dyn PeerConnection> = Arc::new(
            builder
                .with_configuration(rtc_config)
                .with_setting_engine(settings)
                .with_handler(handler.clone())
                .with_data_channel_send_buffer_limit(128 * 1024)
                // webrtc expands 0.0.0.0 to every non-loopback interface, so a loopback
                // candidate needs its own bind. Loopback only reaches this machine; binding
                // it alone keeps local tests off Docker/VM bridges and their check storm.
                .with_udp_addrs(if config.include_loopback_candidate {
                    vec!["127.0.0.1:0"]
                } else {
                    vec!["0.0.0.0:0"]
                })
                .build()
                .await?,
        );
        if offerer {
            handler.attach(pc.create_data_channel("link-v1", None).await?);
        }
        Ok(Self {
            pc,
            tx,
            rx,
            closed,
            opened,
            transport_opened,
            recovering,
            rejected,
            crypto,
            gathered,
        })
    }
    pub async fn local_sdp(&self, offer: bool) -> Result<String> {
        let description = if offer {
            self.pc.create_offer(None).await?
        } else {
            self.pc.create_answer(None).await?
        };
        self.pc.set_local_description(description).await?;
        tokio::time::timeout(Duration::from_secs(25), self.gathered.cancelled())
            .await
            .context("ICE gathering timed out")?;
        Ok(self
            .pc
            .local_description()
            .await
            .context("missing local SDP")?
            .sdp)
    }
    pub fn install_secure_session(
        &self,
        code: &str,
        offer: &Signal,
        answer: &Signal,
        local: EndpointRole,
        offer_owner: EndpointRole,
    ) -> Result<()> {
        let mut crypto = self.crypto.lock().unwrap();
        ensure!(crypto.is_none(), "session keys cannot be replaced");
        *crypto = Some(
            SecureSession::new(
                code,
                offer,
                answer,
                local,
                offer_owner,
                crate::signal::now(),
            )
            .map_err(|_| crate::pairing::PairingRejected)?,
        );
        Ok(())
    }
    pub async fn set_remote(&self, sdp: String, offer: bool, relay: RelayPolicy) -> Result<()> {
        if relay == RelayPolicy::Disabled {
            ensure!(
                !sdp.lines().any(|line| line.starts_with("a=candidate:")
                    && line
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .windows(2)
                        .any(|w| w == ["typ", "relay"])),
                "peer offered TURN candidates but relay is disabled"
            );
        }
        let remote = if offer {
            RTCSessionDescription::offer(sdp)?
        } else {
            RTCSessionDescription::answer(sdp)?
        };
        self.pc.set_remote_description(remote).await?;
        Ok(())
    }
    pub async fn wait_open(&self) -> Result<()> {
        tokio::select! {
            biased;
            _ = self.rejected.cancelled() => Err(crate::pairing::PairingRejected.into()),
            _ = self.closed.cancelled() => bail!("peer closed during setup"),
            _ = tokio::time::sleep(Duration::from_secs(35)) => bail!("P2P connection timed out; check ICE servers/network or explicitly enable your own TURN"),
            _ = self.opened.cancelled() => Ok(())
        }
    }
    pub async fn close(&self) {
        self.closed.cancel();
        let _ = self.pc.close().await;
    }
}

#[cfg(test)]
mod tests;
impl Drop for Peer {
    fn drop(&mut self) {
        self.closed.cancel();
        let pc = self.pc.clone();
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let _ = pc.close().await;
            });
        }
    }
}

pub async fn connect(config: &Config) -> Result<Peer> {
    config.validate()?;
    if config.signaling_backend == crate::config::SignalingBackend::IceExchange {
        return crate::exchange::connect(config, EndpointRole::Client).await;
    }
    let signaling = Signaling::new(config)?;
    let peer = Peer::new(config, true).await?;
    let session = random_id()?;
    let offer = seal(
        config,
        &session,
        "offer",
        &peer.local_sdp(true).await?,
        EndpointRole::Client,
    )?;
    signaling.put(&offer).await?;
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Some(answer) = signaling.get("answer").await? {
                let sdp = crate::pairing::open(&answer, config, "answer", EndpointRole::Host)?;
                if answer.context.session != session {
                    return Err(crate::pairing::PairingRejected.into());
                }
                peer.install_secure_session(
                    &config.pair_key,
                    &offer,
                    &answer,
                    EndpointRole::Client,
                    EndpointRole::Client,
                )?;
                peer.set_remote(sdp, false, config.relay)
                    .await
                    .map_err(|_| crate::pairing::PairingRejected)?;
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await
    .context("waiting for host timed out")??;
    drop(signaling);
    peer.wait_open().await?;
    Ok(peer)
}

pub async fn accept(config: &Config, previous_session: &mut String) -> Result<Peer> {
    config.validate()?;
    if config.signaling_backend == crate::config::SignalingBackend::IceExchange {
        return crate::exchange::connect(config, EndpointRole::Host).await;
    }
    let signaling = Signaling::new(config)?;
    let (offer, sdp) = loop {
        if let Some(offer) = signaling.get("offer").await? {
            let sdp = crate::pairing::open(&offer, config, "offer", EndpointRole::Client)?;
            if offer.context.session != *previous_session {
                *previous_session = offer.context.session.clone();
                break (offer, sdp);
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    let peer = Peer::new(config, false).await?;
    peer.set_remote(sdp, true, config.relay)
        .await
        .map_err(|_| crate::pairing::PairingRejected)?;
    let answer = seal(
        config,
        &offer.context.session,
        "answer",
        &peer.local_sdp(false).await?,
        EndpointRole::Host,
    )?;
    peer.install_secure_session(
        &config.pair_key,
        &offer,
        &answer,
        EndpointRole::Host,
        EndpointRole::Client,
    )?;
    tokio::select! {
        biased;
        _ = peer.transport_opened.cancelled() => {},
        result = signaling.put(&answer) => { result?; }
    }
    drop(signaling);
    peer.wait_open().await?;
    Ok(peer)
}
