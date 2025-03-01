{ pkgs ? import <nixpkgs> { }
}:
with pkgs;

mkShell ({
  nativeBuildInputs = [
    rustup
    rustfmt
    pkg-config
    openssl
  ];

})
