//! Trial access to a public relay: tokens the relay signs and checks by itself, without
//! keeping any of them. A token names a random space shared by its host and client token
//! and the moment it stops working; the relay's secret makes it unforgeable, and changing
//! that secret ends every trial at once.
use crate::diagnostic::Diagnostic;
use anyhow::Result;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::hmac;
use serde::Deserialize;
use std::{path::Path, time::Duration};

const PREFIX: &str = "kt1";
const DOMAIN: &[u8] = b"k-lani-link/trial-token/v1";
const PAYLOAD: usize = 1 + 1 + 16 + 8;
const TAG: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Host = 1,
    Client = 2,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub role: Role,
    pub space: [u8; 16],
    /// Unix seconds from which the token is refused.
    pub expires: u64,
}

fn payload(grant: &Grant) -> [u8; PAYLOAD] {
    let mut out = [0u8; PAYLOAD];
    out[0] = 1;
    out[1] = grant.role as u8;
    out[2..18].copy_from_slice(&grant.space);
    out[18..].copy_from_slice(&grant.expires.to_be_bytes());
    out
}
fn tag(key: &hmac::Key, payload: &[u8]) -> [u8; TAG] {
    let mut context = hmac::Context::with_key(key);
    context.update(DOMAIN);
    context.update(payload);
    context.sign().as_ref()[..TAG].try_into().unwrap()
}
pub fn key(secret: &[u8]) -> hmac::Key {
    hmac::Key::new(hmac::HMAC_SHA256, secret)
}

/// The token for `grant`: letters, digits, - and _ only, like every admission token.
pub fn seal(key: &hmac::Key, grant: &Grant) -> String {
    let payload = payload(grant);
    let mut bytes = payload.to_vec();
    bytes.extend_from_slice(&tag(key, &payload));
    format!("{PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn parts(token: &str) -> Option<([u8; PAYLOAD], [u8; TAG])> {
    let bytes = URL_SAFE_NO_PAD.decode(token.strip_prefix(PREFIX)?).ok()?;
    if bytes.len() != PAYLOAD + TAG || bytes[0] != 1 {
        return None;
    }
    Some((
        bytes[..PAYLOAD].try_into().ok()?,
        bytes[PAYLOAD..].try_into().ok()?,
    ))
}
fn grant(payload: &[u8; PAYLOAD]) -> Option<Grant> {
    Some(Grant {
        role: match payload[1] {
            1 => Role::Host,
            2 => Role::Client,
            _ => return None,
        },
        space: payload[2..18].try_into().ok()?,
        expires: u64::from_be_bytes(payload[18..].try_into().ok()?),
    })
}

/// The grant of a token this relay signed; None for anything else. Expiry is the caller's.
pub fn open(key: &hmac::Key, token: &str) -> Option<Grant> {
    let (payload, presented) = parts(token)?;
    // Constant-time comparison of the tag.
    let mut context = hmac::Context::with_key(key);
    context.update(DOMAIN);
    context.update(&payload);
    let expected = context.sign();
    let mut differ = 0u8;
    for (a, b) in expected.as_ref()[..TAG].iter().zip(presented) {
        differ |= a ^ b;
    }
    (differ == 0).then(|| grant(&payload)).flatten()
}

/// When a trial token ends, read without checking its signature: for showing it only.
pub fn expiry(token: &str) -> Option<u64> {
    parts(token.trim()).and_then(|(payload, _)| grant(&payload).map(|g| g.expires))
}

/// What a relay hands out for one trial.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Minted {
    pub host_token: String,
    pub client_token: String,
    pub expires: u64,
}

/// Asks `relay` for a host and a client token of a new trial space.
pub async fn mint(
    relay: &str,
    ca_cert_file: Option<&Path>,
    proxy_url: Option<&str>,
) -> Result<Minted> {
    let mut client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20));
    if let Some(proxy) = proxy_url {
        client = client.proxy(
            reqwest::Proxy::all(proxy)
                .map_err(|_| Diagnostic::err("invalid explicit HTTPS proxy configuration"))?,
        );
    }
    if let Some(path) = ca_cert_file {
        let pem = super::public_file(path, 128 * 1024)?;
        for root in reqwest::Certificate::from_pem_bundle(&pem)
            .map_err(|_| Diagnostic::err("invalid HTTPS CA certificate"))?
        {
            client = client.add_root_certificate(root);
        }
    }
    let client = client
        .build()
        .map_err(|_| Diagnostic::err("HTTPS client configuration failed"))?;
    let response = client
        .post(format!("{}/v1/trial", relay.trim_end_matches('/')))
        .send()
        .await
        .map_err(|_| Diagnostic::err("the relay cannot be reached"))?;
    let status = response.status().as_u16();
    let body = response.bytes().await.unwrap_or_default();
    if body.len() > 4096 {
        return Err(Diagnostic::err("the relay answered with too much data"));
    }
    match status {
        201 => {
            let minted: Minted = serde_json::from_slice(&body)
                .map_err(|_| Diagnostic::err("the relay's trial answer is not readable"))?;
            for token in [&minted.host_token, &minted.client_token] {
                if expiry(token).is_none() {
                    return Err(Diagnostic::err("the relay's trial answer is not readable"));
                }
            }
            Ok(minted)
        }
        404 | 405 => Err(Diagnostic::err(
            "this relay offers no trial access; it needs your own admission tokens (--admission-token-file)",
        )),
        410 => Err(Diagnostic::err(
            "the free trial on this relay has ended; run your own relay (deploy/cloudflare-relay)",
        )),
        429 => Err(Diagnostic::err(
            "too many trial accesses from this address today; try again tomorrow or run your own relay",
        )),
        503 => Err(Diagnostic::err(
            "the trial relay is full right now; try later or run your own relay (deploy/cloudflare-relay)",
        )),
        _ => Err(Diagnostic::err("the relay refused trial access")),
    }
}

/// A date for people, `YYYY-MM-DD` (UTC).
pub fn day(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trial_token_opens_only_with_its_relays_key_and_shows_its_end() {
        let key = key(b"relay secret of at least thirty-two bytes");
        let grant = Grant {
            role: Role::Client,
            space: [7; 16],
            expires: 1_801_008_000,
        };
        let token = seal(&key, &grant);
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
        assert!((32..=256).contains(&token.len()));
        assert_eq!(open(&key, &token), Some(grant));
        assert_eq!(expiry(&token), Some(1_801_008_000));
        assert_eq!(
            open(
                &super::key(b"another relay's secret, also long enough"),
                &token
            ),
            None
        );
        let mut forged = token.clone().into_bytes();
        let last = forged.len() - 3;
        forged[last] = if forged[last] == b'A' { b'B' } else { b'A' };
        assert_eq!(open(&key, std::str::from_utf8(&forged).unwrap()), None);
        assert_eq!(
            open(&key, "a-normal-admission-token-of-forty-three-chars"),
            None
        );
        assert_eq!(day(1_801_008_000), "2027-01-27");
        assert_eq!(day(0), "1970-01-01");
    }
}
