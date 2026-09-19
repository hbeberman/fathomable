# fathomable

Read-only terminal workspace viewer and annotation side-car for
agent-driven work. You read files as they change, leave threads on the
lines an agent wrote, and the agent answers over MCP. Currently Linux only.

**Enthusiast alpha:** expect features to appear, change, or disappear at
any time. Fathomable stores are not guaranteed to survive upgrades; treat
annotations, review points, and other app state as disposable between
versions. Keep important review conclusions elsewhere. Incompatible stores
are refused, not automatically migrated or deleted. See the
[alpha contract](docs/decisions/0083-single-user-alpha-clean-slate.md#enthusiast-alpha-contract).

## Install

Building needs Rust **1.95 or newer**, a C linker, and Git. The checkout
defaults to stable Rust; the minimum is checked independently in CI.
Run the package commands for your distribution, then install Rust and Fathomable:

```sh
# Azure Linux 4 / Fedora
sudo dnf install gcc git curl ca-certificates tar

# Azure Linux 3
sudo tdnf install build-essential git curl ca-certificates tar

# Ubuntu 24.04
sudo apt-get update
sudo apt-get install build-essential git curl ca-certificates tar

# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"

git clone https://github.com/hbeberman/fathomable
cd fathomable
cargo install --path crates/fathomable --locked
# Ensure ~/.cargo/bin is in PATH
fathomable --version
```

At run time the viewer only shells out for two optional things: your
`$VISUAL` or `$EDITOR` to draft a long comment, and `xdg-open` to follow
a link.

## MCP setup

**Copilot CLI:** register once, then launch Copilot from the checkout you
want to review:

```sh
copilot mcp add fathomable -- fathomable --mcp
```

**VS Code:** register for your user profile, using the open workspace:

```sh
code --add-mcp '{"name":"fathomable","type":"stdio","command":"fathomable","args":["--mcp","${workspaceFolder}"]}'
```

Fathomable must be on `PATH` in the Linux environment where the server runs.
Approve the server when prompted; the viewer need not be running.
See the [setup guide](docs/guide.md#connect-an-agent) for configuration and
remote-workspace details.

## Using it

```sh
fathomable               # Open the cwd or parent git repo
fathomable README.md     # Open on a single file
```

The [setup guide](docs/guide.md) covers the UX, KDL configuration, saved
state, and connecting an agent over MCP.

## Contribute

See [CONTRIBUTING.md](CONTRIBUTING.md) for build environment setup,
including the commit gate, dependency monitoring, and maintaining the doc
bundle. Durable project knowledge lives under [`docs/`](docs/index.md).

## AI Notice
This project is developed via LLM coding agents.

## License

[MIT](LICENSE).

Third-party components retain their own licenses. The checked-in
[license bundle](crates/fathomable/assets/licenses.txt) contains their license
texts, copyright notices, and source references. It is also available offline
in the viewer through **Help > Licenses**.

The bundled Rust-runtime inventory describes the recorded release compiler.
Before redistributing a source build made with another compiler, review its
runtime notices; see [release builds](CONTRIBUTING.md#release-builds).
