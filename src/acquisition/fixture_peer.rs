use crate::census::{AccountState, EncryptedStore};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

const STOP_CHECK: Duration = Duration::from_millis(50);
/// Per-command budget for fixture clients on success paths. The first TLS
/// handshake in a fresh process spent 1.8 to 2.0 s inside the client TLS
/// library under host load (reliability-2), so a 2 s budget failed healthy
/// exchanges. Tests of deadlines set their own tighter budget.
pub(crate) const FIXTURE_COMMAND_TIMEOUT: Duration = Duration::from_secs(10);

fn pending(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}

/// An emulated network path between a fixture and its client: a one-way
/// delay and a bandwidth limit per direction. Bytes cross the link in order;
/// each chunk waits for the link to be free, takes its size over the rate to
/// send, then arrives after the delay. Up is client to fixture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Link {
    pub up_delay: Duration,
    pub down_delay: Duration,
    /// Bytes per second; zero is unlimited.
    pub up_rate: u64,
    pub down_rate: u64,
}
impl Link {
    /// `$MAILROOM_FIXTURE_LINK`, comma-separated `key=value` pairs:
    /// `delay_ms` sets both delays, `up_ms` and `down_ms` one each, and
    /// `up_kbit` and `down_kbit` the rates in kbit/s. Unset means no link.
    pub(crate) fn from_env() -> Option<Self> {
        Self::parse(&std::env::var("MAILROOM_FIXTURE_LINK").ok()?)
    }
    pub(crate) fn parse(spec: &str) -> Option<Self> {
        let mut link = Self::default();
        for pair in spec.split(',').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=')?;
            let value: u64 = value.trim().parse().ok()?;
            match key.trim() {
                "delay_ms" => {
                    link.up_delay = Duration::from_millis(value);
                    link.down_delay = link.up_delay;
                }
                "up_ms" => link.up_delay = Duration::from_millis(value),
                "down_ms" => link.down_delay = Duration::from_millis(value),
                "up_kbit" => link.up_rate = value * 1000 / 8,
                "down_kbit" => link.down_rate = value * 1000 / 8,
                _ => return None,
            }
        }
        Some(link)
    }
}

/// One direction of a [`Link`]: when the link is next free.
struct Lane {
    delay: Duration,
    rate: u64,
    free: Instant,
}
impl Lane {
    fn new(delay: Duration, rate: u64) -> Self {
        Self {
            delay,
            rate,
            free: Instant::now(),
        }
    }
    /// Arrival time of `len` bytes handed to the link now.
    fn send(&mut self, len: usize) -> Instant {
        let start = self.free.max(Instant::now());
        let serialization = if self.rate == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(len as f64 / self.rate as f64)
        };
        self.free = start + serialization;
        self.free + self.delay
    }
}

/// Process-wide traffic counters of all fixture connections. Over an
/// emulated link a round trip is counted each time the fixture, having
/// answered everything it was sent, has to wait for the client's next bytes:
/// the client could not send them before it read the answer. Commands that
/// were already on their way, as pipelined commands are, count for nothing.
/// Over plain loopback each send after the fixture has sent counts.
#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub(crate) struct Traffic {
    pub connections: u64,
    pub round_trips: u64,
    pub bytes_up: u64,
    pub bytes_down: u64,
}
static CONNECTIONS: AtomicU64 = AtomicU64::new(0);
static ROUND_TRIPS: AtomicU64 = AtomicU64::new(0);
static BYTES_UP: AtomicU64 = AtomicU64::new(0);
static BYTES_DOWN: AtomicU64 = AtomicU64::new(0);
/// Returns the counters since the last call and resets them.
pub(crate) fn take_traffic() -> Traffic {
    Traffic {
        connections: CONNECTIONS.swap(0, Ordering::SeqCst),
        round_trips: ROUND_TRIPS.swap(0, Ordering::SeqCst),
        bytes_up: BYTES_UP.swap(0, Ordering::SeqCst),
        bytes_down: BYTES_DOWN.swap(0, Ordering::SeqCst),
    }
}

