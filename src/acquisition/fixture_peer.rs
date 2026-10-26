use crate::census::{AccountState, EncryptedStore};
use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
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

/// Fixture side of one connection. Like a real server, it waits for the client
/// to act or hang up instead of imposing its own idle deadline, so a slow but
/// valid client is never cut off. Socket timeouts only let it notice `stop`.
pub(crate) struct Peer {
    pub(crate) tcp: TcpStream,
    stop: Arc<AtomicBool>,
    opened: Instant,
    waiting: Duration,
}
impl Peer {
    pub(crate) fn new(tcp: TcpStream, stop: Arc<AtomicBool>) -> std::io::Result<Self> {
        tcp.set_nonblocking(false)?;
        tcp.set_read_timeout(Some(STOP_CHECK))?;
        tcp.set_write_timeout(Some(STOP_CHECK))?;
        Ok(Self {
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
}
impl Read for Peer {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let began = Instant::now();
        let result = self.wait(|tcp| tcp.read(buf));
        self.waiting += began.elapsed();
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
        self.wait(|tcp| tcp.write(buf))
    }
    fn flush(&mut self) -> std::io::Result<()> {
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
