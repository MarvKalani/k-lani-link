//! The same application-frame interface for WebRTC and the explicit HTTPS carrier.
use crate::{mailbox::transport::HttpsTunnel, peer::Peer};
use k_lani_link_protocol::Frame;
use std::sync::{Arc, atomic::AtomicBool};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub enum Tunnel {
    Webrtc(Peer),
    Https(HttpsTunnel),
}
impl From<Peer> for Tunnel {
    fn from(peer: Peer) -> Self {
        Self::Webrtc(peer)
    }
}
impl From<HttpsTunnel> for Tunnel {
    fn from(peer: HttpsTunnel) -> Self {
        Self::Https(peer)
    }
}
impl Tunnel {
    pub fn tx(&self) -> &mpsc::Sender<Frame> {
        match self {
            Self::Webrtc(p) => &p.tx,
            Self::Https(p) => &p.tx,
        }
    }
    pub fn closed(&self) -> &CancellationToken {
        match self {
            Self::Webrtc(p) => &p.closed,
            Self::Https(p) => &p.closed,
        }
    }
    pub fn recovering(&self) -> &Arc<AtomicBool> {
        match self {
            Self::Webrtc(p) => &p.recovering,
            Self::Https(p) => &p.recovering,
        }
    }
    pub async fn receive(&mut self) -> Option<Frame> {
        match self {
            Self::Webrtc(p) => p.rx.recv().await,
            Self::Https(p) => p.rx.recv().await,
        }
    }
    pub async fn close(&mut self) {
        match self {
            Self::Webrtc(p) => p.close().await,
            Self::Https(p) => p.close().await,
        }
    }
}