type Delivery = (Instant, Vec<u8>);

/// The emulated path of one connection. A reader thread takes client bytes
/// off the socket as they come and releases them after the up delay; a
/// writer thread puts fixture bytes on the socket after the down delay.
struct Emulated {
    up: mpsc::Receiver<Delivery>,
    pending: Vec<u8>,
    offset: usize,
    down: Option<mpsc::Sender<Delivery>>,
    lane: Lane,
    reader: Option<thread::JoinHandle<()>>,
    writer: Option<thread::JoinHandle<()>>,
    closing: Arc<AtomicBool>,
}
impl Emulated {
    fn start(tcp: &TcpStream, link: Link, stop: Arc<AtomicBool>) -> std::io::Result<Self> {
        let closing = Arc::new(AtomicBool::new(false));
        let (up_tx, up) = mpsc::channel();
        let mut source = tcp.try_clone()?;
        let done = closing.clone();
        let reader = thread::spawn(move || {
            let mut lane = Lane::new(link.up_delay, link.up_rate);
            let mut buf = vec![0; 64 * 1024];
            while !done.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                match source.read(&mut buf) {
                    Ok(0) => {
                        let _ = up_tx.send((lane.send(0), vec![]));
                        return;
                    }
                    Ok(n) => {
                        if up_tx.send((lane.send(n), buf[..n].to_vec())).is_err() {
                            return;
                        }
                    }
                    Err(e) if pending(&e) => {}
                    Err(_) => {
                        let _ = up_tx.send((lane.send(0), vec![]));
                        return;
                    }
                }
            }
        });
        let (down, rx) = mpsc::channel::<Delivery>();
        let mut sink = tcp.try_clone()?;
        let writer = thread::spawn(move || {
            while let Ok((at, bytes)) = rx.recv() {
                let now = Instant::now();
                if at > now {
                    thread::sleep(at - now);
                }
                let mut offset = 0;
                while offset < bytes.len() {
                    match sink.write(&bytes[offset..]) {
                        Ok(0) => return,
                        Ok(n) => offset += n,
                        Err(e) if pending(&e) => {}
                        Err(_) => return,
                    }
                }
            }
        });
        Ok(Self {
            up,
            pending: vec![],
            offset: 0,
            down: Some(down),
            lane: Lane::new(link.down_delay, link.down_rate),
            reader: Some(reader),
            writer: Some(writer),
            closing,
        })
    }
}
impl Drop for Emulated {
    fn drop(&mut self) {
        self.down.take();
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        self.closing.store(true, Ordering::SeqCst);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Fixture side of one connection. Like a real server, it waits for the client
/// to act or hang up instead of imposing its own idle deadline, so a slow but
/// valid client is never cut off. Socket timeouts only let it notice `stop`.
pub(crate) struct Peer {
    emulated: Option<Emulated>,
    sent: bool,
    pub(crate) tcp: TcpStream,
    stop: Arc<AtomicBool>,
    opened: Instant,
    waiting: Duration,
}
impl Peer {
    /// A connection over the link `$MAILROOM_FIXTURE_LINK` describes, or
    /// over plain loopback.
    pub(crate) fn new(tcp: TcpStream, stop: Arc<AtomicBool>) -> std::io::Result<Self> {
        Self::with_link(tcp, stop, Link::from_env())
    }
    pub(crate) fn with_link(
        tcp: TcpStream,
        stop: Arc<AtomicBool>,
        link: Option<Link>,
    ) -> std::io::Result<Self> {
        tcp.set_nonblocking(false)?;
        tcp.set_read_timeout(Some(STOP_CHECK))?;
        tcp.set_write_timeout(Some(STOP_CHECK))?;
        CONNECTIONS.fetch_add(1, Ordering::SeqCst);
        Ok(Self {
            emulated: link
                .map(|link| Emulated::start(&tcp, link, stop.clone()))
                .transpose()?,
            sent: true,
            tcp,
            stop,
            opened: Instant::now(),
            waiting: Duration::ZERO,
        })
    }
    pub(crate) fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }
    fn wait<T>(
        &mut self,
        mut io: impl FnMut(&mut TcpStream) -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        loop {
            match io(&mut self.tcp) {
                Err(e) if pending(&e) && !self.stopped() => {}
                result => return result,
            }
        }
    }
    /// Holds a stalled exchange open until the client gives up and closes.
    pub(crate) fn until_closed(io: &mut impl Read) {
        let _ = std::io::copy(io, &mut std::io::sink());
    }
    fn receive(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let Some(e) = self.emulated.as_mut() else {
            return self.wait(|tcp| tcp.read(buf));
        };
        let mut waited = false;
        while e.offset == e.pending.len() {
            if self.stop.load(Ordering::Relaxed) {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            let next = match e.up.try_recv() {
                Ok(delivery) => Ok(delivery),
                Err(_) => {
                    waited = true;
                    e.up.recv_timeout(STOP_CHECK)
                }
            };
            match next {
                Ok((at, bytes)) => {
                    let now = Instant::now();
                    if at > now {
                        thread::sleep(at - now);
                    }
                    if bytes.is_empty() {
                        return Ok(0);
                    }
                    if at > now {
                        waited = true;
                    }
                    e.pending = bytes;
                    e.offset = 0;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
            }
        }
        let n = buf.len().min(e.pending.len() - e.offset);
        buf[..n].copy_from_slice(&e.pending[e.offset..e.offset + n]);
        e.offset += n;
        if waited && std::mem::take(&mut self.sent) {
            ROUND_TRIPS.fetch_add(1, Ordering::Relaxed);
        }
        Ok(n)
    }
}
impl Read for Peer {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let began = Instant::now();
        let result = self.receive(buf);
        self.waiting += began.elapsed();
        if let Ok(n @ 1..) = result {
            BYTES_UP.fetch_add(n as u64, Ordering::Relaxed);
            if self.emulated.is_none() && std::mem::take(&mut self.sent) {
                ROUND_TRIPS.fetch_add(1, Ordering::Relaxed);
            }
        }
        result
    }
}
/// Splits each connection's lifetime into time spent waiting for the client
/// and time the fixture itself was busy, so a slow exchange names its side.
impl Drop for Peer {
    fn drop(&mut self) {
        let open = self.opened.elapsed();
        eprintln!(
            "fixture.peer.closed open_ms={} client_wait_ms={} fixture_busy_ms={}",
            open.as_millis(),
            self.waiting.as_millis(),
            open.saturating_sub(self.waiting).as_millis()
        );
    }
}
impl Write for Peer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = match self.emulated.as_mut() {
            Some(e) => {
                let at = e.lane.send(buf.len());
                e.down
                    .as_ref()
                    .expect("open link")
                    .send((at, buf.to_vec()))
                    .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
                buf.len()
            }
            None => self.wait(|tcp| tcp.write(buf))?,
        };
        if n > 0 {
            self.sent = true;
            BYTES_DOWN.fetch_add(n as u64, Ordering::Relaxed);
        }
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if self.emulated.is_some() {
            return Ok(());
        }
        self.wait(|tcp| tcp.flush())
    }
}

