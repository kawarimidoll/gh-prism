# gh-prism

A TUI for reviewing GitHub Pull Requests, built as a [gh](https://cli.github.com/) extension.

## Features

- 📋 PR description, commits, changed files, and conversation in a single TUI
- 🔍 Syntax-highlighted side-by-side diff viewer with hunk/change navigation
- 💬 Inline code review comments with suggestion blocks (`Ctrl+G`)
- ✅ Submit reviews (Approve / Request Changes / Comment)
- 🖼️ Inline image preview in PR descriptions
- 🌗 Auto-detects terminal light/dark theme (or force with `--light` / `--dark`)

## Installation

Requires [GitHub CLI](https://cli.github.com/) (`gh`).

```bash
gh extension install kawarimidoll/gh-prism
```

Or build from source (requires Rust toolchain):

```bash
cargo install --path .
```

## Usage

```bash
gh prism <PR_NUMBER>
```

### Options

| Option | Description |
| --- | --- |
| `--repo owner/repo` | Specify repository (default: detect from git remote) |
| `--no-cache` | Disable cache and always fetch from API |
| `--light` | Force light theme |
| `--dark` | Force dark theme |

### Key Bindings (excerpt)

| Key | Action |
| --- | --- |
| `j/k` | Move down / up |
| `h/l` | Previous / next pane |
| `1-4` | Jump to pane |
| `Enter` | Open diff / conversation / comment |
| `v` | Enter line select mode |
| `c` | Comment on selected line(s) or PR |
| `S` | Submit review |
| `?` | Show full help |
| `q` | Quit |

## Development

### Prerequisites

- [Nix](https://nixos.org/) with flakes enabled
- [direnv](https://direnv.net/) (optional but recommended)

### Setup

```bash
# With direnv (recommended)
direnv allow .

# Or manually enter the development shell
nix develop
```

This provides:

- Rust toolchain (cargo, rustc, clippy, rustfmt)
- Pre-commit hooks (auto-installed on first shell entry)
- Formatters (dprint, nixfmt)
- Linters (actionlint, typos)

### Commit Convention

This project uses [Conventional Commits](https://www.conventionalcommits.org/).

Allowed types: `feat`, `fix`, `docs`, `refactor`, `test`, `chore`, `ci`

```bash
git commit -m "feat: add new feature"
git commit -m "fix: resolve bug"
```

### Build

```bash
nix build
```

The binary will be at `./result/bin/gh-prism`.
