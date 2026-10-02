{
  description = "guitar — pitch monitor for tuning a guitar and singing (gpui)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        lib = pkgs.lib;

        # pipewire-rs generates bindings with bindgen, so it needs libclang + headers.
        nativeBuildInputs = with pkgs; [ pkg-config rustPlatform.bindgenHook ];
        # libxkbcommon: gpui links it directly.
        buildInputs = with pkgs; [ pipewire libxkbcommon ];

        # gpui loads these at runtime with dlopen.
        gpuLibs = with pkgs; [ vulkan-loader wayland libxkbcommon ];
        # Use nixpkgs' Mesa Vulkan drivers: the host's are built against a different libc,
        # which is what usually breaks Nix GPU apps on non-NixOS systems.
        vkDriverFiles = lib.concatMapStringsSep ":" (f: "${pkgs.mesa}/share/vulkan/icd.d/${f}") [
          "intel_icd.x86_64.json"
          "radeon_icd.x86_64.json"
          "nouveau_icd.x86_64.json"
          "lvp_icd.x86_64.json"
        ];

        guitar = pkgs.rustPlatform.buildRustPackage {
          pname = "guitar";
          version = "0.1.0";
          src = lib.fileset.toSource {
            root = ./.;
            fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ./packaging ];
          };
          cargoLock.lockFile = ./Cargo.lock;
          inherit buildInputs;
          nativeBuildInputs = nativeBuildInputs ++ [ pkgs.makeWrapper ];
          # The input name comes from wpctl; prefer the system's, fall back to ours.
          # GPU libraries go in the rpath rather than LD_LIBRARY_PATH so they don't leak into
          # wpctl. (postFixup, because fixup shrinks the rpath to what's linked, dropping
          # dlopen'd libs.)
          postFixup = ''
            patchelf --add-rpath ${lib.makeLibraryPath gpuLibs} $out/bin/guitar
            wrapProgram $out/bin/guitar \
              --suffix PATH : ${lib.makeBinPath [ pkgs.wireplumber ]} \
              --set-default VK_DRIVER_FILES ${vkDriverFiles}

            install -Dm644 packaging/guitar.desktop $out/share/applications/guitar.desktop
            substituteInPlace $out/share/applications/guitar.desktop --replace-fail @guitar@ $out/bin/guitar
            install -Dm644 packaging/guitar.svg $out/share/icons/hicolor/scalable/apps/guitar.svg
          '';
          meta.mainProgram = "guitar";
        };
      in
      {
        packages.default = guitar;
        apps.default = flake-utils.lib.mkApp { drv = guitar; };

        devShells.default = pkgs.mkShell {
          inherit buildInputs;
          nativeBuildInputs = nativeBuildInputs
            ++ (with pkgs; [ cargo rustc rust-analyzer clippy rustfmt ]);
          LD_LIBRARY_PATH = lib.makeLibraryPath gpuLibs;
          VK_DRIVER_FILES = vkDriverFiles;
        };
      });
}
