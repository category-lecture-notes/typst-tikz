{
  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs }:
    let
      inherit (builtins)
        substring
        ;
      inherit (nixpkgs.lib)
        genAttrs
        importTOML
        optionals
        ;

      eachSystem = f: genAttrs
        [
          "aarch64-darwin"
          "aarch64-linux"
          "x86_64-darwin"
          "x86_64-linux"
        ]
        (system: f nixpkgs.legacyPackages.${system});

      rev = fallback:
        if self ? rev then
          substring 0 8 self.rev
        else
          fallback;

      packageFor = pkgs: pkgs.rustPlatform.buildRustPackage {
        pname = "typst-tikz";
        version = rev "00000000";

        src = self;

        cargoLock = {
          lockFile = ./Cargo.lock;
          allowBuiltinFetchGit = true;
        };

        nativeBuildInputs = [
          pkgs.installShellFiles
        ];

        buildInputs = optionals pkgs.stdenv.isDarwin [
          pkgs.darwin.apple_sdk.frameworks.CoreServices
        ];
       
        propagatedBuildInputs = with pkgs; [
          pdf2svg
          (texlive.combine {
            inherit (texlive) scheme-basic
            luatex85 standalone pgf tikz-cd;
          })
        ];

        GEN_ARTIFACTS = "artifacts";
        TYPST_VERSION = "${(importTOML ./Cargo.toml).package.version} (${rev "unknown hash"})";
      };
    in
    {
      devShells = eachSystem (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            clippy
            pdf2svg
            rust-analyzer
            rustc
            rustfmt
            (texlive.combine {
              inherit (pkgs.texlive) scheme-basic
              luatex85 standalone pgf tikz-cd;
            })
          ];

          buildInputs = optionals pkgs.stdenv.isDarwin [
            pkgs.darwin.apple_sdk.frameworks.CoreServices
            pkgs.libiconv
          ];

          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
        };
      });

      formatter = eachSystem (pkgs: pkgs.nixpkgs-fmt);

      overlays.default = _: prev: {
        typst-dev = packageFor prev;
      };

      packages = eachSystem (pkgs: {
        default = packageFor pkgs;
      });
    };
}
