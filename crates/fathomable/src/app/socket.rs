// @okf-doc: /decisions/0012-workspace-mode.md
//! The session socket: answer protocol v0 requests line by line.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fathomable_core::session::{Record, answer};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::task::JoinHandle;

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

    /// Accept connections until dropped, answering on behalf of `record`.
    pub fn serve(self, record: Record) -> Serving {
        let path = self.path.clone();
        let handle = tokio::spawn(async move {
            loop {
                match self.listener.accept().await {
                    Ok((stream, _)) => {
                        let record = record.clone();
                        tokio::spawn(async move {
                            let (reader, mut writer) = stream.into_split();
                            let mut lines = BufReader::new(reader).lines();
                            while let Ok(Some(line)) = lines.next_line().await {
                                tracing::debug!(request = %line, "socket request");
                                let mut reply = answer(&line, &record).to_line();
                                reply.push('\n');
                                if writer.write_all(reply.as_bytes()).await.is_err() {
                                    break;
                                }
                            }
                        });
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
