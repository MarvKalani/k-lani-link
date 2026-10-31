use super::*;
use futures_util::StreamExt;
use k_lani_link_protocol::{Chain, HostChains, Initiator, Verdict};

fn server() -> Arc<Relay> {
    Relay::new(
        Limits::default(),
        vec![
            ("a".repeat(43), Limits::default()),
            ("b".repeat(43), Limits::default()),
        ],
    )
    .unwrap()
}
fn paired(relay: &Relay, tenant: usize, seed: u8) -> (String, String, Vec<u8>, Vec<u8>) {
    let chain = Chain::from_bytes([seed; 32]);
    let client =
        Initiator::start(&chain, [seed + 1; 32], [seed + 2; 16], [seed + 3; 24], 1).unwrap();
    let room = client.room();
    let hello = client.hello().to_vec();
    let Verdict::Accepted(host) = HostChains::new(chain)
        .accept(&room, &hello, [seed + 4; 32], [seed + 5; 24], 1)
        .unwrap()
    else {
        panic!()
    };
    let reply = host.reply().to_vec();
    let session = session_id(&hello, &reply);
    assert_eq!(host.session_id(), session);
    relay
        .write_hello(
            tenant,
            &room,
            None,
            Bytes::from(hello.clone()),
            relay.upload(tenant, HELLO_BYTES).unwrap(),
        )
        .unwrap();
    relay
        .write_hello(
            tenant,
            &room,
            Some(&hello_hash(&hello)),
            Bytes::from(reply.clone()),
            relay.upload(tenant, REPLY_BYTES).unwrap(),
        )
        .unwrap();
    (room, session, hello, reply)
}
fn packet(relay: &Relay, tenant: usize, sequence: u64, byte: u8) -> Arc<Packet> {
    let mut bytes = vec![byte; 28];
    bytes[..4].copy_from_slice(b"KLI2");
    bytes[4..12].copy_from_slice(&(sequence - 1).to_be_bytes());
    let mut upload = relay.upload(tenant, MAX_PACKET_BYTES).unwrap();
    upload.bytes.as_mut().unwrap().shrink(bytes.len());
    Arc::new(Packet {
        body: Bytes::from(bytes),
        _bytes: upload.bytes.take().unwrap(),
    })
}
fn headers(tenant: usize) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!(
            "Bearer {}",
            if tenant == 0 {
                "a".repeat(43)
            } else {
                "b".repeat(43)
            }
        )
        .parse()
        .unwrap(),
    );
    headers
}
#[tokio::test]
async fn multi_record_reads_are_contiguous_non_destructive_and_acknowledge_only_offered_prefixes() {
    let relay = server();
    let (room, session, _, _) = paired(&relay, 0, 1);
    let insert = |n| {
        relay
            .store_record(0, &room, &session, 0, n, &packet(&relay, 0, n, 1))
            .unwrap()
    };
    assert!(insert(2));
    assert!(relay.records(0, &room, &session, 0, 1).unwrap().is_empty());
    assert_eq!(
        relay.records(0, &room, &session, 0, 2).err(),
        Some(StatusCode::BAD_REQUEST)
    );
    assert!(insert(1));
    assert!(insert(3));
    assert_eq!(relay.records(0, &room, &session, 0, 1).unwrap().len(), 3);
    assert_eq!(relay.records(0, &room, &session, 0, 1).unwrap().len(), 3);
    assert_eq!(
        relay.records(0, &room, &session, 0, 5).err(),
        Some(StatusCode::BAD_REQUEST)
    );
    assert_eq!(relay.records(0, &room, &session, 0, 3).unwrap().len(), 1);
    assert_eq!(
        relay.records(0, &room, &session, 0, 1).err(),
        Some(StatusCode::GONE)
    );
    assert!(
        relay
            .store_record(0, &room, &session, 0, 1, &packet(&relay, 0, 1, 1))
            .unwrap()
    );
    assert_eq!(
        relay.store_record(0, &room, &session, 0, 1, &packet(&relay, 0, 1, 2)),
        Err(StatusCode::CONFLICT)
    );
    assert_eq!(
        relay.store_record(0, &room, &session, 0, 3, &packet(&relay, 0, 3, 2)),
        Err(StatusCode::CONFLICT)
    );
}
#[tokio::test]
async fn retained_window_keeps_a_slot_for_a_delayed_predecessor_and_retry_hashes_are_bounded() {
    let relay = server();
    let (room, session, _, _) = paired(&relay, 0, 1);
    for n in 2..=WINDOW as u64 {
        assert!(
            relay
                .store_record(0, &room, &session, 0, n, &packet(&relay, 0, n, 1))
                .unwrap()
        );
    }
    assert!(
        !relay
            .store_record(0, &room, &session, 0, 33, &packet(&relay, 0, 33, 1))
            .unwrap()
    );
    assert!(
        relay
            .store_record(0, &room, &session, 0, 1, &packet(&relay, 0, 1, 1))
            .unwrap()
    );
    assert_eq!(
        relay.records(0, &room, &session, 0, 1).unwrap().len(),
        WINDOW
    );
    relay.records(0, &room, &session, 0, 33).unwrap();
    for n in 33..=66 {
        assert!(
            relay
                .store_record(0, &room, &session, 0, n, &packet(&relay, 0, n, 1))
                .unwrap()
        );
        assert_eq!(relay.records(0, &room, &session, 0, n).unwrap().len(), 1);
        relay.records(0, &room, &session, 0, n + 1).unwrap();
    }
    let inner = relay.inner.lock().unwrap();
    assert_eq!(
        inner.rooms[&(0, room.clone())].sessions[&session].channels[0]
            .acknowledged
            .len(),
        RETRY_HASHES
    );
    drop(inner);
    assert_eq!(
        relay.store_record(0, &room, &session, 0, 1, &packet(&relay, 0, 1, 1)),
        Err(StatusCode::GONE)
    );
    assert!(
        relay
            .store_record(0, &room, &session, 0, 66, &packet(&relay, 0, 66, 1))
            .unwrap()
    );
}
#[tokio::test(start_paused = true)]
async fn duplicate_put_does_not_refresh_expiry_and_inflight_payloads_remain_charged() {
    let relay = server();
    let (room, session, _, _) = paired(&relay, 0, 1);
    let bytes = packet(&relay, 0, 1, 1);
    assert!(
        relay
            .store_record(0, &room, &session, 0, 1, &bytes)
            .unwrap()
    );
    let old = relay.records(0, &room, &session, 0, 1).unwrap();
    tokio::time::advance(Duration::from_secs(29)).await;
    assert!(
        relay
            .store_record(0, &room, &session, 0, 1, &bytes)
            .unwrap()
    );
    tokio::time::advance(Duration::from_secs(2)).await;
    relay.sweep();
    assert_eq!(
        relay.records(0, &room, &session, 0, 1).err(),
        Some(StatusCode::GONE)
    );
    assert_eq!(
        relay.store_record(0, &room, &session, 0, 1, &bytes),
        Err(StatusCode::GONE)
    );
    assert_eq!(relay.counts.bytes.load(Ordering::SeqCst), bytes.body.len());
    drop(bytes);
    assert_eq!(relay.counts.bytes.load(Ordering::SeqCst), 28);
    let held = Bytes::from_owner(PacketBody(old[0].clone()));
    drop(old);
    assert_eq!(relay.counts.bytes.load(Ordering::SeqCst), 28);
    drop(held);
    assert_eq!(relay.counts.bytes.load(Ordering::SeqCst), 0);
    tokio::time::advance(Duration::from_secs(SESSION_TTL)).await;
    relay.sweep();
    assert!(relay.inner.lock().unwrap().rooms.is_empty());
    assert_eq!(
        relay.store_record(0, &room, &session, 0, 2, &packet(&relay, 0, 2, 1)),
        Err(StatusCode::GONE)
    );
    assert!(relay.inner.lock().unwrap().rooms.is_empty());
}
#[tokio::test]
async fn admission_reserves_and_releases_global_and_per_token_budgets() {
    let quota = Limits {
        pending_puts: 1,
        pending_put_bytes: MAX_PACKET_BYTES,
        get_waiters: 1,
        bytes: MAX_PACKET_BYTES,
        ..Limits::default()
    };
    let relay = Relay::new(
        Limits {
            pending_puts: 2,
            pending_put_bytes: 2 * MAX_PACKET_BYTES,
            get_waiters: 2,
            bytes: 2 * MAX_PACKET_BYTES,
            ..Limits::default()
        },
        vec![("a".repeat(43), quota.clone()), ("b".repeat(43), quota)],
    )
    .unwrap();
    let first = relay.upload(0, MAX_PACKET_BYTES).unwrap();
    assert_eq!(
        relay.upload(0, 1).err(),
        Some(StatusCode::TOO_MANY_REQUESTS)
    );
    let second = relay.upload(1, MAX_PACKET_BYTES).unwrap();
    assert_eq!(relay.counts.puts.load(Ordering::SeqCst), 2);
    assert_eq!(
        relay.upload(1, 1).err(),
        Some(StatusCode::TOO_MANY_REQUESTS)
    );
    drop(first);
    drop(second);
    for counts in [
        &relay.counts,
        &relay.tenants[0].counts,
        &relay.tenants[1].counts,
    ] {
        assert_eq!(counts.puts.load(Ordering::SeqCst), 0);
        assert_eq!(counts.put_bytes.load(Ordering::SeqCst), 0);
        assert_eq!(counts.bytes.load(Ordering::SeqCst), 0);
    }
    let first = relay.getter(0).unwrap();
    assert_eq!(relay.getter(0).err(), Some(StatusCode::TOO_MANY_REQUESTS));
    let second = relay.getter(1).unwrap();
    drop(first);
    drop(second);
    assert_eq!(relay.counts.gets.load(Ordering::SeqCst), 0);
}

