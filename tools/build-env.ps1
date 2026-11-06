# Shared by install.ps1 and build-setup.ps1.
# Rust writes the source paths of libraries into the program (for error messages), and those live
# under the user's folder: C:\Users\<name>\.cargo\... . Replace that part with "~" so the
# program carries no user name. CARGO_ENCODED_RUSTFLAGS (0x1F-separated) allows spaces in the path.
$env:CARGO_ENCODED_RUSTFLAGS = "--remap-path-prefix=$env:USERPROFILE=~"
