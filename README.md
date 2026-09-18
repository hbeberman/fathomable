# fathomable

Read-only terminal workspace viewer and annotation side-car for
agent-driven work. You read files as they change, leave threads on the
lines an agent wrote, and the agent answers over MCP. New here? Read the
[setup guide](docs/guide.md).

Linux only: viewer liveness reads `/proc`. Agent chat identity comes
automatically from supported harness environment or MCP metadata for writes;
reads do not require identity or change discussion state. See the
[agent setup](docs/guide.md#8-connect-an-agent) for supported channels
and lifecycle limits.

## Install

Building needs a Rust toolchain, a C linker, and Git. Install the source-build
packages for your distribution first:

```sh
# Fedora
sudo dnf install gcc git curl ca-certificates tar

# Azure Linux 3
sudo tdnf install build-essential git curl ca-certificates tar

# Ubuntu 24.04
sudo apt-get update
sudo apt-get install build-essential git curl ca-certificates tar
```

Then install rustup and load Cargo into the current shell:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
```

`rust-toolchain.toml` pins the compiler, so the first `cargo` command in
the checkout installs the right version on its own. The product crates
are pure Rust: no OpenSSL, libgit2, or other C headers are needed.

```sh
git clone https://github.com/hbeberman/fathomable
cd fathomable
cargo install --path crates/fathomable --locked
fathomable --version
```

At run time the viewer only shells out for two optional things: your
`$VISUAL` or `$EDITOR` to draft a long comment, and `xdg-open` to follow
a link.

## Use it

```sh
fathomable README.md     # single file
fathomable               # workspace rooted at the enclosing git root, or cwd
```

The [setup guide](docs/guide.md) covers the keys, themes, configuration,
and connecting an agent over MCP.

## Contribute

Product installation does not install contributor tools or Git hooks. See
[CONTRIBUTING.md](CONTRIBUTING.md) for that explicit setup, the commit gate,
dependency monitoring, and the documentation bundle. Durable project
knowledge lives under [`docs/`](docs/index.md).

## License

[MIT](LICENSE).

Third-party components retain their own licenses. Open **Help > Licenses**
or `:licenses` in the viewer for bundled license texts, copyright notices,
and source references.
