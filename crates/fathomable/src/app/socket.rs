// @okf-doc: /decisions/0051-retire-one-release-compatibility.md
//! The session socket: parse protocol lines and hand each request to the
//! app loop.
//!
//! Only same-user peers are served (ADR 0014): `$XDG_RUNTIME_DIR` is
//! private already, and the peer-uid check makes that explicit.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use fathomable_core::session::{Request, Response};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

/// A request the app loop must answer, with the channel to answer on.
#[derive(Debug)]
pub(crate) struct Envelope {
    pub(crate) request: Request,
    pub(crate) reply: oneshot::Sender<Response>,
}

/// A bound Unix socket, removed from disk when dropped.
#[derive(Debug)]
pub(crate) struct Listener {
    path: PathBuf,
    /// The uid that owns the socket file: this process's own, read back
    /// from the file it just bound so no `/proc` is needed.
    uid: u32,
    socket: UnixListener,
}

impl Listener {
    /// Bind `path`, creating its directory and replacing a stale file.
    pub(crate) fn bind(path: &Path) -> io::Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        match fs::remove_file(path) {
            Ok(()) => tracing::info!(path = %path.display(), "replaced stale socket"),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(path)?;
        let uid = fs::metadata(path)?.uid();
        tracing::info!(path = %path.display(), "listening on the viewer socket");
        Ok(Self {
            path: path.to_path_buf(),
            uid,
            socket: listener,
        })
    }

    /// Accept connections until dropped; every request goes to `app` and
    /// waits for the loop's reply.
    pub(crate) fn serve(self, app: mpsc::Sender<Envelope>) -> Serving {
        let path = self.path.clone();
        let uid = self.uid;
        let handle = tokio::spawn(async move {
            loop {
                match self.socket.accept().await {
                    Ok((stream, _)) => {
                        if !same_user(&stream, uid) {
                            tracing::warn!("refused socket peer with another uid");
                            continue;
                        }
                        tokio::spawn(connection(stream, app.clone()));
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

/// Whether the peer runs as `uid`, the user this process runs as.
fn same_user(stream: &UnixStream, uid: u32) -> bool {
    stream.peer_cred().is_ok_and(|peer| peer.uid() == uid)
}

async fn connection(stream: UnixStream, app: mpsc::Sender<Envelope>) {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(request = %line, "socket request");
        let response = match line.parse::<Request>() {
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
        return Response::Error("viewer is shutting down".to_owned());
    }
    answer
        .await
        .unwrap_or_else(|_| Response::Error("viewer dropped the request".to_owned()))
}

/// The running accept loop; dropping it stops serving and unlinks the socket.
#[derive(Debug)]
pub(crate) struct Serving {
    path: PathBuf,
    handle: JoinHandle<()>,
}

impl Drop for Serving {
    fn drop(&mut self) {
        self.handle.abort();
        let _ = fs::remove_file(&self.path);
    }
}
