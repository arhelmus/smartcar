# syntax=docker/dockerfile:1
# =============================================================================
# Multi-stage Docker build for the OpenAuto Android Auto head unit emulator.
#
# Build context is the repo root (see docker-compose.yml).
# Submodules must be checked out before building:
#   git submodule update --init server/third_party/aasdk
#   git submodule update --init server/third_party/openauto
#
# The build proceeds in three stages:
#   1. "aasdk-builder"   – builds libaasdk and installs it to /usr/local
#   2. "openauto-builder"– builds the openauto autoapp binary against libaasdk
#   3. "runtime"         – minimal image that ships only the binary + runtime libs
# =============================================================================

ARG UBUNTU_VERSION=22.04

# ---------------------------------------------------------------------------
# Stage 1 – build libaasdk (dependency of openauto)
# ---------------------------------------------------------------------------
FROM ubuntu:${UBUNTU_VERSION} AS aasdk-builder

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        cmake \
        ninja-build \
        pkg-config \
        git \
        # protobuf – aasdk uses it for the Android Auto protocol
        libprotobuf-dev \
        protobuf-compiler \
        # crypto / USB
        libssl-dev \
        libusb-1.0-0-dev \
        # Boost (system + log are used by aasdk)
        libboost-system-dev \
        libboost-log-dev \
        libboost-test-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src/aasdk

# Copy only the aasdk submodule so changes to the rest of the repo don't
# invalidate this expensive layer.
COPY server/third_party/aasdk/ .

# Build aasdk in Release mode and install to /usr/local so openauto can find
# it via the standard cmake search paths.
# SKIP_BUILD_PROTOBUF / SKIP_BUILD_ABSL tell aasdk's CMake to use the
# system-installed versions rather than bundling its own copies.
RUN cmake -S . -B build-release \
        -GNinja \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_INSTALL_PREFIX=/usr/local \
        -DAASDK_TEST=OFF \
        -DAASDK_BENCHMARK=OFF \
        -DSKIP_BUILD_PROTOBUF=ON \
        -DSKIP_BUILD_ABSL=ON \
    && cmake --build build-release -j"$(nproc)" \
    && cmake --install build-release

# ---------------------------------------------------------------------------
# Stage 2 – build openauto
# ---------------------------------------------------------------------------
FROM ubuntu:${UBUNTU_VERSION} AS openauto-builder

ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        cmake \
        ninja-build \
        pkg-config \
        git \
        # protobuf
        libprotobuf-dev \
        protobuf-compiler \
        # crypto / USB / misc
        libssl-dev \
        libusb-1.0-0-dev \
        # Boost
        libboost-system-dev \
        libboost-log-dev \
        libboost-log1.74-dev \
        # Qt5 – multimedia + Bluetooth + DBus + Network are all required by openauto
        qtbase5-dev \
        qtmultimedia5-dev \
        qttools5-dev \
        qttools5-dev-tools \
        qtconnectivity5-dev \
        libqt5dbus5 \
        # GStreamer backend for Qt Multimedia (required for H.264 video decode)
        gstreamer1.0-tools \
        gstreamer1.0-plugins-base \
        gstreamer1.0-plugins-good \
        gstreamer1.0-plugins-bad \
        gstreamer1.0-libav \
        libgstreamer1.0-dev \
        libgstreamer-plugins-base1.0-dev \
        # Audio: RtAudio is used for microphone / speaker routing
        librtaudio-dev \
        # Tag library (media metadata, listed in openauto CMakeLists.txt)
        libtag1-dev \
        # Block device detection (for drive/USB detection)
        libblkid-dev \
        # GPS (gpsd client)
        libgps-dev \
    && rm -rf /var/lib/apt/lists/*

# Bring in the installed aasdk artifacts (headers + libs) from stage 1.
COPY --from=aasdk-builder /usr/local /usr/local

# Refresh the dynamic linker cache so cmake can find libaasdk.
RUN ldconfig

WORKDIR /src/openauto
COPY server/third_party/openauto/ .

# Build with NOPI=ON (no Raspberry Pi hardware; required for x86/amd64).
# The binary lands at build-release/bin/autoapp.
RUN cmake -S . -B build-release \
        -GNinja \
        -DCMAKE_BUILD_TYPE=Release \
        -DNOPI=ON \
    && cmake --build build-release -j"$(nproc)"

# ---------------------------------------------------------------------------
# Stage 3 – minimal runtime image
# ---------------------------------------------------------------------------
FROM ubuntu:${UBUNTU_VERSION} AS runtime

ARG DEBIAN_FRONTEND=noninteractive

# Install only the shared-library runtime deps (no -dev headers needed).
RUN apt-get update && apt-get install -y --no-install-recommends \
        # Qt5 runtime
        libqt5core5a \
        libqt5gui5 \
        libqt5widgets5 \
        libqt5multimedia5 \
        libqt5multimediawidgets5 \
        libqt5bluetooth5 \
        libqt5network5 \
        libqt5dbus5 \
        libqt5multimedia5-plugins \
        # GStreamer video / audio decoding
        gstreamer1.0-plugins-base \
        gstreamer1.0-plugins-good \
        gstreamer1.0-plugins-bad \
        gstreamer1.0-plugins-ugly \
        gstreamer1.0-libav \
        # Runtime audio
        librtaudio6 \
        # Tag library
        libtag1v5 \
        # Block-device / GPS / USB
        libblkid1 \
        libgps28 \
        libusb-1.0-0 \
        # Crypto
        libssl3 \
        # Boost runtime
        libboost-system1.74.0 \
        libboost-log1.74.0 \
        # protobuf runtime
        libprotobuf23 \
        # X11 client utilities for DISPLAY check
        x11-utils \
    && rm -rf /var/lib/apt/lists/*

# Copy installed aasdk shared libraries from the builder stage.
COPY --from=aasdk-builder /usr/local/lib /usr/local/lib
COPY --from=aasdk-builder /usr/local/include /usr/local/include

# Copy the compiled autoapp binary.
COPY --from=openauto-builder /src/openauto/build-release/bin/autoapp /usr/local/bin/autoapp

# Copy the openauto assets (icons, UI files, etc.) that autoapp may load at
# runtime from its working directory or a well-known path.
COPY --from=openauto-builder /src/openauto/assets /opt/openauto/assets

RUN ldconfig

# Entrypoint script handles DISPLAY setup and graceful shutdown.
COPY docker/entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

WORKDIR /opt/openauto

ENTRYPOINT ["/entrypoint.sh"]
