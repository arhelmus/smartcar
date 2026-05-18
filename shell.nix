{ pkgs ? import <nixpkgs> {} }:

let
  # openauto's CMakeLists.txt does find_package(blkid REQUIRED) but no source
  # file actually includes blkid.h — it's a dead cmake dependency inherited from
  # the Linux build. Provide an empty stub so the finder is satisfied.
  blkidStub = pkgs.runCommandCC "blkid-stub" {} ''
    mkdir -p $out/include/blkid $out/lib
    touch $out/include/blkid/blkid.h
    echo "" | $CC -x c - -c -o $TMPDIR/empty.o
    ar rcs $out/lib/libblkid.a $TMPDIR/empty.o
  '';
in

pkgs.mkShell {
  name = "smartcar-openauto";

  buildInputs = with pkgs; [
    # Build tools
    cmake
    ninja
    pkg-config
    python3
    git

    # Boost 1.82 — oldest nixpkgs version with good C++17 patches
    boost182

    # Core C++ deps
    openssl
    libusb1
    protobuf
    abseil-cpp
    rtaudio
    taglib
    gpsd

    # Qt 5
    qt5.qtbase
    qt5.qtmultimedia
    qt5.qtconnectivity

    # GStreamer
    gst_all_1.gstreamer
    gst_all_1.gst-plugins-base
    gst_all_1.gst-plugins-good
    gst_all_1.gst-plugins-bad
    gst_all_1.gst-libav

    blkidStub
  ] ++ pkgs.lib.optionals pkgs.stdenv.isLinux [
    util-linux   # real blkid on Linux
  ];

  # Export the stub prefix so the Python script can pass it to cmake.
  BLKID_STUB = blkidStub;

  shellHook = ''
    if [[ "$PATH" == */homebrew* || "$PATH" == */usr/local/bin* ]]; then
      echo "WARNING: Homebrew on PATH — run with 'nix-shell --pure' for full isolation." >&2
    fi
    # Expose Qt platform plugins (cocoa) at runtime.
    export QT_QPA_PLATFORM_PLUGIN_PATH="${pkgs.qt5.qtbase.bin}/lib/qt-${pkgs.qt5.qtbase.version}/plugins/platforms"

    # GStreamer plugin path — pinned to the exact Nix closure so the Qt
    # GStreamer backend finds an H.264 decoder (avdec_h264 from gst-libav;
    # vtdec from gst-plugins-bad/applemedia).  Globbing /nix/store is
    # unreliable: it depends on what is realised and survives GC.
    export GST_PLUGIN_PATH="${pkgs.gst_all_1.gstreamer}/lib/gstreamer-1.0:${pkgs.gst_all_1.gst-plugins-base}/lib/gstreamer-1.0:${pkgs.gst_all_1.gst-plugins-good}/lib/gstreamer-1.0:${pkgs.gst_all_1.gst-plugins-bad}/lib/gstreamer-1.0:${pkgs.gst_all_1.gst-libav}/lib/gstreamer-1.0"

    # NOTE: the runtime-only plugin packages (esp. the H.264 decoder) are
    # pinned with GC roots by scripts/run_openauto.py, not here — `nix-shell
    # --pure` strips nix-store from PATH so gcroot creation must happen
    # outside the pure shell.  Without those roots `nix-collect-garbage`
    # silently removes the decoder and video projection regresses.

    echo "smartcar Nix shell ready. Run:"
    echo "  python3 scripts/run_openauto.py [--clean] [--attached]"
  '';
}
