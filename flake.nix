{
  description = "Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      # The toolchain channel comes from rust-toolchain.toml.
      overrides = builtins.fromTOML (builtins.readFile ./rust-toolchain.toml);
      rustChannel = overrides.toolchain.channel;
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          # rustup names toolchain dirs "<channel>-<host-triple>".
          hostTriple = pkgs.stdenv.hostPlatform.rust.rustcTarget;
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo-edit
              cargo-watch
              rustup
            ];

            # rustup installs the toolchain from rust-toolchain.toml into
            # $RUSTUP_HOME on first use. Put cargo and the toolchain on PATH.
            shellHook = ''
              export PATH="''${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
              export PATH="''${RUSTUP_HOME:-$HOME/.rustup}/toolchains/${rustChannel}-${hostTriple}/bin:$PATH"
            '';
          };
        }
      );
    };
}
