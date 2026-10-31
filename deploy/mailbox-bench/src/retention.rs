//! Volatile benchmark delivery state. Not endpoint credentials or production code.
use super::*;
use axum::extract::Query;
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
pub struct Cursor {
    pub from: u64,
}
pub fn index(packet: &[u8]) -> Result<u64, StatusCode> {
    if !(28..=MAX_PACKET_BYTES).contains(&packet.len()) || &packet[..4] != b"KLI2" {
        return Err(StatusCode::BAD_REQUEST);
    }
    // The existing authenticated record header is zero-based; mailbox addresses start at one.
    u64::from_be_bytes(packet[4..12].try_into().unwrap())
        .checked_add(1)
        .ok_or(StatusCode::BAD_REQUEST)
}
pub fn expire(app: &App, queue: &Queue) {
    let mut records = queue.records.lock().unwrap();
    records.retain(|seq, (created, bytes)| {
        if created.elapsed() < Duration::from_secs(30) {
            return true;
        }
        app.buffered.fetch_sub(bytes.len(), Relaxed);
        queue.floor.fetch_max(seq.saturating_add(1), Relaxed);
        false
    });
}
pub fn insert(app: &App, queue: &Queue, seq: u64, body: Bytes) -> StatusCode {
    if index(&body) != Ok(seq) {
        return StatusCode::BAD_REQUEST;
    }
    expire(app, queue);
    let mut records = queue.records.lock().unwrap();
    if seq
        < queue
            .acknowledged
            .load(Relaxed)
            .max(queue.floor.load(Relaxed))
    {
        return match queue.tombstones.lock().unwrap().get(&seq) {
            Some(old) if *old == <[u8; 32]>::from(Sha256::digest(&body)) => StatusCode::NO_CONTENT,
            Some(_) => StatusCode::CONFLICT,
            None => StatusCode::GONE,
        };
    }
    if let Some((_, old)) = records.get(&seq) {
        return if old == &body {
            StatusCode::NO_CONTENT
        } else {
            StatusCode::CONFLICT
        };
    }
    if records.len() >= CAP
        || app
            .buffered
            .fetch_update(Relaxed, Relaxed, |n| {
                n.checked_add(body.len()).filter(|n| *n <= MAX_BUFFERED)
            })
            .is_err()
    {
        app.rejected.fetch_add(1, Relaxed);
        return StatusCode::TOO_MANY_REQUESTS;
    }
    records.insert(seq, (Instant::now(), body));
    drop(records);
    queue.notify.notify_waiters();
    queue.notify.notify_one();
    StatusCode::NO_CONTENT
}
pub fn acknowledge(app: &App, queue: &Queue, from: u64) -> Result<(), StatusCode> {
    expire(app, queue);
    let mut records = queue.records.lock().unwrap();
    if from
        < queue
            .floor
            .load(Relaxed)
            .max(queue.acknowledged.load(Relaxed))
    {
        return Err(StatusCode::GONE);
    }
    if from == 0 || from > queue.offered.load(Relaxed).saturating_add(1) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut tombstones = queue.tombstones.lock().unwrap();
    while let Some((&seq, _)) = records.first_key_value() {
        if seq >= from {
            break;
        }
        let (_, (_, body)) = records.pop_first().unwrap();
        app.buffered.fetch_sub(body.len(), Relaxed);
        tombstones.insert(seq, Sha256::digest(&body).into());
    }
    while tombstones.len() > CAP * 2 {
        tombstones.pop_first();
    }
    queue.acknowledged.store(from, Relaxed);
    drop(tombstones);
    drop(records);
    queue.notify.notify_waiters();
    queue.notify.notify_one();
    Ok(())
}
pub async fn insert_wait(app: &App, queue: &Queue, seq: u64, body: Bytes) -> StatusCode {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let status = insert(app, queue, seq, body.clone());
        if status != StatusCode::TOO_MANY_REQUESTS {
            return status;
        }
        if tokio::time::timeout_at(deadline, queue.notify.notified())
            .await
            .is_err()
        {
            return StatusCode::REQUEST_TIMEOUT;
        }
    }
}
pub fn read(
    app: &App,
    queue: &Queue,
    next: &mut u64,
    destructive: bool,
) -> Result<Vec<Bytes>, StatusCode> {
    expire(app, queue);
    let mut records = queue.records.lock().unwrap();
    if *next
        < queue
            .floor
            .load(Relaxed)
            .max(queue.acknowledged.load(Relaxed))
    {
        return Err(StatusCode::GONE);
    }
    let mut output = Vec::new();
    for _ in 0..CAP {
        let Some((_, body)) = records.get(next) else {
            break;
        };
        output.push(body.clone());
        if destructive {
            let (_, body) = records.remove(next).unwrap();
            app.buffered.fetch_sub(body.len(), Relaxed);
            queue.floor.store(next.saturating_add(1), Relaxed);
        }
        queue.offered.fetch_max(*next, Relaxed);
        *next += 1;
        app.taken.fetch_add(1, Relaxed);
    }
    Ok(output)
}
pub async fn ack(
    State(app): State<Arc<App>>,
    Path(room): Path<String>,
    Query(cursor): Query<Cursor>,
    headers: HeaderMap,
) -> StatusCode {
    if !authorized(&headers) {
        return StatusCode::UNAUTHORIZED;
    }
    app.acks.fetch_add(1, Relaxed);
    let Some(queue) = app.queue(room) else {
        return StatusCode::TOO_MANY_REQUESTS;
    };
    acknowledge(&app, &queue, cursor.from).map_or_else(|status| status, |()| StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(seq: u64, byte: u8) -> Bytes {
        let mut value = vec![byte; 28];
        value[..4].copy_from_slice(b"KLI2");
        value[4..12].copy_from_slice(&(seq - 1).to_be_bytes());
        Bytes::from(value)
    }
    #[test]
    fn lost_response_can_be_retrieved_again_until_the_next_cursor_acknowledges_it() {
        let app = App::default();
        let q = app.queue("test".into()).unwrap();
        for seq in 1..=3 {
            assert_eq!(
                insert(&app, &q, seq, packet(seq, 1)),
                StatusCode::NO_CONTENT
            );
        }
        assert_eq!(read(&app, &q, &mut 1, false).unwrap().len(), 3);
        assert_eq!(read(&app, &q, &mut 1, false).unwrap().len(), 3);
        acknowledge(&app, &q, 3).unwrap();
        assert_eq!(read(&app, &q, &mut 3, false).unwrap().len(), 1);
        assert_eq!(read(&app, &q, &mut 1, false).unwrap_err(), StatusCode::GONE);
        assert_eq!(insert(&app, &q, 1, packet(1, 1)), StatusCode::NO_CONTENT);
        assert_eq!(insert(&app, &q, 1, packet(1, 2)), StatusCode::CONFLICT);
        assert_eq!(
            acknowledge(&app, &q, 5).unwrap_err(),
            StatusCode::BAD_REQUEST
        );
    }
    #[test]
    fn caps_expiry_and_counter_mismatches_fail_without_silent_skipping() {
        let app = App::default();
        let q = app.queue("test".into()).unwrap();
        assert_eq!(insert(&app, &q, 1, packet(2, 1)), StatusCode::BAD_REQUEST);
        for seq in 1..=CAP as u64 {
            assert_eq!(
                insert(&app, &q, seq, packet(seq, 1)),
                StatusCode::NO_CONTENT
            );
        }
        assert_eq!(
            insert(&app, &q, CAP as u64 + 1, packet(CAP as u64 + 1, 1)),
            StatusCode::TOO_MANY_REQUESTS
        );
        q.records.lock().unwrap().get_mut(&1).unwrap().0 = Instant::now() - Duration::from_secs(31);
        assert_eq!(read(&app, &q, &mut 1, false).unwrap_err(), StatusCode::GONE);
        assert_eq!(insert(&app, &q, 1, packet(1, 1)), StatusCode::GONE);
    }
}
