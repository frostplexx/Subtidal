{
  description = "Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    fenix = {
      url = "github:nix-community/fenix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { self, nixpkgs, fenix }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          toolchain = with fenix.packages.${system}; combine [
            stable.cargo
            stable.rustc
            stable.clippy
            stable.rustfmt
            stable.rust-analyzer
          ];
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              toolchain
              cargo-edit
              cargo-watch
            ];
          };
        }
      );
    };
}
