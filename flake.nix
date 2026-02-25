{
  description = "gh extension TUI for reviewing GitHub Pull Requests";

  nixConfig = {
    extra-substituters = [ "https://kawarimidoll.cachix.org" ];
    extra-trusted-public-keys = [
      "kawarimidoll.cachix.org-1:43W5G98mVTyDaMeG7ZGzx4h/be5u4ULUGV/9svLjKJY="
    ];
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      git-hooks,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
          versionFile = ./VERSION;
          ghPrismVersion =
            if builtins.pathExists versionFile then
              builtins.replaceStrings [ "\n" ] [ "" ] (builtins.readFile versionFile)
            else
              "${cargoToml.package.version}-${self.dirtyShortRev or self.shortRev or "dirty"}";
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = cargoToml.package.name;
            version = ghPrismVersion;
            src = ./.;
            cargoLock = {
              lockFile = ./Cargo.lock;
            };

            env.GH_PRISM_VERSION = ghPrismVersion;

            nativeBuildInputs = [ pkgs.makeWrapper ];

            postInstall = ''
              wrapProgram $out/bin/gh-prism \
                --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.gh ]}
            '';

            meta = with pkgs.lib; {
              description = "gh extension TUI for reviewing GitHub Pull Requests";
              homepage = "https://github.com/kawarimidoll/gh-prism";
              license = licenses.mit;
              maintainers = [ ];
              mainProgram = "gh-prism";
            };
          };
        }
      );

      overlays.default = _final: prev: {
        gh-prism = self.packages.${prev.system}.default;
      };

      homeManagerModules.default =
        {
          config,
          lib,
          pkgs,
          ...
        }:
        let
          cfg = config.programs.gh-prism;
        in
        {
          options.programs.gh-prism = {
            enable = lib.mkEnableOption "gh-prism - TUI for reviewing GitHub Pull Requests";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
              description = "The gh-prism package to use.";
            };
          };

          config = lib.mkIf cfg.enable {
            home.packages = [ cfg.package ];
          };
        };

      checks = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          pre-commit-check = git-hooks.lib.${system}.run {
            src = ./.;
            hooks = {
              # Rust
              rustfmt.enable = true;
              clippy = {
                enable = true;
                stages = [ "pre-push" ]; # Heavy check - run on push only
              };

              # Nix
              nixfmt-rfc-style.enable = true;

              # Conventional Commits (commit-msg stage)
              convco = {
                enable = true;
                entry =
                  let
                    script = pkgs.writeShellScript "convco-check" ''
                      msg=$(head -1 "$1")
                      # Skip git-generated messages (fixup/squash/amend/revert)
                      re='^(fixup|squash|amend)! |^Revert "'
                      if [[ "$msg" =~ $re ]]; then
                        exit 0
                      fi
                      ${pkgs.lib.getExe pkgs.convco} check --from-stdin < "$1"
                    '';
                  in
                  builtins.toString script;
              };

              # Markdown / YAML (fast alternative to prettier)
              dprint = {
                enable = true;
                name = "dprint";
                entry = "${pkgs.dprint}/bin/dprint fmt --diff";
                types = [
                  "markdown"
                  "yaml"
                ];
                pass_filenames = false;
              };

              # YAML (GitHub Actions)
              actionlint.enable = true;

              # Spell check (Rust-based, fast)
              typos.enable = true;

              # Security
              check-merge-conflicts.enable = true;
              detect-private-keys.enable = true;

              # File hygiene
              check-case-conflicts.enable = true;
              end-of-file-fixer.enable = true;
              trim-trailing-whitespace.enable = true;
            };
          };
        }
      );

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShell {
            inherit (self.checks.${system}.pre-commit-check) shellHook;
            buildInputs = with pkgs; [
              cargo
              rustc
              rust-analyzer
              clippy
              rustfmt
              just
              nixfmt-rfc-style
              dprint
              actionlint
              convco
              typos
            ];
          };
        }
      );
    };
}
