{
  description = "GitPulse – a TUI dashboard for monitoring git repository status";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      rust-overlay,
      ...
    }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = fn: nixpkgs.lib.genAttrs systems fn;

      mkPkgs = system:
        import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

      mkCraneLib = pkgs:
        let
          toolchain = pkgs.rust-bin.stable.latest.minimal.override {
            extensions = [ "clippy" ];
          };
        in
        (crane.mkLib pkgs).overrideToolchain toolchain;
    in
    {
      packages = forAllSystems (system:
        let
          pkgs = mkPkgs system;
          craneLib = mkCraneLib pkgs;
          src = craneLib.cleanCargoSource ./.;
          commonArgs = {
            inherit src;
            pname = "gitocular";
            strictDeps = true;
            nativeCheckInputs = [ pkgs.git ];
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
          gitocular = craneLib.buildPackage (commonArgs // {
            inherit cargoArtifacts;
            meta = {
              description = "A TUI dashboard for monitoring git repository status";
              homepage = "https://github.com/krfl/gitocular";
              license = pkgs.lib.licenses.asl20;
              mainProgram = "gitocular";
            };
          });
        in
        {
          default = gitocular;
          inherit gitocular;
        }
      );

      checks = forAllSystems (system:
        let
          pkgs = mkPkgs system;
          craneLib = mkCraneLib pkgs;
          src = craneLib.cleanCargoSource ./.;
          commonArgs = {
            inherit src;
            pname = "gitocular";
            strictDeps = true;
            nativeCheckInputs = [ pkgs.git ];
          };
          cargoArtifacts = craneLib.buildDepsOnly commonArgs;
        in
        {
          gitocular = self.packages.${system}.default;

          gitocular-clippy = craneLib.cargoClippy (commonArgs // {
            inherit cargoArtifacts;
            cargoClippyExtraArgs = "--workspace -- -D warnings";
          });

          gitocular-tests = craneLib.cargoTest (commonArgs // {
            inherit cargoArtifacts;
          });
        }
      );

      devShells = forAllSystems (system:
        let
          pkgs = mkPkgs system;
          toolchain = pkgs.rust-bin.stable.latest.default.override {
            extensions = [
              "rust-src"
              "rust-analyzer"
              "clippy"
            ];
          };
        in
        {
          default = pkgs.mkShell {
            packages = [
              toolchain
              pkgs.cargo-watch
              pkgs.git
            ];
          };
        }
      );

      overlays.default = final: _prev: {
        gitocular = self.packages.${final.system}.default;
      };
    };
}
