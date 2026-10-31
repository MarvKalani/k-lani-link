//! Adapter for the existing Peer2Download ice-exchange v3 HTTP contract.
//! A sealed envelope travels in a fixed SDP-shaped carrier. No peer SDP is exposed.
use crate::{
    config::{Config, random_id},
    peer::Peer,
    signal::seal,
};
use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use k_lani_link_protocol::{EndpointRole, MAX_SIGNAL_BYTES, Signal};
use serde_json::{Value, json};
use std::time::Duration;

const AUTH_LINE: &str = "a=x-k-lani-link-v2:";
const CARRIER: &str = "v=0\r\no=- 0 0 IN IP4 0.0.0.0\r\ns=k-lani-link\r\nt=0 0\r\n";

fn encode(signal: &Signal) -> Result<String> {
    let carrier = format!(
        "{CARRIER}{AUTH_LINE}{}\r\n",
        STANDARD.encode(serde_json::to_vec(signal)?)
    );
    ensure!(
        carrier.len() < MAX_SIGNAL_BYTES - 1024,
        "encrypted carrier too large"
    );
    Ok(carrier)
}

fn decode(carrier: &str) -> Result<Signal> {
    let encoded = carrier
        .strip_prefix(CARRIER)
        .and_then(|s| s.strip_prefix(AUTH_LINE))
        .and_then(|s| s.strip_suffix("\r\n"))
        .context("missing encrypted k-lani-link v2 envelope")?;
    ensure!(!encoded.contains(['\r', '\n']), "invalid encrypted carrier");
    Ok(serde_json::from_slice(&STANDARD.decode(encoded)?)?)
}

async fn response_json(mut response: reqwest::Response) -> Result<Value> {
    if response.status() == reqwest::StatusCode::REQUEST_TIMEOUT {
        bail!("ice-exchange waiting for other peer");
    }
    response.error_for_status_ref()?;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len() + chunk.len() > MAX_SIGNAL_BYTES {
            return Err(crate::pairing::PairingRejected.into());
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| crate::pairing::PairingRejected.into())
}

pub async fn connect(cfg: &Config, endpoint: EndpointRole) -> Result<Peer> {
    cfg.validate()?;
    // Server JOIN uses a 30-second long poll. Allow its normal timeout/cleanup.
    let http = reqwest::Client::builder()
        .http1_only()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(0)
        .timeout(Duration::from_secs(40))
        .build()?;
    let base = format!("{}/{}", cfg.signaling_url.trim_end_matches('/'), cfg.room);
    let join = response_json(
        http.post(&base)
            .header("content-type", "application/json")
            .header("connection", "close")
            .send()
            .await?,
    )
    .await?;
    let offerer = match join["role"].as_str() {
        Some("offerer") => true,
        Some("answerer") => false,
        _ => return Err(crate::pairing::PairingRejected.into()),
    };
    let peer = Peer::new(cfg, offerer).await?;
    if offerer {
        let session = random_id()?;
        let signal = seal(
            cfg,
            &session,
            "offer",
            &peer.local_sdp(true).await?,
            endpoint,
        )?;
        let sdp = encode(&signal)?;
        let reply = response_json(
            http.post(format!("{base}/offer"))
                .header("connection", "close")
                .json(&json!({"sdp":sdp,"ice":[]}))
                .send()
                .await?,
        )
        .await?;
        // Candidates are already inside the authenticated, fully gathered SDP.
        // Never add the separate, unauthenticated `ice` array returned by the server.
        let answer = decode(
            reply["sdp"]
                .as_str()
                .ok_or(crate::pairing::PairingRejected)?,
        )
        .map_err(|_| crate::pairing::PairingRejected)?;
        if answer.context.session != session {
            return Err(crate::pairing::PairingRejected.into());
        }
        let sdp = crate::pairing::open(&answer, cfg, "answer", endpoint.opposite())?;
        peer.install_secure_session(&cfg.pair_key, &signal, &answer, endpoint, endpoint)?;
        peer.set_remote(sdp, false, cfg.relay)
            .await
            .map_err(|_| crate::pairing::PairingRejected)?;
    } else {
        let reply = response_json(
            http.get(format!("{base}/offer"))
                .header("connection", "close")
                .send()
                .await?,
        )
        .await?;
        let offer = decode(
            reply["sdp"]
                .as_str()
                .ok_or(crate::pairing::PairingRejected)?,
        )
        .map_err(|_| crate::pairing::PairingRejected)?;
        let sdp = crate::pairing::open(&offer, cfg, "offer", endpoint.opposite())?;
        peer.set_remote(sdp, true, cfg.relay)
            .await
            .map_err(|_| crate::pairing::PairingRejected)?;
        let answer = seal(
            cfg,
            &offer.context.session,
            "answer",
            &peer.local_sdp(false).await?,
            endpoint,
        )?;
        let sdp = encode(&answer)?;
        peer.install_secure_session(
            &cfg.pair_key,
            &offer,
            &answer,
            endpoint,
            endpoint.opposite(),
        )?;
        let acknowledgement = async {
            let reply = response_json(
                http.post(format!("{base}/answer"))
                    .header("connection", "close")
                    .json(&json!({"sdp":sdp,"ice":[]}))
                    .send()
                    .await?,
            )
            .await?;
            ensure!(reply["ok"] == true, "ice-exchange did not accept answer");
            Ok::<_, anyhow::Error>(())
        };
        // The authenticated peer is authoritative once the data channel opens.
        // Do not keep signaling alive just to await a late server acknowledgement.
        tokio::select! {
            biased;
            _ = peer.transport_opened.cancelled() => {},
            result = acknowledgement => { result?; }
        }
    }
    drop(http);
    peer.wait_open().await?;
    Ok(peer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signal::now;
    #[test]
    fn carrier_contains_no_plain_peer_sdp_and_rejects_tampering() {
        let cfg = crate::config::tests::config();
        let sdp = "v=0\r\na=fingerprint:sha-256 AB:CD\r\na=candidate:192.168.1.23\r\n";
        let envelope = seal(
            &cfg,
            &random_id().unwrap(),
            "offer",
            sdp,
            EndpointRole::Client,
        )
        .unwrap();
        let encoded = encode(&envelope).unwrap();
        assert!(!encoded.contains("AB:CD") && !encoded.contains("192.168.1.23"));
        let restored = decode(&encoded).unwrap();
        assert_eq!(
            restored
                .open(
                    &cfg.pair_key,
                    &cfg.room,
                    "offer",
                    now(),
                    EndpointRole::Client
                )
                .unwrap(),
            sdp
        );
        let mut altered = restored.clone();
        let first = if altered.ciphertext.starts_with('A') {
            "B"
        } else {
            "A"
        };
        altered.ciphertext.replace_range(0..1, first);
        assert!(
            altered
                .open(
                    &cfg.pair_key,
                    &cfg.room,
                    "offer",
                    now(),
                    EndpointRole::Client
                )
                .is_err()
        );
        assert!(decode("v=0\r\na=x-k-lani-link-v1:old\r\n").is_err());
    }
}
