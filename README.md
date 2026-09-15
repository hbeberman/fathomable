# fathomable

Read-only terminal workspace viewer and annotation side-car for
agent-driven work. You read files as they change, leave threads on the
lines an agent wrote, and the agent answers over MCP. New here? Read the
[setup guide](docs/guide.md).

Linux only: viewer liveness reads `/proc`. Agent chat identity comes
automatically from supported harness environment or MCP metadata;
automatic comment delivery remains opt-in. See the
[agent setup](docs/guide.md#8-connect-an-agent) for supported channels
and lifecycle limits.

## Install

Building needs a Rust toolchain, a C linker, git, and make. Install the
system packages for your distribution, then rustup:

```sh
# Fedora
sudo dnf install gcc git make

# Azure Linux 3
sudo tdnf install build-essential git ca-certificates

# Azure Linux 4
sudo tdnf install gcc git make tar ca-certificates

# Ubuntu 24.04
sudo apt install build-essential git curl

# any of the above
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Each line was run in that distribution's official container image
(`fedora:latest`, `mcr.microsoft.com/azurelinux/base/core:3.0`,
`mcr.microsoft.com/azurelinux-beta/base/core:4.0`, `ubuntu:24.04`).
`rust-toolchain.toml` pins the compiler, so the first `cargo` command in
the checkout installs the right version on its own. The product crates
are pure Rust: no OpenSSL, libgit2, or other C headers are needed.

```sh
git clone https://github.com/hbeberman/fathomable
cd fathomable
make install          # cargo install --path crates/fathomable --locked
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

See [CONTRIBUTING.md](CONTRIBUTING.md) for the build tooling, the commit
gate, and the documentation bundle. Durable project knowledge lives under
[`docs/`](docs/index.md).

## License

[MIT](LICENSE).
