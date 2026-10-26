use std::{
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

const STOP_CHECK: Duration = Duration::from_millis(50);

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
}
impl Peer {
    pub(crate) fn new(tcp: TcpStream, stop: Arc<AtomicBool>) -> std::io::Result<Self> {
        tcp.set_nonblocking(false)?;
        tcp.set_read_timeout(Some(STOP_CHECK))?;
        tcp.set_write_timeout(Some(STOP_CHECK))?;
        Ok(Self { tcp, stop })
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
        self.wait(|tcp| tcp.read(buf))
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
