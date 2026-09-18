# rushi-simple-english — ext flake (Mode B, ext-flake-authoring.md §3).
#
# Exposes two rushi package artifacts:
#   packages.<system>.hook-simple-english  hook crate  (bare buildRustPackage)
#   packages.<system>.simple-english-ext  TUI ext crate (wrapAsExt)
#
# $out contracts (ext-flake-authoring.md §2):
#   hook: $out/bin/harness-hook-simple-english
#         → mkRushi copies bin/. → hooks/
#   ext:  $out/simple-english/ext.toml + $out/simple-english/target/release/simple-english-ext
#         → mkRushi copies into ui_extensions/
#
# Usage from a consumer flake (ext-flake-authoring.md §5):
#   inputs.rushi-simple-english.url = "github:TonyWu20/rushi-simple-english?ref=<rev>";
#   rushi.external_hooks       = [ seFlake.packages.${system}.hook-simple-english ];
#   rushi.external_ui_extensions = [ seFlake.packages.${system}.simple-english-ext ];

{
  description = "rushi-simple-english — ASD-STE100 writing-rule hook and TUI status extension for rushi";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, fenix }:
    let
      # Mirror the kernel flake: fixed system list + genAttrs.
      # Avoid flake-utils.eachDefaultSystem (transposes the result and
      # breaks nix develop / per-system devShells).
      supportedSystems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      pkgLib = nixpkgs.lib;
    in
    {
      # ── Per-system packages and devShells ──
      # Standard flake layout: packages.<system>.<name>.
      # This makes `nix build .#hook-simple-english` work (Nix resolves
      # to packages.<system>.<name>) and lets consumers write
      # extFlake.packages.${system}.hook-simple-english.
      packages = pkgLib.genAttrs supportedSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ fenix.overlays.default ];
          };
          rustToolchain = fenix.packages.${system}.stable.withComponents [
            "cargo"
            "clippy"
            "rust-src"
            "rustc"
            "rustfmt"
            "rust-analyzer"
          ];

          # Build one standalone cargo crate from a subpath of this flake's
          # source tree. crateDir "." is the flake root (the hook crate).
          # A subpath builds one crate from the tree. Intra-repo path deps
          # resolve because src keeps the repo layout intact.
          # The output binary name comes from the crate's [[bin]] name.
          buildCrate = { crateDir, crateName }:
            let
              crateSrc = if crateDir == "." then self else "${self}/${crateDir}";
            in
            pkgs.rustPlatform.buildRustPackage {
              pname = crateName;
              version = "0.1.0";
              src = crateSrc;
              nativeBuildInputs = [ rustToolchain ];
              cargoLock = { lockFile = "${crateSrc}/Cargo.lock"; };
              doCheck = false;
            };

          # UI-ext wrapper: $out/<extName>/ext.toml + <extName>/<binDir>/<bin>.
          # binDir must equal the relative command path in ext.toml,
          # because the TUI resolves command against the ext entry dir.
          # buildRustPackage is a release build, so the default is
          # "target/release".
          wrapAsExt = { extName, extToml, built, binDir ? "target/release" }:
            pkgs.stdenv.mkDerivation {
              name = "${extName}-ui-ext";
              src = self;
              nativeBuildInputs = [ built ];
              installPhase = ''
                mkdir -p $out/${extName}/${binDir}
                cp ${extToml} $out/${extName}/ext.toml
                cp -rL ${built}/bin/. $out/${extName}/${binDir}/
              '';
              # meta.rushi.ext (rushi#13): the UI-ext entry dir this
              # package provides. lib.mkRushi reads it at eval time.
              meta = { rushi = { ext = extName; }; };
            };
          # Plain-named bindings so the `default` attr can reference them
          # (hyphenated attr names cannot be referenced by bare identifier).
          #
          # Base hook package (guide §4.2): a bare buildRustPackage result is
          # a valid hook source. $out/bin/harness-hook-simple-english →
          # mkRushi copies bin/. → hooks/. The binary name comes from the
          # crate's [[bin]] name and must match the command field in
          # config.toml [[hooks.on]].
          hookBase = buildCrate {
            crateDir = ".";
            crateName = "hook-simple-english";
          };
          # meta.rushi.bin (rushi#13): the runtime binary name, so a consumer
          # can derive hook commands without a second typed copy. The merge
          # preserves any meta buildRustPackage already set on the package.
          hookPkg = hookBase // {
            meta = (hookBase.meta or { }) // {
              rushi = { bin = "harness-hook-simple-english"; };
            };
          };
          extPkg = wrapAsExt {
            # TUI extension: wrap into $out/simple-english/ext.toml +
            # $out/simple-english/target/release/simple-english-ext.
            # ext.toml command = "target/release/simple-english-ext" matches
            # the default binDir.
            extName = "simple-english";
            extToml = "${self}/simple-english-ext/ext.toml";
            built = buildCrate {
              crateDir = "simple-english-ext";
              crateName = "simple-english-ext";
            };
          };
        in
        {
          "hook-simple-english" = hookPkg;
          "simple-english-ext" = extPkg;
          default = hookPkg;
        }
      );

      # ── Dev shell: in-tree cargo dev for both crates. ──
      devShells = pkgLib.genAttrs supportedSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ fenix.overlays.default ];
          };
          rustToolchain = fenix.packages.${system}.stable.withComponents [
            "cargo"
            "clippy"
            "rust-src"
            "rustc"
            "rustfmt"
            "rust-analyzer"
          ];
        in
        {
          default = pkgs.mkShell {
            buildInputs = [
              rustToolchain
              pkgs.jq
              pkgs.python3
            ];
            shellHook = ''
              echo "rushi-simple-english dev shell: rust on PATH."
              echo "Build the hook:  cargo build --release"
              echo "Build the ext:  cd simple-english-ext && cargo build --release"
            '';
          };
        }
      );
    };
}