/// The provider state one fixture connection last saved, with the exact file
/// bytes it produced. While the file still holds those bytes, the next command
/// reuses the state instead of decrypting and parsing the whole store again;
/// any other writer changes the bytes and forces a fresh load.
#[derive(Default)]
pub(crate) struct LastSaved(Option<(Vec<u8>, AccountState)>);
impl LastSaved {
    pub(crate) fn load(&self, store: &EncryptedStore) -> AccountState {
        match (&self.0, std::fs::read(store.path())) {
            (Some((saved, state)), Ok(current)) if *saved == current => state.clone(),
            _ => store
                .load_state()
                .unwrap_or_else(|error| panic!("fixture.store_load={error:?}")),
        }
    }
    pub(crate) fn save(&mut self, store: &EncryptedStore, state: &mut AccountState) {
        store
            .save_state(state)
            .unwrap_or_else(|error| panic!("fixture.store_save={error:?}"));
        self.0 = std::fs::read(store.path())
            .ok()
            .map(|bytes| (bytes, state.clone()));
    }
}

/// Provider state of one personal-scale fixture connection, decoded once and
/// reused while the store file keeps its identity: device, inode, length and
/// modification and change times. Those stores are hundreds of MB, so
/// decoding or even reading them per command would measure the fixture.
#[derive(Default)]
pub(crate) struct Cached(Option<(FileId, AccountState)>);
pub(crate) type FileId = (u64, u64, u64, i64, i64, i64, i64);
pub(crate) fn file_id(path: &std::path::Path) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::metadata(path).ok()?;
    Some((
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    ))
}
impl Cached {
    pub(crate) fn load(&mut self, store: &EncryptedStore) -> AccountState {
        let before = file_id(&store.path());
        if let (Some((id, state)), Some(current)) = (&self.0, before) {
            if *id == current {
                return state.clone();
            }
        }
        let state = store
            .load_state()
            .unwrap_or_else(|error| panic!("fixture.store_load={error:?}"));
        self.0 = before
            .filter(|id| file_id(&store.path()) == Some(*id))
            .map(|id| (id, state.clone()));
        state
    }
    pub(crate) fn save(&mut self, store: &EncryptedStore, state: &mut AccountState) {
        store
            .save_state(state)
            .unwrap_or_else(|error| panic!("fixture.store_save={error:?}"));
        self.0 = file_id(&store.path()).map(|id| (id, state.clone()));
    }
}

