//! Loopback relay between the capture process and the engine.
//!
//! The capture sends MPEG-TS over TCP from the moment a page is ready, which
//! may be well before its playlist slot. The relay discards that stream until
//! the engine connects, and then starts it at the most recent key frame, so
//! the engine can decode from its first packet. TCP makes the transfer
//! lossless; a UDP socket drops the large key frames of a screen capture.

use std::{io, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::timeout,
};

const TS_PACKET: usize = 188;
const TS_SYNC: u8 = 0x47;
const PAT_PID: u16 = 0;
/// Packet identifier of the video stream; the capture is started with it.
pub(super) const VIDEO_PID: u16 = 0x100;
/// A group of pictures lasts half a second; anything beyond this size means
/// the stream carries no key frame markers and is relayed without a cache.
const MAX_CACHE_BYTES: usize = 32 * 1024 * 1024;
/// The engine reads in real time. A consumer that stalls for this long is
/// dropped, so it cannot hold back the capture.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct Relay {
    ingest_port: u16,
    serve_port: u16,
    task: JoinHandle<()>,
}

impl Relay {
    pub(super) async fn start() -> io::Result<Self> {
        let ingest = TcpListener::bind(("127.0.0.1", 0)).await?;
        let serve = TcpListener::bind(("127.0.0.1", 0)).await?;
        let ingest_port = ingest.local_addr()?.port();
        let serve_port = serve.local_addr()?.port();
        let task = tokio::spawn(async move {
            // Both ends closing is the normal end of a session.
            let _ = run(ingest, serve).await;
        });

        Ok(Self {
            ingest_port,
            serve_port,
            task,
        })
    }

    /// Where the capture process sends its stream.
    pub(super) fn ingest_url(&self) -> String {
        format!("tcp://127.0.0.1:{}", self.ingest_port)
    }

    /// The live source the engine plays.
    pub(super) fn serve_url(&self) -> String {
        format!("tcp://127.0.0.1:{}", self.serve_port)
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(ingest: TcpListener, serve: TcpListener) -> io::Result<()> {
    let (mut source, _) = ingest.accept().await?;

    // Only the capture process feeds this relay.
    drop(ingest);

    let mut cache = GopCache::default();
    let mut consumer: Option<TcpStream> = None;
    let mut buffer = vec![0_u8; TS_PACKET * 348];

    loop {
        tokio::select! {
            read = source.read(&mut buffer) => {
                let length = read?;

                if length == 0 {
                    return Ok(());
                }

                cache.push(&buffer[..length]);

                if let Some(stream) = &mut consumer
                    && !send(stream, &buffer[..length]).await
                {
                    consumer = None;
                }
            }
            accepted = serve.accept() => {
                let (mut stream, _) = accepted?;
                let _ = stream.set_nodelay(true);

                // Everything after the cached bytes follows from `source`.
                consumer = send(&mut stream, cache.bytes()).await.then_some(stream);
            }
        }
    }
}

async fn send(stream: &mut TcpStream, bytes: &[u8]) -> bool {
    matches!(
        timeout(WRITE_TIMEOUT, stream.write_all(bytes)).await,
        Ok(Ok(()))
    )
}

/// The stream since the start of the latest key frame, including the program
/// tables in front of it.
#[derive(Default)]
struct GopCache {
    data: Vec<u8>,
    /// Bytes of `data` that were inspected; always whole packets.
    parsed: usize,
    /// Offset of the program association table in front of the current frame.
    table: Option<usize>,
}

impl GopCache {
    fn push(&mut self, chunk: &[u8]) {
        self.data.extend_from_slice(chunk);

        while self.parsed + TS_PACKET <= self.data.len() {
            let packet = &self.data[self.parsed..self.parsed + TS_PACKET];
            let pid = u16::from(packet[1] & 0x1F) << 8 | u16::from(packet[2]);

            if packet[0] != TS_SYNC {
                self.table = None;
            } else if pid == PAT_PID {
                self.table = Some(self.parsed);
            } else if pid == VIDEO_PID && starts_key_frame(packet) {
                let start = self.table.take().unwrap_or(self.parsed);

                self.data.drain(..start);
                self.parsed -= start;
            }

            self.parsed += TS_PACKET;
        }

        if self.data.len() > MAX_CACHE_BYTES {
            self.data.drain(..self.parsed);
            self.parsed = 0;
            self.table = None;
        }
    }

    fn bytes(&self) -> &[u8] {
        // A trailing partial packet is completed by the chunks that follow.
        &self.data
    }
}

/// The muxer sets the random access indicator on the first packet of a key
/// frame.
fn starts_key_frame(packet: &[u8]) -> bool {
    let has_adaptation_field = packet[3] & 0x20 != 0;

    has_adaptation_field && packet[4] > 0 && packet[5] & 0x40 != 0
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpStream,
        time::{Duration, sleep},
    };

    use super::{GopCache, Relay, TS_PACKET, TS_SYNC, VIDEO_PID};

    fn packet(pid: u16, key_frame: bool, tag: u8) -> Vec<u8> {
        let mut packet = vec![tag; TS_PACKET];

        packet[0] = TS_SYNC;
        packet[1] = (pid >> 8) as u8 & 0x1F;
        packet[2] = pid as u8;
        packet[3] = if key_frame { 0x30 } else { 0x10 };
        packet[4] = if key_frame { 7 } else { tag };
        packet[5] = if key_frame { 0x40 } else { tag };

        packet
    }

    /// Program tables, then one video frame of two packets.
    fn frame(key_frame: bool, tag: u8) -> Vec<u8> {
        [
            packet(0, false, tag),
            packet(0x1000, false, tag),
            packet(VIDEO_PID, key_frame, tag),
            packet(VIDEO_PID, false, tag),
        ]
        .concat()
    }

    #[test]
    fn cache_restarts_at_the_tables_of_each_key_frame() {
        let mut cache = GopCache::default();
        let stream = [
            frame(true, 1),
            frame(false, 2),
            frame(true, 3),
            frame(false, 4),
        ]
        .concat();

        // Chunks do not have to end on packet boundaries.
        for chunk in stream.chunks(301) {
            cache.push(chunk);
        }

        assert_eq!(cache.bytes(), [frame(true, 3), frame(false, 4)].concat());
    }

    #[test]
    fn cache_keeps_everything_until_the_first_key_frame() {
        let mut cache = GopCache::default();
        let stream = [frame(false, 1), frame(false, 2)].concat();

        cache.push(&stream);

        assert_eq!(cache.bytes(), stream);
    }

    #[tokio::test]
    async fn consumer_receives_the_stream_from_the_latest_key_frame() {
        let relay = Relay::start().await.unwrap();
        let mut capture = TcpStream::connect(("127.0.0.1", relay.ingest_port))
            .await
            .unwrap();

        capture.write_all(&frame(true, 1)).await.unwrap();
        capture.write_all(&frame(true, 2)).await.unwrap();
        capture.write_all(&frame(false, 3)).await.unwrap();
        // Let the relay read what was sent before the consumer connects.
        sleep(Duration::from_millis(200)).await;

        let mut consumer = TcpStream::connect(("127.0.0.1", relay.serve_port))
            .await
            .unwrap();

        sleep(Duration::from_millis(200)).await;
        capture.write_all(&frame(false, 4)).await.unwrap();
        drop(capture);

        let mut received = Vec::new();

        consumer.read_to_end(&mut received).await.unwrap();

        assert_eq!(
            received,
            [frame(true, 2), frame(false, 3), frame(false, 4)].concat()
        );
    }
}
