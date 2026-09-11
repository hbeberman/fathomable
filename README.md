# fathomable

Read-only terminal workspace viewer and annotation side-car for
agent-driven work. You read files as they change, leave threads on the
lines an agent wrote, and the agent answers over MCP. New here? Read the
[setup guide](docs/guide.md).

Linux only: viewer liveness and the session bonds that let a headless
`--mcp` learn its session read `/proc`.

## Install

Building needs a Rust toolchain, a C linker, and git. Install the
system packages for your distribution, then rustup:

```sh
# Fedora
sudo dnf install gcc git

# Azure Linux 3
sudo tdnf install build-essential git ca-certificates

# Ubuntu 24.04
sudo apt install build-essential git

# any of the above
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

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
