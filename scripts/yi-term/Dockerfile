# The term host for scripts/tui_see.py: real Ghostty, kitty and WezTerm under Xvfb with software GL.
# Ghostty is mkasberg's Ubuntu package, kitty Ubuntu's own, WezTerm its official 22.04 build.
FROM ubuntu:24.04
ARG TARGETARCH
ARG GHOSTTY=1.3.1-0.ppa2
ARG SHA256_amd64=478d440153ef544426418efc7d6d8901715359f452c46be29071901a94b8cd47
ARG SHA256_arm64=91063815b6ce3d834d59714b4ad0310f744448b6716836d035b3d331d1923363
ARG WEZTERM=20240203-110809-5046fc22
ARG WEZ_amd64=Ubuntu22.04.deb
ARG WEZ_arm64=Ubuntu22.04.arm64.deb
# amd64's sum is the release's own .sha256; arm64 ships none, so this one was measured on download.
ARG WEZ_SHA256_amd64=86358dab5794a4fb63f7c91dd68d4fdc3da58faad648a58fc77d2bd51c7b0686
ARG WEZ_SHA256_arm64=3e64e618601f254795f7d18a41d8dda27092ddc4e7f91e017492156c48f9798b
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl xvfb xauth mesa-utils libgl1-mesa-dri libegl1 \
      ffmpeg python3 python3-pyte fonts-jetbrains-mono git kitty \
 && curl -fsSL -o /tmp/ghostty.deb \
      "https://github.com/mkasberg/ghostty-ubuntu/releases/download/${GHOSTTY%.ppa2}-ppa2/ghostty_${GHOSTTY}_${TARGETARCH}_24.04.deb" \
 && if [ "$TARGETARCH" = arm64 ]; then sum=$SHA256_arm64; else sum=$SHA256_amd64; fi \
 && echo "$sum  /tmp/ghostty.deb" | sha256sum -c - \
 && if [ "$TARGETARCH" = arm64 ]; then wez=$WEZ_arm64 sum=$WEZ_SHA256_arm64; \
    else wez=$WEZ_amd64 sum=$WEZ_SHA256_amd64; fi \
 && curl -fsSL -o /tmp/wezterm.deb \
      "https://github.com/wez/wezterm/releases/download/${WEZTERM}/wezterm-${WEZTERM}.${wez}" \
 && echo "$sum  /tmp/wezterm.deb" | sha256sum -c - \
 && apt-get install -y --no-install-recommends /tmp/ghostty.deb /tmp/wezterm.deb \
 && rm -rf /var/lib/apt/lists/* /tmp/ghostty.deb /tmp/wezterm.deb \
 && git config --system --add safe.directory '*'
ENV LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe
