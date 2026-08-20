# Linux Glimpse 네이티브 바이너리 빌드 컨테이너.
# 사용: docker build -f native/docker/linux-build.Dockerfile -t glimpse-linux-build .
#      docker run --rm -v "$PWD/native/src/linux:/src" -v "$PWD/native/linux:/out" glimpse-linux-build
# (GTK4>=4.12 + WebKitGTK 6.0 요구 — Ubuntu 24.04 기준)
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        curl ca-certificates pkg-config build-essential git \
        libgtk-4-dev libwebkitgtk-6.0-dev libsoup-3.0-dev \
        libgraphene-1.0-dev libssl-dev \
        meson ninja-build libwayland-dev gobject-introspection libgirepository1.0-dev \
    && rm -rf /var/lib/apt/lists/*

# gtk4-layer-shell — Ubuntu에 패키지가 없어 소스 빌드 (Wayland layer-shell용)
RUN git clone --depth 1 https://github.com/wmww/gtk4-layer-shell.git /tmp/ls \
    && meson setup /tmp/ls/build /tmp/ls -Dexamples=false -Dtests=false -Ddocs=false -Dvapi=false \
    && ninja -C /tmp/ls/build install \
    && ldconfig \
    && rm -rf /tmp/ls

# Rust 설치
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
ENV PATH="/root/.cargo/bin:${PATH}"

# git 의존성(gtk-rs/gir-files 등)이 ssh URL로 fetch되므로 https로 재작성
RUN git config --global url."https://github.com/".insteadOf "git@github.com:"

WORKDIR /build
# 소스는 볼륨으로 마운트: -v $PWD/native/src/linux:/build
# 산출은 볼륨으로 추출: -v $PWD/native/linux:/out
CMD ["sh", "-c", "cargo build --release && cp target/release/glimpse /out/glimpse && echo '✓ /out/glimpse'"]