#[test]
fn each_independent_admission_budget_rejects_global_and_per_tenant_exhaustion() {
    for kind in [Charge::Put, Charge::PutBytes, Charge::Bytes, Charge::Get] {
        for global in [true, false] {
            let mut low = Limits::default();
            match kind {
                Charge::Put => low.pending_puts = 2,
                Charge::PutBytes => low.pending_put_bytes = 2,
                Charge::Bytes => low.bytes = 2,
                Charge::Get => low.get_waiters = 2,
            }
            let relay = Relay::new(
                if global {
                    low.clone()
                } else {
                    Limits::default()
                },
                vec![
                    ("a".repeat(43), if global { Limits::default() } else { low }),
                    ("b".repeat(43), Limits::default()),
                ],
            )
            .unwrap();
            let held = relay.charge(0, kind, 2).unwrap();
            assert_eq!(
                relay.charge(if global { 1 } else { 0 }, kind, 1).err(),
                Some(StatusCode::TOO_MANY_REQUESTS)
            );
            if !global {
                drop(relay.charge(1, kind, 2).unwrap());
            }
            drop(held);
            assert_eq!(relay.counts.field(kind).load(Ordering::SeqCst), 0);
            assert_eq!(
                relay.tenants[0].counts.field(kind).load(Ordering::SeqCst),
                0
            );
            drop(relay.charge(0, kind, 1).unwrap());
        }
    }
}
#[tokio::test]
async fn room_session_and_handshake_slot_quotas_do_not_create_rooms_or_refresh_lifetimes() {
    let relay = Relay::new(
        Limits {
            rooms: 2,
            sessions: 2,
            ..Limits::default()
        },
        vec![
            (
                "a".repeat(43),
                Limits {
                    rooms: 1,
                    sessions: 1,
                    ..Limits::default()
                },
            ),
            (
                "b".repeat(43),
                Limits {
                    rooms: 1,
                    sessions: 1,
                    ..Limits::default()
                },
            ),
        ],
    )
    .unwrap();
    let (room, session, _, _) = paired(&relay, 0, 1);
    let (other, _, _, _) = paired(&relay, 1, 1);
    assert_eq!(room, other);
    let inner = relay.inner.lock().unwrap();
    assert_eq!(
        relay.room_capacity(&inner, 0, "not-an-existing-room"),
        Err(StatusCode::TOO_MANY_REQUESTS)
    );
    assert_eq!(
        relay.session_capacity(&inner, 0),
        Err(StatusCode::TOO_MANY_REQUESTS)
    );
    drop(inner);
    assert_eq!(relay.records(1, &room, &session, 0, 1).unwrap().len(), 0);
    let quota = Limits {
        hello_slots: 1,
        ..Limits::default()
    };
    let relay = Relay::new(Limits::default(), vec![("a".repeat(43), quota)]).unwrap();
    let chain = Chain::from_bytes([1; 32]);
    let client = Initiator::start(&chain, [2; 32], [3; 16], [4; 24], 1).unwrap();
    let host = HostChains::new(chain)
        .accept(&client.room(), client.hello(), [5; 32], [6; 24], 1)
        .unwrap();
    let Verdict::Accepted(host) = host else {
        panic!()
    };
    relay
        .write_hello(
            0,
            &client.room(),
            None,
            Bytes::copy_from_slice(client.hello()),
            relay.upload(0, HELLO_BYTES).unwrap(),
        )
        .unwrap();
    assert_eq!(
        relay.write_hello(
            0,
            &client.room(),
            Some(&hello_hash(client.hello())),
            Bytes::copy_from_slice(host.reply()),
            relay.upload(0, REPLY_BYTES).unwrap()
        ),
        Err(StatusCode::TOO_MANY_REQUESTS)
    );
    assert!(
        relay.inner.lock().unwrap().rooms[&(0, client.room())]
            .sessions
            .is_empty()
    );
}
#[tokio::test(start_paused = true)]
async fn request_rate_and_burst_are_enforced_before_state_mutation() {
    let quota = Limits {
        requests_per_second: 1,
        request_burst: 2,
        ..Limits::default()
    };
    let relay = Relay::new(quota.clone(), vec![("a".repeat(43), quota)]).unwrap();
    assert_eq!(
        relay.authorize(&HeaderMap::new(), None),
        Err(StatusCode::UNAUTHORIZED)
    );
    assert_eq!(relay.authorize(&headers(0), None), Ok(0));
    assert_eq!(relay.authorize(&headers(0), None), Ok(0));
    assert_eq!(
        relay.authorize(&headers(0), None),
        Err(StatusCode::TOO_MANY_REQUESTS)
    );
    assert!(relay.inner.lock().unwrap().rooms.is_empty());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(relay.authorize(&headers(0), None), Ok(0));
}
#[tokio::test(start_paused = true)]
async fn blocked_upload_timeout_and_cancellation_release_all_admission_and_direction_slots() {
    let relay = server();
    let (room, session, _, _) = paired(&relay, 0, 1);
    let make = || {
        let relay = relay.clone();
        let room = room.clone();
        let session = session.clone();
        let packet = packet(&relay, 0, 1000, 1).body.clone();
        let request = Request::builder()
            .header("authorization", format!("Bearer {}", "a".repeat(43)))
            .body(Body::from(packet))
            .unwrap();
        tokio::spawn(async move {
            put_record(
                State(relay),
                Path((room, session, "c2h".into(), 1000)),
                request,
            )
            .await
        })
    };
    let mut pending = Vec::new();
    for _ in 0..PIPELINE {
        pending.push(make());
    }
    tokio::task::yield_now().await;
    let denied = make().await.unwrap();
    assert_eq!(denied.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(relay.counts.puts.load(Ordering::SeqCst), PIPELINE);
    let cancelled = pending.pop().unwrap();
    cancelled.abort();
    let _ = cancelled.await;
    assert_eq!(relay.counts.puts.load(Ordering::SeqCst), PIPELINE - 1);
    tokio::time::advance(Duration::from_secs(RECORD_TTL)).await;
    for job in pending {
        assert_eq!(job.await.unwrap().status(), StatusCode::REQUEST_TIMEOUT);
    }
    assert_eq!(relay.counts.puts.load(Ordering::SeqCst), 0);
    assert_eq!(relay.counts.put_bytes.load(Ordering::SeqCst), 0);
    let inner = relay.inner.lock().unwrap();
    let channel = &inner.rooms[&(0, room)].sessions[&session].channels[0];
    assert_eq!(channel.uploads.load(Ordering::SeqCst), 0);
    assert!(channel.records.is_empty());
}
#[tokio::test]
async fn handshake_reads_and_data_namespaces_cannot_cross_tenants_or_sessions() {
    let relay = server();
    let (room, session, hello, reply) = paired(&relay, 0, 1);
    let before = relay.inner.lock().unwrap().rooms[&(0, room.clone())]
        .client
        .as_ref()
        .unwrap()
        .created;
    relay
        .write_hello(
            0,
            &room,
            None,
            Bytes::from(hello.clone()),
            relay.upload(0, HELLO_BYTES).unwrap(),
        )
        .unwrap();
    assert_eq!(
        relay.inner.lock().unwrap().rooms[&(0, room.clone())]
            .client
            .as_ref()
            .unwrap()
            .created,
        before
    );
    assert_eq!(relay.hello(0, &room, None).unwrap().body.as_ref(), hello);
    assert!(relay.hello(1, &room, None).is_none());
    assert_eq!(
        relay.records(1, &room, &session, 0, 1).err(),
        Some(StatusCode::GONE)
    );
    assert_eq!(
        relay
            .records(0, &room, &URL_SAFE_NO_PAD.encode([9; 32]), 0, 1)
            .err(),
        Some(StatusCode::GONE)
    );
    let mut wrong = reply;
    *wrong.last_mut().unwrap() ^= 1;
    assert_eq!(
        relay.write_hello(
            0,
            &room,
            Some(&hello_hash(&hello)),
            Bytes::from(wrong),
            relay.upload(0, REPLY_BYTES).unwrap()
        ),
        Err(StatusCode::CONFLICT)
    );
}

#[tokio::test(start_paused = true)]
async fn background_sweep_expires_payload_and_idle_metadata_without_client_traffic() {
    let relay = server();
    let (room, session, _, _) = paired(&relay, 0, 1);
    assert!(
        relay
            .store_record(0, &room, &session, 0, 1, &packet(&relay, 0, 1, 1))
            .unwrap()
    );
    let stop = CancellationToken::new();
    let router = relay.router(stop.clone());
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(RECORD_TTL + 1)).await;
    tokio::task::yield_now().await;
    assert_eq!(relay.counts.bytes.load(Ordering::SeqCst), 0);
    assert!(
        relay.inner.lock().unwrap().rooms[&(0, room)].sessions[&session].channels[0]
            .records
            .is_empty()
    );
    tokio::time::advance(Duration::from_secs(SESSION_TTL)).await;
    tokio::task::yield_now().await;
    assert!(relay.inner.lock().unwrap().rooms.is_empty());
    stop.cancel();
    drop(router);
}

