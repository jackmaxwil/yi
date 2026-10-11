# The term host for scripts/tui_see.py: real Ghostty under Xvfb with software GL, so a scene's
# keyframes carry kitty images and true colour. Ghostty comes from the mkasberg Ubuntu packages.
FROM ubuntu:24.04
ARG TARGETARCH
ARG GHOSTTY=1.3.1-0.ppa2
ARG SHA256_amd64=478d440153ef544426418efc7d6d8901715359f452c46be29071901a94b8cd47
ARG SHA256_arm64=91063815b6ce3d834d59714b4ad0310f744448b6716836d035b3d331d1923363
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl xvfb xauth mesa-utils libgl1-mesa-dri libegl1 \
      ffmpeg python3 python3-pyte fonts-jetbrains-mono git \
 && curl -fsSL -o /tmp/ghostty.deb \
      "https://github.com/mkasberg/ghostty-ubuntu/releases/download/${GHOSTTY%.ppa2}-ppa2/ghostty_${GHOSTTY}_${TARGETARCH}_24.04.deb" \
 && if [ "$TARGETARCH" = arm64 ]; then sum=$SHA256_arm64; else sum=$SHA256_amd64; fi \
 && echo "$sum  /tmp/ghostty.deb" | sha256sum -c - \
 && apt-get install -y --no-install-recommends /tmp/ghostty.deb \
 && rm -rf /var/lib/apt/lists/* /tmp/ghostty.deb \
 && git config --system --add safe.directory '*'
ENV LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe
