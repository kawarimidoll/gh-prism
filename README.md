# gh-prism

gh extension to review pull request

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