#[tokio::test]
async fn response_bytes_stay_in_the_budget_after_ack_and_body_cancellation() {
    let relay = server();
    let (room, session, _, _) = paired(&relay, 0, 1);
    let before = relay.counts.bytes.load(Ordering::SeqCst);
    assert!(
        relay
            .store_record(0, &room, &session, 0, 1, &packet(&relay, 0, 1, 1))
            .unwrap()
    );
    let response = get_records(
        State(relay.clone()),
        Path((room.clone(), session.clone(), "c2h".into())),
        Peer(None),
        headers(0),
        Ok(Query(Cursor { from: 1, wait: 0 })),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();
    let prefix = body.next().await.unwrap().unwrap();
    assert_eq!(prefix.as_ref(), 28u32.to_be_bytes());
    let retained_by_network = body.next().await.unwrap().unwrap();
    assert_eq!(relay.counts.gets.load(Ordering::SeqCst), 1);
    drop(body);
    assert_eq!(relay.counts.gets.load(Ordering::SeqCst), 0);
    assert!(relay.records(0, &room, &session, 0, 2).unwrap().is_empty());
    assert_eq!(relay.counts.bytes.load(Ordering::SeqCst), before + 28);
    drop(retained_by_network);
    assert_eq!(relay.counts.bytes.load(Ordering::SeqCst), before);
}

async fn http_server(
    relay: &Arc<Relay>,
) -> (String, CancellationToken, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let router = relay.router(stop.clone());
    let stopping = stop.clone();
    let task = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(stopping.cancelled_owned())
            .await
            .unwrap();
    });
    (base, stop, task)
}
async fn until(mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("bounded resources must be released after disconnect");
}

