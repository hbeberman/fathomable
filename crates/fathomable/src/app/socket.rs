// @okf-doc: /decisions/0014-mcp-server-and-socket-v1.md
//! The session socket: parse protocol lines, answer liveness directly, and
//! hand everything else to the app loop.
//!
//! Only same-user peers are served (ADR 0014): `$XDG_RUNTIME_DIR` is
//! private already, and the peer-uid check makes that explicit.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use fathomable_core::session::{Record, Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// A request the app loop must answer, with the channel to answer on.
#[derive(Debug)]
pub struct Envelope {
    pub request: Request,
    pub reply: oneshot::Sender<Response>,
}

/// A bound Unix socket, removed from disk when dropped.
#[derive(Debug)]
pub struct Listener {
    path: PathBuf,
    listener: UnixListener,
}

impl Listener {
    /// Bind `path`, creating its directory and replacing a stale file.
    pub fn bind(path: &Path) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        match fs::remove_file(path) {
            Ok(()) => tracing::info!(path = %path.display(), "replaced stale socket"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(path)?;
        tracing::info!(path = %path.display(), "listening on session socket");
        Ok(Self {
            path: path.to_path_buf(),
            listener,
        })
    }

    /// Accept connections until dropped. `ping` and `session_info` are
    /// answered from `record`; other requests go to `app` and wait for the
    /// loop's reply.
    pub fn serve(self, record: Record, app: mpsc::Sender<Envelope>) -> Serving {
        let path = self.path.clone();
        let handle = tokio::spawn(async move {
            loop {
                match self.listener.accept().await {
                    Ok((stream, _)) => {
                        if !same_user(&stream) {
                            tracing::warn!("refused socket peer with another uid");
                            continue;
                        }
                        tokio::spawn(connection(stream, record.clone(), app.clone()));
                    }
                    Err(error) => {
                        tracing::warn!(%error, "socket accept failed");
                        break;
                    }
                }
            }
        });
        Serving { path, handle }
    }
}

/// Whether the peer runs as the same user as this process.
fn same_user(stream: &UnixStream) -> bool {
    let Ok(peer) = stream.peer_cred() else {
        return false;
    };
    fs::metadata("/proc/self").is_ok_and(|me| me.uid() == peer.uid())
}

async fn connection(stream: UnixStream, record: Record, app: mpsc::Sender<Envelope>) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(request = %line, "socket request");
        let response = match line.parse::<Request>() {
            Ok(Request::Ping) => Response::Pong,
            Ok(Request::SessionInfo) => Response::Session(record.clone()),
            Ok(request) => forward(&app, request).await,
            Err(error) => Response::Error(error.to_string()),
        };
        let mut reply = response.to_line();
        reply.push('\n');
        if writer.write_all(reply.as_bytes()).await.is_err() {
            break;
        }
    }
}

async fn forward(app: &mpsc::Sender<Envelope>, request: Request) -> Response {
    let (reply, answer) = oneshot::channel();
    if app.send(Envelope { request, reply }).await.is_err() {
        return Response::Error("session is shutting down".to_owned());
    }
    answer
        .await
        .unwrap_or_else(|_| Response::Error("session dropped the request".to_owned()))
}

/// The running accept loop; dropping it stops serving and unlinks the socket.
#[derive(Debug)]
pub struct Serving {
    path: PathBuf,
    handle: JoinHandle<()>,
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.handle.abort();
        let _ = fs::remove_file(&self.path);
    }
}
