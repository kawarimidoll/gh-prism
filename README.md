# gh-prism

A TUI for reviewing GitHub Pull Requests, built as a [gh](https://cli.github.com/) extension.

## Features

- 📋 PR description, commits, changed files, and conversation in a single TUI
- 🔍 Syntax-highlighted diff viewer with hunk/change navigation
- 💬 Inline code review comments with suggestion blocks (`Ctrl+G`)
- ✅ Submit reviews (Approve / Request Changes / Comment)
- 🔀 Merge PRs directly from TUI (merge commit / squash / rebase)
- 🖼️ Inline image preview in PR descriptions
- 🌗 Auto-detects terminal light/dark theme (or force with `--light` / `--dark`)

## Installation

Requires [GitHub CLI](https://cli.github.com/) (`gh`).

```bash
gh extension install kawarimidoll/gh-prism
```

To upgrade to the latest version:

```bash
gh extension upgrade gh-prism
```

<details>
<summary>Installation with Nix / home-manager</summary>

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

#### Using the home-manager module

```nix
{ inputs, ... }:
{
  imports = [ inputs.gh-prism.homeManagerModules.default ];

  programs.gh-prism.enable = true;

  # The module adds gh-prism to programs.gh.extensions automatically
  programs.gh.enable = true;
}
```

#### Using the package directly

```nix
{ inputs, pkgs, ... }:
{
  programs.gh = {
    enable = true;
    extensions = [
      inputs.gh-prism.packages.${pkgs.stdenv.hostPlatform.system}.default
      # other extensions...
    ];
  };
}
```

#### Cachix binary cache (optional)

Prebuilt binaries are available via [Cachix](https://kawarimidoll.cachix.org).
To skip building from source, add the following to your nix-darwin / NixOS /
home-manager configuration:

```nix
nix.settings = {
  extra-substituters = [
    "https://kawarimidoll.cachix.org"
    # other substituters...
  ];
  extra-trusted-public-keys = [
    "kawarimidoll.cachix.org-1:43W5G98mVTyDaMeG7ZGzx4h/be5u4ULUGV/9svLjKJY="
    # other public keys...
  ];
};
```

</details>

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
| `1/2/3` | Jump to pane |
| `Enter` | Open diff / conversation / comment |
| `v` | Enter line select mode |
| `c` | Comment on selected line(s) or PR |
| `x` | Toggle file/commit as viewed |
| `S` | Submit review |
| `M` | Merge pull request |
| `R` | Reload PR data |
| `z` | Toggle zoom |
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
# ./result/bin/gh-prism
```

Or with Cargo:

```bash
cargo build --release
# ./target/release/gh-prism
```

To install the built binary as a gh extension:

```bash
ln -s target/release/gh-prism gh-prism  # or: ln -s result/bin/gh-prism gh-prism
gh extension install .
```

## Release

Releases use [CalVer](https://calver.org/) (`YY.MM.DD`). To create a new release:

```bash
gh workflow run publish.yml
```

Or use the "Run workflow" button on the [Actions tab](https://github.com/kawarimidoll/gh-prism/actions/workflows/publish.yml).