/// Serves every connection on a thread of its own, as an IMAP server serves
/// concurrent sessions, and stops when `stop` is set. `serve` gets a lock
/// that makes each command's read, change and save of the provider state
/// atomic across sessions. A panic in any session is raised again here.
pub(crate) fn serve_sessions(
    listener: std::net::TcpListener,
    stop: Arc<AtomicBool>,
    serve: impl Fn(Peer, &std::sync::Mutex<()>) -> std::io::Result<()> + Send + Sync + 'static,
) -> thread::JoinHandle<()> {
    let serve = Arc::new(serve);
    let lock = Arc::new(std::sync::Mutex::new(()));
    thread::spawn(move || {
        let mut sessions = vec![];
        while !stop.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((tcp, _)) => {
                    let (serve, lock, stop) = (serve.clone(), lock.clone(), stop.clone());
                    sessions.push(thread::spawn(move || {
                        let _ = Peer::new(tcp, stop).and_then(|peer| serve(peer, &lock));
                    }));
                }
                Err(_) => thread::sleep(Duration::from_millis(5)),
            }
        }
        for session in sessions {
            if let Err(panic) = session.join() {
                std::panic::resume_unwind(panic);
            }
        }
    })
}

/// Holds the command lock, recovering it after a session panicked.
pub(crate) fn command(lock: &std::sync::Mutex<()>) -> std::sync::MutexGuard<'_, ()> {
    lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The UIDs of `existing` an IMAP sequence set names, `*` being the largest
/// (RFC 3501 section 9, `sequence-set`). `None` when the set is malformed.
pub(crate) fn uid_set(set: &str, existing: &std::collections::BTreeSet<u32>) -> Option<Vec<u32>> {
    let largest = existing.last().copied().unwrap_or(0);
    let number = |s: &str| -> Option<u32> {
        if s == "*" {
            Some(largest)
        } else {
            s.parse().ok().filter(|n| *n > 0)
        }
    };
    let mut found = std::collections::BTreeSet::new();
    for item in set.split(',') {
        let (a, b) = match item.split_once(':') {
            Some((a, b)) => (number(a)?, number(b)?),
            None => (number(item)?, number(item)?),
        };
        found.extend(existing.range(a.min(b)..=a.max(b)).copied());
    }
    Some(found.into_iter().collect())
}
