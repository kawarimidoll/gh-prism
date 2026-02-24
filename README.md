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

### Installation with Nix / home-manager

Add the input to your `flake.nix`:

```nix
{
  inputs = {
    gh-prism = {
      url = "github:kawarimidoll/gh-prism";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };
}
```

Then add it to `programs.gh.extensions`:

```nix
{ inputs, pkgs, ... }:
{
  programs.gh = {
    enable = true;
    extensions = [
      inputs.gh-prism.packages.${pkgs.stdenv.hostPlatform.system}.default
    ];
  };
}
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

## Release

Releases use [CalVer](https://calver.org/) (`YY.MM.DD`). To create a new release:

```bash
gh workflow run publish.yml
```

Or use the "Run workflow" button on the [Actions tab](https://github.com/kawarimidoll/gh-prism/actions/workflows/publish.yml).
