//! Desired-behavior regressions for the three original state counterexamples.
//! Synthetic credentials only; no relay, transport or production wiring.
use k_lani_link::{
    chain_state::{Lifetimes, MISSING, REVOKED, Store},
    config::random_id,
};
use k_lani_link_protocol::{Accepted, Chain, EndpointRole, Frame, HelloError, Initiator, Verdict};
use std::path::{Path, PathBuf};
const NOW: u64 = 1_000_000;
fn scratch() -> PathBuf {
    std::env::temp_dir().join(format!("link-regression-{}", random_id().unwrap()))
}
fn initial() -> Chain {
    Chain::from_bytes([1; 32])
}
fn start(chain: &Chain, seed: u8) -> Initiator {
    Initiator::start(chain, [seed; 32], [seed; 16], [seed; 24], NOW).unwrap()
}
fn issued(dir: &Path) -> (Store, Store) {
    let mut host = Store::open(dir, EndpointRole::Host).unwrap();
    let mut client = Store::open(dir, EndpointRole::Client).unwrap();
    host.initialize(&initial(), NOW, Lifetimes::default())
        .unwrap();
    client
        .initialize(&initial(), NOW, Lifetimes::default())
        .unwrap();
    (host, client)
}
fn accept(host: &mut Store, hello: &Initiator, seed: u8) -> Accepted {
    let Verdict::Accepted(a) = host
        .accept_host(&hello.room(), hello.hello(), [seed; 32], [seed; 24], NOW)
        .unwrap()
    else {
        panic!("expected new attempt");
    };
    a
}
fn advance(host: &mut Store, client: &mut Store, seed: u8) {
    let hello = start(&client.client(NOW).unwrap().chain, seed);
    let mut a = accept(host, &hello, seed + 1);
    let mut c = hello.finish(a.reply(), NOW).unwrap();
    client.commit_client(&c, NOW).unwrap();
    host.confirm_host(&mut a, &c.session.seal_batch(&[Frame::Ready]).unwrap(), NOW)
        .unwrap();
}
#[test]
fn replay_after_host_restart_preserves_the_clients_durable_successor() {
    let dir = scratch();
    let (mut host, mut client) = issued(&dir);
    let hello = start(&initial(), 7);
    let room = hello.room();
    let wire = hello.hello().to_vec();
    let attempt = accept(&mut host, &hello, 9);
    let reply = attempt.reply().to_vec();
    let old_session = attempt.session_id().to_owned();
    let established = hello.finish(&reply, NOW).unwrap();
    client.commit_client(&established, NOW).unwrap();
    // Host process loses its volatile session before Ready. Client keeps its saved successor.
    drop(attempt);
    drop(host);
    let mut host = Store::open(&dir, EndpointRole::Host).unwrap();
    let Verdict::CachedReply(cached) = host
        .accept_host(&room, &wire, [11; 32], [11; 24], NOW)
        .unwrap()
    else {
        panic!("replay must return only stored bytes");
    };
    assert_eq!(cached, reply);
    let durable = client.client(NOW).unwrap().chain;
    assert!(
        host.host(NOW)
            .unwrap()
            .chains
            .rooms()
            .contains(&durable.room())
    );
    assert_eq!(host.host(NOW).unwrap().chains.pending().len(), 1);
    let fresh = start(&durable, 13);
    let mut accepted = accept(&mut host, &fresh, 15);
    assert_ne!(accepted.session_id(), old_session);
    let mut established = fresh.finish(accepted.reply(), NOW).unwrap();
    client.commit_client(&established, NOW).unwrap();
    host.confirm_host(
        &mut accepted,
        &established.session.seal_batch(&[Frame::Ready]).unwrap(),
        NOW,
    )
    .unwrap();
    assert!(
        host.host(NOW).unwrap().chains.current().as_bytes()
            == client.client(NOW).unwrap().chain.as_bytes()
    );
    // The known confirmed replay cannot raise a false stale/copy alarm.
    assert!(matches!(
        host.accept_host(&room, &wire, [17; 32], [17; 24], NOW)
            .unwrap(),
        Verdict::Ignored
    ));
    drop(host);
    drop(client);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn deleting_or_revoking_host_state_never_reenables_the_bootstrap_key() {
    let dir = scratch();
    let (mut host, mut client) = issued(&dir);
    advance(&mut host, &mut client, 3);
    let old = start(&initial(), 7);
    drop(host);
    std::fs::remove_file(dir.join("host.chain")).unwrap();
    let mut host = Store::open(&dir, EndpointRole::Host).unwrap();
    assert_eq!(host.host(NOW).err().unwrap().to_string(), MISSING);
    assert_eq!(
        host.accept_host(&old.room(), old.hello(), [9; 32], [9; 24], NOW)
            .err()
            .unwrap()
            .to_string(),
        MISSING
    );
    assert!(!dir.join("host.chain").exists());
    host.revoke().unwrap();
    drop(host);
    let mut host = Store::open(&dir, EndpointRole::Host).unwrap();
    assert_eq!(host.host(NOW).err().unwrap().to_string(), REVOKED);
    assert!(
        host.initialize(&initial(), NOW, Lifetimes::default())
            .is_err()
    );
    drop(host);
    drop(client);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn a_two_generations_old_credential_gets_the_bounded_stale_warning_after_restart() {
    let dir = scratch();
    let (mut host, mut client) = issued(&dir);
    advance(&mut host, &mut client, 3);
    advance(&mut host, &mut client, 7);
    drop(host);
    let mut host = Store::open(&dir, EndpointRole::Host).unwrap();
    let old = start(&initial(), 11);
    assert!(host.host(NOW).unwrap().chains.rooms().contains(&old.room()));
    let Verdict::Stale(reply) = host
        .accept_host(&old.room(), old.hello(), [13; 32], [13; 24], NOW)
        .unwrap()
    else {
        panic!("expected stale warning");
    };
    assert_eq!(old.finish(&reply, NOW).err(), Some(HelloError::Stale));
    assert!(HelloError::Stale.to_string().contains("possibly copied"));
    drop(host);
    drop(client);
    std::fs::remove_dir_all(dir).unwrap();
}
