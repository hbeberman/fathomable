# fathomable

Read-only terminal workspace viewer and annotation side-car for
agent-driven work. You read files as they change, leave threads on the
lines an agent wrote, and the agent answers over MCP. Currently Linux only.

## Install

Building needs a Rust toolchain, a C linker, and Git. Run the package commands
for your distribution, then install Rust and Fathomable:

```sh
# Fedora
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
fathomable --version
```

At run time the viewer only shells out for two optional things: your
`$VISUAL` or `$EDITOR` to draft a long comment, and `xdg-open` to follow
a link.

## Using it

```sh
fathomable               # Open the cwd or parent git repo
fathomable README.md     # Open on a single file
```

The [setup guide](docs/guide.md) covers the UX, KDL configuration, saved
state, and connecting an agent over MCP.

## Contribute

Product installation does not install contributor tools or Git hooks. See
[CONTRIBUTING.md](CONTRIBUTING.md) for that explicit setup, the commit gate,
dependency monitoring, and the documentation bundle. Durable project
knowledge lives under [`docs/`](docs/index.md).

## License

[MIT](LICENSE).

Third-party components retain their own licenses. The checked-in
[license bundle](crates/fathomable/assets/licenses.txt) contains their license
texts, copyright notices, and source references. It is also available offline
in the viewer through **Help > Licenses** or `:licenses`.
