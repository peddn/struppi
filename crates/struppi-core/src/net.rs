//! The network side of a connection: a task that shuttles bytes between a
//! TCP socket and the engine.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

/// How long a connection attempt may take.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

const READ_BUFFER: usize = 8 * 1024;

/// What the connection task reports to the engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetEvent {
    Connected,
    Data(Vec<u8>),
    /// The connection ended or could not be established. `None` means the
    /// server closed it; otherwise the text describes the error.
    Closed(Option<String>),
}

/// Handle to a running connection task. Dropping it closes the connection.
#[derive(Debug)]
pub struct Connection {
    outgoing: mpsc::UnboundedSender<Vec<u8>>,
    task: JoinHandle<()>,
}

impl Connection {
    /// Starts connecting to `host:port`. Progress is reported through
    /// `events`, ending with exactly one [`NetEvent::Closed`].
    ///
    /// Must be called from within a tokio runtime.
    pub fn open(host: String, port: u16, events: mpsc::Sender<NetEvent>) -> Self {
        let (outgoing, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(host, port, events, rx));
        Self { outgoing, task }
    }

    /// Queues bytes for sending. Returns `false` when the connection is gone.
    pub fn send(&self, bytes: Vec<u8>) -> bool {
        bytes.is_empty() || self.outgoing.send(bytes).is_ok()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn run(
    host: String,
    port: u16,
    events: mpsc::Sender<NetEvent>,
    mut outgoing: mpsc::UnboundedReceiver<Vec<u8>>,
) {
    let reason = match tokio::time::timeout(
        CONNECT_TIMEOUT,
        TcpStream::connect((host.as_str(), port)),
    )
    .await
    {
        Err(_) => Some("connection timed out".to_owned()),
        Ok(Err(error)) => Some(error.to_string()),
        Ok(Ok(stream)) => {
            // Commands are small and interactive; do not wait to batch them.
            let _ = stream.set_nodelay(true);
            if events.send(NetEvent::Connected).await.is_err() {
                return;
            }
            pump(stream, &events, &mut outgoing).await
        }
    };
    let _ = events.send(NetEvent::Closed(reason)).await;
}

/// Moves bytes in both directions until the connection ends. Returns the
/// error, or `None` when the server closed the connection.
async fn pump(
    stream: TcpStream,
    events: &mpsc::Sender<NetEvent>,
    outgoing: &mut mpsc::UnboundedReceiver<Vec<u8>>,
) -> Option<String> {
    let (mut reader, mut writer) = stream.into_split();
    let mut buffer = vec![0u8; READ_BUFFER];
    loop {
        tokio::select! {
            read = reader.read(&mut buffer) => match read {
                Ok(0) => return None,
                Ok(count) => {
                    if events.send(NetEvent::Data(buffer[..count].to_vec())).await.is_err() {
                        return None;
                    }
                }
                Err(error) => return Some(error.to_string()),
            },
            bytes = outgoing.recv() => match bytes {
                Some(bytes) => {
                    if let Err(error) = writer.write_all(&bytes).await {
                        return Some(error.to_string());
                    }
                }
                // The handle was dropped.
                None => return None,
            },
        }
    }
}