#[tokio::test]
async fn real_http_requests_preserve_retention_and_return_fixed_status_for_bad_inputs() {
    let relay = server();
    let (room, session, _, _) = paired(&relay, 0, 1);
    let (base, stop, task) = http_server(&relay).await;
    let http = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();
    let path = format!("{base}/v1/box/{room}/{session}/c2h");
    assert_eq!(
        http.get(format!("{path}?from=1&wait=0"))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::UNAUTHORIZED
    );
    for suffix in [
        "?from=0",
        "?from=secret-canary",
        "?from=1&wait=26",
        "?from=1&unknown=secret-canary",
    ] {
        let response = http
            .get(format!("{path}{suffix}"))
            .bearer_auth("a".repeat(43))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response.bytes().await.unwrap().is_empty());
    }
    let response = http
        .put(format!("{path}/1"))
        .bearer_auth("a".repeat(43))
        .body(vec![0; MAX_PACKET_BYTES + 1])
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let wrong = packet(&relay, 0, 2, 1).body.clone();
    assert_eq!(
        http.put(format!("{path}/1"))
            .bearer_auth("a".repeat(43))
            .body(wrong)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    for n in 1..=3 {
        let payload = packet(&relay, 0, n, 1).body.clone();
        assert_eq!(
            http.put(format!("{path}/{n}"))
                .bearer_auth("a".repeat(43))
                .body(payload)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NO_CONTENT
        );
    }
    let first = http
        .get(format!("{path}?from=1&wait=0"))
        .bearer_auth("a".repeat(43))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()["cache-control"], "no-store, no-transform");
    let bytes = first.bytes().await.unwrap();
    assert_eq!(bytes.len(), 3 * 32);
    let repeated = http
        .get(format!("{path}?from=1&wait=0"))
        .bearer_auth("a".repeat(43))
        .send()
        .await
        .unwrap();
    assert_eq!(repeated.bytes().await.unwrap(), bytes);
    assert_eq!(
        http.get(format!("{path}?from=4&wait=0"))
            .bearer_auth("a".repeat(43))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        http.get(format!("{path}?from=1&wait=0"))
            .bearer_auth("a".repeat(43))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::GONE
    );
    assert_eq!(
        http.get(format!("{path}?from=5&wait=0"))
            .bearer_auth("a".repeat(43))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
    stop.cancel();
    task.await.unwrap();
}

