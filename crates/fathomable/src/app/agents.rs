// @okf-doc: /decisions/0040-agent-subscriptions-and-hooks.md
//! The viewer's side of agent subscriptions (ADR 0040): who is
//! subscribed, who watches the open thread, and `Space w`, which hands a
//! subscriber its pending threads through the configured wake command.

use std::process::{Command, Stdio};

use fathomable_core::agents::{Register, Subscriber};
use fathomable_core::annotations::ThreadId;

use crate::app::threads::now;
use crate::app::{App, PickerKind};
use crate::hooks;

impl App {
    /// The live subscribers of this workspace, read from the register.
    pub(super) fn subscribers(&self) -> Vec<Subscriber> {
        let path = self.dirs.agents_file(self.workspace.root());
        match Register::open(path, now(), self.agents.expire_after) {
            Ok(register) => register.subscribers().to_vec(),
            Err(error) => {
                tracing::warn!(%error, "cannot read the agent register");
                Vec::new()
            }
        }
    }

    /// Re-read who watches which thread; called when the store reloads
    /// and when a thread pane opens, so the header stays honest without
    /// reading the register on every frame.
    pub(super) fn refresh_watchers(&mut self) {
        let path = self.dirs.agents_file(self.workspace.root());
        self.watchers = match Register::open(path, now(), self.agents.expire_after) {
            Ok(register) => register
                .watches()
                .iter()
                .map(|w| {
                    let label = register
                        .subscriber(w.subscriber())
                        .map_or_else(|| w.subscriber().to_owned(), Subscriber::label);
                    (w.on().clone(), label)
                })
                .collect(),
            Err(_) => Vec::new(),
        };
    }

    /// The labels of the subscribers watching `thread`.
    pub(super) fn watchers_of(&self, thread: &ThreadId) -> Vec<&str> {
        self.watchers
            .iter()
            .filter(|(on, _)| on == thread)
            .map(|(_, label)| label.as_str())
            .collect()
    }

    /// `Space w`: wake a subscriber with its pending threads. One
    /// subscriber is woken at once; several open a picker.
    pub fn wake(&mut self) {
        if self.agents.wake.is_none() {
            self.notice("set agents.wake in config.kdl to a command with {id} and {prompt}");
            return;
        }
        let subscribers = self.subscribers();
        match subscribers.as_slice() {
            [] => self.notice("no agent is subscribed to this workspace"),
            [only] => {
                let id = only.id().to_owned();
                self.wake_subscriber(&id);
            }
            many => {
                let _ = many;
                self.open_picker(PickerKind::Wake);
            }
        }
    }

    /// Run the wake command for subscriber `id` with what it has not
    /// seen, recording the delivery; a quiet subscriber is left alone.
    pub(super) fn wake_subscriber(&mut self, id: &str) {
        let Some(template) = self.agents.wake.clone() else {
            return;
        };
        let label = self
            .subscribers()
            .iter()
            .find(|s| s.id() == id)
            .map_or_else(|| id.to_owned(), Subscriber::label);
        let root = self.workspace.root().to_path_buf();
        // `Space w` hands the blob over as the agent's next prompt, so it
        // forces a turn exactly as the stop hook does.
        let prompt = match hooks::compose(
            &self.dirs,
            &root,
            id,
            &self.agents,
            hooks::Occasion::TurnEnd,
        ) {
            Ok(Some(prompt)) => prompt,
            Ok(None) => {
                self.notice(format!("nothing pending for {label}"));
                return;
            }
            Err(error) => {
                self.notice(format!("cannot compose for {label}: {error}"));
                return;
            }
        };
        // The placeholders become positional parameters, so the id and
        // the prompt reach the command intact whatever they contain.
        let script = template
            .replace("{id}", "\"$1\"")
            .replace("{prompt}", "\"$2\"");
        let spawned = Command::new("sh")
            .arg("-c")
            .arg(&script)
            .arg("fathomable")
            .arg(id)
            .arg(&prompt)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        match spawned {
            Ok(child) => {
                tracing::info!(id, pid = child.id(), %script, "wake command started");
                self.notice(format!("woke {label}"));
            }
            Err(error) => self.notice(format!("cannot run the wake command: {error}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use fathomable_core::XdgDirs;
    use fathomable_core::agents::{Register, WatchWhen};
    use fathomable_core::annotations::{Draft, LineRange, Store};
    use fathomable_core::config::AgentsConfig;
    use fathomable_core::workspace::Workspace;

    use crate::app::threads::now;
    use crate::app::{App, Options, PickerKind, Popup};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> std::io::Result<Self> {
            let dir =
                std::env::temp_dir().join(format!("fathomable-wake-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(dir.join("ws"))?;
            fs::create_dir_all(dir.join("state"))?;
            Ok(Self(dir))
        }

        fn dirs(&self) -> XdgDirs {
            let state = self.0.join("state").into_os_string();
            XdgDirs::resolve(move |name| (name == "XDG_STATE_HOME").then(|| state.clone()))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// The status row and the thread header read the register; `Space w`
    /// refuses without a command, wakes one subscriber directly, and
    /// offers a picker for several.
    #[test]
    fn subscribers_show_up_and_space_w_picks_one() -> TestResult {
        let dir = TempDir::new("status")?;
        let dirs = dir.dirs();
        let root = dir.0.join("ws").canonicalize()?;
        fs::write(root.join("a.md"), "one\n")?;
        let mut store = Store::open(dirs.threads_file(&root))?;
        let id = store.annotate(
            Draft::new(Path::new("a.md"), LineRange::new(1, 1), "why?"),
            "one\n",
            now(),
        )?;
        let when = now();
        let mut register = Register::open(
            dirs.agents_file(&root),
            when,
            AgentsConfig::default().expire_after,
        )?;
        register.subscribe("s-1", "coder", Some("bot"), None, vec![], when)?;
        register.watch("s-1", &id, WatchWhen::Resolved, vec![], when)?;
        let workspace = Workspace::discover(&root)?;
        let options = Options {
            dirs: dirs.clone(),
            store: Some(Store::open(dirs.threads_file(&root))?),
            ..Options::for_test(root.clone())
        };
        let mut app = App::new(workspace, 80, 24, options);
        let status = app.status_lines();
        let row = status
            .iter()
            .find(|(k, _)| k == "subscribers")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        assert!(row.contains("bot (coder) s-1"), "{row}");
        app.open("a.md".as_ref());
        app.open_thread(id.clone());
        assert_eq!(app.watchers_of(&id), ["bot (coder)"]);
        app.wake();
        assert!(app.message().is_some_and(|m| m.contains("agents.wake")));
        app.agents.wake = Some("true".to_owned());
        app.wake();
        assert!(
            app.message()
                .is_some_and(|m| m.contains("woke bot (coder)")),
            "{:?}",
            app.message()
        );
        app.wake();
        assert!(
            app.message().is_some_and(|m| m.contains("nothing pending")),
            "{:?}",
            app.message()
        );
        register.subscribe("s-2", "reviewer", None, None, vec![], when)?;
        app.wake();
        assert!(matches!(app.popup(), Some(Popup::Picker(p)) if p.kind() == PickerKind::Wake));
        app.picker_confirm();
        assert!(
            app.message()
                .is_some_and(|m| m.contains("nothing pending for")),
            "{:?}",
            app.message()
        );
        Ok(())
    }
}