#[tokio::test]
async fn real_disconnected_get_and_put_release_waiter_body_and_direction_budgets() {
    use tokio::io::AsyncWriteExt;
    let relay = server();
    let (room, session, _, _) = paired(&relay, 0, 1);
    let before = relay.counts.bytes.load(Ordering::SeqCst);
    let (base, stop, task) = http_server(&relay).await;
    let address = base.strip_prefix("http://").unwrap();
    let mut get = tokio::net::TcpStream::connect(address).await.unwrap();
    get.write_all(format!("GET /v1/box/{room}/{session}/c2h?from=1&wait=25 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\n\r\n","a".repeat(43)).as_bytes()).await.unwrap();
    until(|| relay.counts.gets.load(Ordering::SeqCst) == 1).await;
    drop(get);
    until(|| relay.counts.gets.load(Ordering::SeqCst) == 0).await;
    let mut put = tokio::net::TcpStream::connect(address).await.unwrap();
    let body = packet(&relay, 0, 1000, 1).body.clone();
    put.write_all(format!("PUT /v1/box/{room}/{session}/c2h/1000 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n","a".repeat(43),body.len()).as_bytes()).await.unwrap();
    put.write_all(&body).await.unwrap();
    until(|| relay.counts.puts.load(Ordering::SeqCst) == 1).await;
    drop(put);
    until(|| relay.counts.puts.load(Ordering::SeqCst) == 0).await;
    assert_eq!(relay.counts.put_bytes.load(Ordering::SeqCst), 0);
    assert_eq!(relay.counts.bytes.load(Ordering::SeqCst), before);
    let mut stalled = tokio::net::TcpStream::connect(address).await.unwrap();
    stalled.write_all(format!("PUT /v1/box/{room}/{session}/c2h/1 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Length: 100\r\n\r\n","a".repeat(43)).as_bytes()).await.unwrap();
    until(|| relay.counts.puts.load(Ordering::SeqCst) == 1).await;
    drop(stalled);
    until(|| relay.counts.puts.load(Ordering::SeqCst) == 0).await;
    assert_eq!(relay.counts.bytes.load(Ordering::SeqCst), before);
    stop.cancel();
    task.await.unwrap();
}
#[tokio::test]
async fn a_stored_record_wakes_only_waiters_of_its_own_direction() {
    use futures_util::FutureExt;
    let relay = server();
    let (room_a, session_a, _, _) = paired(&relay, 0, 1);
    let (room_b, session_b, _, _) = paired(&relay, 0, 11);
    {
        let other = relay.waiter(Wake::Channel(0, room_b.clone(), session_b.clone(), 0));
        let reverse = relay.waiter(Wake::Channel(0, room_a.clone(), session_a.clone(), 1));
        let own = relay.waiter(Wake::Channel(0, room_a.clone(), session_a.clone(), 0));
        let (other_wait, reverse_wait, own_wait) =
            (other.notified(), reverse.notified(), own.notified());
        tokio::pin!(other_wait, reverse_wait, own_wait);
        for wait in [
            other_wait.as_mut(),
            reverse_wait.as_mut(),
            own_wait.as_mut(),
        ] {
            wait.enable();
        }
        assert!(
            relay
                .store_record(0, &room_a, &session_a, 0, 1, &packet(&relay, 0, 1, 1))
                .unwrap()
        );
        assert!(own_wait.now_or_never().is_some());
        assert!(other_wait.as_mut().now_or_never().is_none());
        assert!(reverse_wait.as_mut().now_or_never().is_none());
        assert!(
            relay
                .store_record(0, &room_b, &session_b, 0, 1, &packet(&relay, 0, 1, 1))
                .unwrap()
        );
        assert!(other_wait.now_or_never().is_some());
    }
    relay.sweep();
    assert!(relay.inner.lock().unwrap().wakes.is_empty());
}
#[test]
fn only_admission_refusals_carry_a_retry_hint() {
    let relay = server();
    let refused = relay.response(StatusCode::TOO_MANY_REQUESTS, Body::empty());
    assert_eq!(refused.headers()["retry-after"], "1");
    let timed_out = relay.response(StatusCode::REQUEST_TIMEOUT, Body::empty());
    assert!(timed_out.headers().get("retry-after").is_none());
}

fn admit(byte: char, space: Option<&str>, access: Access) -> Admit {
    Admit {
        key: byte.to_string().repeat(43),
        space: space.map(Into::into),
        limits: Some(Limits::default()),
        access,
    }
}
fn bearer(byte: char) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        "authorization",
        format!("Bearer {}", byte.to_string().repeat(43))
            .parse()
            .unwrap(),
    );
    headers
}
fn ip(text: &str) -> Option<IpAddr> {
    Some(text.parse().unwrap())
}
#[tokio::test]
async fn tokens_of_one_space_share_rooms_and_other_spaces_stay_apart() {
    use tower::ServiceExt;
    let relay = Relay::with_access(
        Limits::default(),
        vec![
            admit('a', Some("team"), Access::default()),
            admit('b', Some("team"), Access::default()),
            admit('c', None, Access::default()),
        ],
        Brake::default(),
    )
    .unwrap();
    assert_eq!(relay.tenants.len(), 2);
    let router = relay.router(CancellationToken::new());
    let client =
        Initiator::start(&Chain::from_bytes([9; 32]), [1; 32], [2; 16], [3; 24], 1).unwrap();
    let room = client.room();
    let request = |method: &str, token: char, body: Vec<u8>| {
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(format!("/v1/hello/{room}/client?wait=0"))
            .body(Body::from(body))
            .unwrap();
        *request.headers_mut() = bearer(token);
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 1], 4000))));
        request
    };
    let stored = router
        .clone()
        .oneshot(request("PUT", 'a', client.hello().to_vec()))
        .await
        .unwrap();
    assert_eq!(stored.status(), StatusCode::NO_CONTENT);
    let shared = router
        .clone()
        .oneshot(request("GET", 'b', vec![]))
        .await
        .unwrap();
    assert_eq!(shared.status(), StatusCode::OK);
    let separate = router.oneshot(request("GET", 'c', vec![])).await.unwrap();
    assert_eq!(separate.status(), StatusCode::NO_CONTENT);
}
#[test]
fn tokens_of_one_space_must_have_identical_limits() {
    let mut other = admit('b', Some("team"), Access::default());
    other.limits.as_mut().unwrap().rooms = 3;
    let mut inherits = admit('c', Some("team"), Access::default());
    inherits.limits = None;
    let mut first = admit('a', Some("team"), Access::default());
    first.limits.as_mut().unwrap().rooms = 3;
    let relay =
        Relay::with_access(Limits::default(), vec![first, inherits], Brake::default()).unwrap();
    assert_eq!(relay.tenants.len(), 1);
    assert_eq!(relay.tenants[0].limits.rooms, 3);
    assert!(
        Relay::with_access(
            Limits::default(),
            vec![admit('a', Some("team"), Access::default()), other],
            Brake::default(),
        )
        .is_err()
    );
}
#[tokio::test]
async fn an_expired_token_is_refused() {
    let relay = Relay::with_access(
        Limits::default(),
        vec![
            admit(
                'a',
                None,
                Access {
                    expires: Some(unix_now()),
                    ..Access::default()
                },
            ),
            admit(
                'b',
                None,
                Access {
                    expires: Some(unix_now() + 3600),
                    ..Access::default()
                },
            ),
        ],
        Brake::default(),
    )
    .unwrap();
    assert_eq!(
        relay.authorize(&bearer('a'), None),
        Err(StatusCode::UNAUTHORIZED)
    );
    assert_eq!(relay.authorize(&bearer('b'), None), Ok(1));
}
#[tokio::test]
async fn address_networks_admit_only_matching_peers_and_need_a_known_peer() {
    let relay = Relay::with_access(
        Limits::default(),
        vec![
            admit(
                'a',
                None,
                Access {
                    allow_ips: vec![
                        "192.0.2.0/24".parse().unwrap(),
                        "2001:db8::/48".parse().unwrap(),
                    ],
                    ..Access::default()
                },
            ),
            admit('b', None, Access::default()),
        ],
        Brake::default(),
    )
    .unwrap();
    assert_eq!(relay.authorize(&bearer('a'), ip("192.0.2.77")), Ok(0));
    assert_eq!(
        relay.authorize(&bearer('a'), ip("::ffff:192.0.2.77")),
        Ok(0)
    );
    assert_eq!(relay.authorize(&bearer('a'), ip("2001:db8:0:5::1")), Ok(0));
    assert_eq!(
        relay.authorize(&bearer('a'), ip("198.51.100.1")),
        Err(StatusCode::UNAUTHORIZED)
    );
    assert_eq!(
        relay.authorize(&bearer('a'), None),
        Err(StatusCode::UNAUTHORIZED)
    );
    assert_eq!(relay.authorize(&bearer('b'), None), Ok(1));
}
#[tokio::test(start_paused = true)]
async fn addresses_per_day_refuse_new_peers_but_keep_known_ones() {
    let relay = Relay::with_access(
        Limits::default(),
        vec![admit(
            'a',
            None,
            Access {
                max_ips_per_day: 2,
                ..Access::default()
            },
        )],
        Brake::default(),
    )
    .unwrap();
    assert_eq!(relay.authorize(&bearer('a'), ip("192.0.2.1")), Ok(0));
    assert_eq!(relay.authorize(&bearer('a'), ip("2001:db8:0:1::1")), Ok(0));
    // The same IPv6 /64 is one subscriber.
    assert_eq!(relay.authorize(&bearer('a'), ip("2001:db8:0:1::99")), Ok(0));
    assert_eq!(
        relay.authorize(&bearer('a'), ip("192.0.2.2")),
        Err(StatusCode::UNAUTHORIZED)
    );
    assert_eq!(relay.authorize(&bearer('a'), ip("192.0.2.1")), Ok(0));
    assert_eq!(
        relay.authorize(&bearer('a'), None),
        Err(StatusCode::UNAUTHORIZED)
    );
    // No address is kept in the clear.
    assert!(relay.inner.lock().unwrap().seen[0].len() == 2);
    tokio::time::advance(DAY).await;
    assert_eq!(relay.authorize(&bearer('a'), ip("192.0.2.2")), Ok(0));
}
#[tokio::test(start_paused = true)]
async fn the_failure_brake_bans_an_address_but_never_a_valid_token() {
    let relay = Relay::with_access(
        Limits::default(),
        vec![admit('a', None, Access::default())],
        Brake {
            failures_per_minute: 3,
            ban: Duration::from_secs(600),
        },
    )
    .unwrap();
    let wrong = bearer('z');
    for _ in 0..3 {
        assert_eq!(
            relay.authorize(&wrong, ip("192.0.2.9")),
            Err(StatusCode::UNAUTHORIZED)
        );
    }
    assert_eq!(
        relay.authorize(&wrong, ip("192.0.2.9")),
        Err(StatusCode::TOO_MANY_REQUESTS)
    );
    assert_eq!(
        relay.authorize(&wrong, ip("192.0.2.10")),
        Err(StatusCode::UNAUTHORIZED)
    );
    assert_eq!(relay.authorize(&bearer('a'), ip("192.0.2.9")), Ok(0));
    // Without a known address there is nothing to count.
    for _ in 0..5 {
        assert_eq!(relay.authorize(&wrong, None), Err(StatusCode::UNAUTHORIZED));
    }
    tokio::time::advance(Duration::from_secs(599)).await;
    relay.sweep();
    assert_eq!(
        relay.authorize(&wrong, ip("192.0.2.9")),
        Err(StatusCode::TOO_MANY_REQUESTS)
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    assert_eq!(
        relay.authorize(&wrong, ip("192.0.2.9")),
        Err(StatusCode::UNAUTHORIZED)
    );
}
#[tokio::test]
async fn the_relay_listener_passes_the_peer_address_to_the_access_rules() {
    let only = |net: &str| Access {
        allow_ips: vec![net.parse().unwrap()],
        ..Access::default()
    };
    let relay = Relay::with_access(
        Limits::default(),
        vec![
            admit('a', None, only("127.0.0.0/8")),
            admit('b', None, only("192.0.2.0/24")),
        ],
        Brake::default(),
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let task = tokio::spawn(super::super::server::serve_router(
        listener,
        relay.router(stop.clone()),
        None,
        8,
        false,
        stop.clone(),
    ));
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let room = URL_SAFE_NO_PAD.encode([4u8; 16]);
    let status = |token: char| {
        http.get(format!("{base}/v1/hello/{room}/client?wait=0"))
            .bearer_auth(token.to_string().repeat(43))
            .send()
    };
    assert_eq!(status('a').await.unwrap().status(), StatusCode::NO_CONTENT);
    assert_eq!(
        status('b').await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    stop.cancel();
    task.await.unwrap().unwrap();
}

async fn listener_with_peers(
    relay: &Arc<Relay>,
) -> (
    String,
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let stop = CancellationToken::new();
    let task = tokio::spawn(super::super::server::serve_router(
        listener,
        relay.router(stop.clone()),
        None,
        8,
        false,
        stop.clone(),
    ));
    (base, stop, task)
}

#[tokio::test]
async fn a_pairing_slot_carries_one_exchange_and_the_client_side_needs_no_token() {
    let relay = server();
    let (base, stop, task) = listener_with_peers(&relay).await;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let host = |method: reqwest::Method, path: &str| {
        http.request(method, format!("{base}{path}"))
            .bearer_auth("a".repeat(43))
    };
    let open = host(reqwest::Method::POST, "/v1/pair")
        .body(vec![1u8; 33])
        .send()
        .await
        .unwrap();
    assert_eq!(open.status(), StatusCode::CREATED);
    let number = open.text().await.unwrap();
    assert!(slot_number_for_tests(&number));
    let get = |part: &str| http.get(format!("{base}/v1/pair/{number}/{part}")).send();
    let put = |part: &str, body: Vec<u8>| {
        http.put(format!("{base}/v1/pair/{number}/{part}"))
            .body(body)
            .send()
    };
    let opening = get("a").await.unwrap();
    assert_eq!(opening.status(), StatusCode::OK);
    assert_eq!(opening.bytes().await.unwrap().as_ref(), &[1u8; 33]);
    // The client may not skip ahead, and the host's parts need the token.
    assert_eq!(
        put("c", vec![3]).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(get("c").await.unwrap().status(), StatusCode::NO_CONTENT);
    assert_eq!(
        put("b", vec![2u8; 60]).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    // One attempt per slot: a second answer and a late opening read both miss.
    assert_eq!(
        put("b", vec![9u8; 60]).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(get("a").await.unwrap().status(), StatusCode::NOT_FOUND);
    assert_eq!(get("b").await.unwrap().status(), StatusCode::UNAUTHORIZED);
    let answer = host(reqwest::Method::GET, &format!("/v1/pair/{number}/b?wait=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(answer.bytes().await.unwrap().as_ref(), &[2u8; 60]);
    let sealed = host(reqwest::Method::PUT, &format!("/v1/pair/{number}/c"))
        .body(vec![3u8; 500])
        .send()
        .await
        .unwrap();
    assert_eq!(sealed.status(), StatusCode::NO_CONTENT);
    assert_eq!(get("c").await.unwrap().bytes().await.unwrap().len(), 500);
    assert_eq!(
        put("d", vec![4u8; 40]).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    let ack = host(reqwest::Method::GET, &format!("/v1/pair/{number}/d?wait=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(ack.bytes().await.unwrap().as_ref(), &[4u8; 40]);
    // A lost answer can be read again; the slot stays until the host closes it.
    let again = host(reqwest::Method::GET, &format!("/v1/pair/{number}/d?wait=0"))
        .send()
        .await
        .unwrap();
    assert_eq!(again.bytes().await.unwrap().as_ref(), &[4u8; 40]);
    let closed = host(reqwest::Method::DELETE, &format!("/v1/pair/{number}"))
        .send()
        .await
        .unwrap();
    assert_eq!(closed.status(), StatusCode::NO_CONTENT);
    assert_eq!(get("c").await.unwrap().status(), StatusCode::NOT_FOUND);
    assert!(relay.inner.lock().unwrap().slots.is_empty());
    stop.cancel();
    task.await.unwrap().unwrap();
}
fn slot_number_for_tests(text: &str) -> bool {
    text.len() == 3 && text.parse::<u16>().is_ok_and(|n| (100..=999).contains(&n))
}

#[tokio::test]
async fn without_a_token_only_the_client_side_of_a_slot_answers() {
    let relay = server();
    let number = relay.open_slot(0, Bytes::from_static(&[1; 33])).unwrap();
    let (base, stop, task) = listener_with_peers(&relay).await;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let room = URL_SAFE_NO_PAD.encode([4u8; 16]);
    for (method, path) in [
        (reqwest::Method::POST, "/v1/pair".to_string()),
        (reqwest::Method::DELETE, format!("/v1/pair/{number}")),
        (reqwest::Method::GET, format!("/v1/pair/{number}/b")),
        (reqwest::Method::GET, format!("/v1/pair/{number}/d")),
        (reqwest::Method::PUT, format!("/v1/pair/{number}/c")),
        (
            reqwest::Method::GET,
            format!("/v1/hello/{room}/client?wait=0"),
        ),
        (reqwest::Method::PUT, format!("/v1/hello/{room}/client")),
        (
            reqwest::Method::GET,
            format!("/v1/box/{room}/{room}/c2h?from=1&wait=0"),
        ),
    ] {
        let status = http
            .request(method.clone(), format!("{base}{path}"))
            .body(vec![1u8; 33])
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
    // Another space's token cannot read or close the slot.
    let other = |method: reqwest::Method, path: String| {
        http.request(method, format!("{base}{path}"))
            .bearer_auth("b".repeat(43))
            .send()
    };
    assert_eq!(
        other(reqwest::Method::GET, format!("/v1/pair/{number}/b?wait=0"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        other(reqwest::Method::DELETE, format!("/v1/pair/{number}"))
            .await
            .unwrap()
            .status(),
        StatusCode::NOT_FOUND
    );
    assert!(relay.inner.lock().unwrap().slots.contains_key(&number));
    stop.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn pairing_slots_are_bounded_per_space_expire_and_can_be_closed() {
    let slots = |pair_slots| Limits {
        pair_slots: Some(pair_slots),
        ..Limits::default()
    };
    let relay = Relay::new(
        slots(6),
        vec![("a".repeat(43), slots(4)), ("b".repeat(43), slots(4))],
    )
    .unwrap();
    let opening = || Bytes::from_static(&[1; 33]);
    let numbers: Vec<u16> = (0..4)
        .map(|_| relay.open_slot(0, opening()).unwrap())
        .collect();
    assert_eq!(
        relay.open_slot(0, opening()),
        Err(StatusCode::TOO_MANY_REQUESTS)
    );
    // Two more fill the relay: the other space is then refused below its own bound.
    let others: Vec<u16> = (0..2)
        .map(|_| relay.open_slot(1, opening()).unwrap())
        .collect();
    assert_eq!(
        relay.open_slot(1, opening()),
        Err(StatusCode::TOO_MANY_REQUESTS)
    );
    relay.close_slot(others[1], 1).unwrap();
    assert_eq!(
        relay.open_slot(0, Bytes::new()),
        Err(StatusCode::BAD_REQUEST)
    );
    assert_eq!(
        relay.open_slot(1, Bytes::from(vec![1u8; 65])),
        Err(StatusCode::BAD_REQUEST)
    );
    assert_eq!(
        relay.write_part(numbers[0], 1, None, Bytes::from(vec![2u8; 129])),
        Err(StatusCode::BAD_REQUEST)
    );
    assert_eq!(relay.close_slot(numbers[0], 1), Err(StatusCode::NOT_FOUND));
    relay.close_slot(numbers[0], 0).unwrap();
    assert_eq!(
        relay.read_part(numbers[0], 0, None),
        Err(StatusCode::NOT_FOUND)
    );
    tokio::time::advance(slots::PAIR_TTL).await;
    assert_eq!(
        relay.read_part(numbers[1], 0, None),
        Err(StatusCode::NOT_FOUND)
    );
    assert_eq!(
        relay.write_part(numbers[2], 1, None, Bytes::from_static(&[2])),
        Err(StatusCode::NOT_FOUND)
    );
    relay.sweep();
    assert!(relay.inner.lock().unwrap().slots.is_empty());
    assert!(relay.open_slot(0, opening()).is_ok());
}

#[tokio::test]
async fn pair_slots_are_configurable_and_zero_turns_pairing_by_code_off() {
    let slots = |pair_slots| Limits {
        pair_slots: Some(pair_slots),
        ..Limits::default()
    };
    let opening = || Bytes::from_static(&[1; 33]);
    // Without the key: four per space.
    let relay = server();
    for _ in 0..4 {
        relay.open_slot(0, opening()).unwrap();
    }
    assert_eq!(
        relay.open_slot(0, opening()),
        Err(StatusCode::TOO_MANY_REQUESTS)
    );
    // Off for one space only: the other space still pairs.
    let relay = Relay::new(
        Limits::default(),
        vec![("a".repeat(43), slots(0)), ("b".repeat(43), slots(1))],
    )
    .unwrap();
    assert_eq!(relay.open_slot(0, opening()), Err(StatusCode::FORBIDDEN));
    assert!(relay.open_slot(1, opening()).is_ok());
    assert_eq!(
        relay.open_slot(1, opening()),
        Err(StatusCode::TOO_MANY_REQUESTS)
    );
    // Off for the whole relay, whatever a space allows.
    let relay = Relay::new(slots(0), vec![("a".repeat(43), slots(4))]).unwrap();
    assert_eq!(relay.open_slot(0, opening()), Err(StatusCode::FORBIDDEN));
    // The whole number range can be used, but no more.
    let relay = Relay::new(slots(900), vec![("a".repeat(43), slots(900))]).unwrap();
    for _ in 0..900 {
        relay.open_slot(0, opening()).unwrap();
    }
    assert_eq!(
        relay.open_slot(0, opening()),
        Err(StatusCode::TOO_MANY_REQUESTS)
    );
    assert!(Relay::new(slots(901), vec![("a".repeat(43), slots(4))]).is_err());
    assert!(Relay::new(slots(4), vec![("a".repeat(43), slots(901))]).is_err());
}

#[tokio::test]
async fn guessing_slot_numbers_without_a_token_hits_the_failure_brake() {
    let relay = Relay::with_access(
        Limits::default(),
        vec![admit('a', None, Access::default())],
        Brake {
            failures_per_minute: 3,
            ban: Duration::from_secs(600),
        },
    )
    .unwrap();
    let number = relay.open_slot(0, Bytes::from_static(&[1; 33])).unwrap();
    let (base, stop, task) = listener_with_peers(&relay).await;
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let get = |n: &str| http.get(format!("{base}/v1/pair/{n}/a")).send();
    // A good read is not a strike.
    for _ in 0..5 {
        assert_eq!(
            get(&number.to_string()).await.unwrap().status(),
            StatusCode::OK
        );
    }
    let wrong = if number == 999 { 998 } else { number + 1 };
    for _ in 0..3 {
        assert_eq!(
            get(&wrong.to_string()).await.unwrap().status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        get("abc").await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    // Once banned, even the real slot answers 429 to that address.
    assert_eq!(
        get(&number.to_string()).await.unwrap().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    stop.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test(start_paused = true)]
async fn requests_without_a_token_cannot_use_up_the_tunnels_budget() {
    let relay = server();
    let mut refused = 0;
    for _ in 0..200 {
        if relay.open_door(None).is_err() {
            refused += 1;
        }
    }
    assert!(
        refused >= 150,
        "the tokenless budget must be small: {refused}"
    );
    // Every paired tunnel still gets its requests.
    for tenant in 0..2 {
        assert_eq!(relay.authorize(&headers(tenant), None), Ok(tenant));
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(relay.open_door(None).is_ok());
}
