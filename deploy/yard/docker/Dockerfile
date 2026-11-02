# The one image behind the yard, litestream and ops services (compose.yml): the operating scripts, Litestream, rclone,
# sqlite3 and curl, and the site (site/). Ubuntu 24.04 is the base because the workflows' runner is: yard needs the same
# glibc or newer. docs/registry.md, "Running on Docker".
#
# Where yard comes from:
#   - the published image (ghcr.io/<owner>/codeyard-yard, the yard-image workflow) has it built in: that workflow puts
#     the binary beside this file and builds with --build-arg YARD=included;
#   - a local build (YARD=none, the default) has none, and the stack mounts ./bin instead.
# Either way ./bin wins when it holds a yard (it is first on PATH), so host/install can still swap the binary in place.
ARG YARD=none

FROM scratch AS yard-none
FROM scratch AS yard-included
COPY --chmod=755 yard /yard
FROM yard-${YARD} AS yard

FROM ubuntu:24.04@sha256:fe81560990fae86dc4743202b2cb4d3df98d0bcf949173cecef15ce1127ef852

ARG TARGETARCH
# Pinned by version and checksum, per architecture (the server is amd64, a developer's Mac arm64).
ARG LITESTREAM_VERSION=0.5.17
ARG LITESTREAM_SHA256_AMD64=cfb371176d164437ae869f8351cfde49bd1804ae71c61923f75c9cba9c9c006d
ARG LITESTREAM_SHA256_ARM64=f8ca4a050095c1efbda2c4365172e61bf9d955ea0d9ac42f448b52e51819baa5
ARG RCLONE_VERSION=1.75.2
ARG RCLONE_SHA256_AMD64=349ac8fba6ff65d6247043f1750cdcb518ec5d500ef91463a10d37c0ccdf3702
ARG RCLONE_SHA256_ARM64=7e1e8d69654941b7b7df84ee74c5f7fb09cce7d5496947fb7bf16b55ff427d10

RUN set -eu; \
	export DEBIAN_FRONTEND=noninteractive; \
	apt-get update; \
	apt-get install -y --no-install-recommends ca-certificates curl jq sqlite3 unzip; \
	case "$TARGETARCH" in \
	amd64) ls_arch=x86_64; ls_sha=$LITESTREAM_SHA256_AMD64; rc_arch=amd64; rc_sha=$RCLONE_SHA256_AMD64 ;; \
	arm64) ls_arch=arm64; ls_sha=$LITESTREAM_SHA256_ARM64; rc_arch=arm64; rc_sha=$RCLONE_SHA256_ARM64 ;; \
	*) echo "no Litestream or rclone pinned for $TARGETARCH" >&2; exit 1 ;; \
	esac; \
	curl -fsSL -o /tmp/litestream.tgz "https://github.com/benbjohnson/litestream/releases/download/v$LITESTREAM_VERSION/litestream-$LITESTREAM_VERSION-linux-$ls_arch.tar.gz"; \
	echo "$ls_sha  /tmp/litestream.tgz" | sha256sum -c -; \
	tar -xzf /tmp/litestream.tgz -C /usr/local/bin litestream; \
	curl -fsSL -o /tmp/rclone.zip "https://github.com/rclone/rclone/releases/download/v$RCLONE_VERSION/rclone-v$RCLONE_VERSION-linux-$rc_arch.zip"; \
	echo "$rc_sha  /tmp/rclone.zip" | sha256sum -c -; \
	unzip -j -o -d /usr/local/bin /tmp/rclone.zip '*/rclone'; \
	chmod 755 /usr/local/bin/litestream /usr/local/bin/rclone; \
	apt-get purge -y --auto-remove unzip; \
	rm -rf /var/lib/apt/lists/* /tmp/*

# One unprivileged user for every service that touches the registry's folder. A named volume takes this ownership from
# the image; a bind mount on the server is given it by host/setup-host.sh.
RUN groupadd --gid 10001 yard \
	&& useradd --uid 10001 --gid 10001 --no-create-home --home-dir /srv/yard --shell /usr/sbin/nologin yard \
	&& install -d -o yard -g yard -m 750 /srv/yard /work

COPY --chmod=755 ops/ /usr/local/bin/
COPY --chmod=644 litestream.yml /etc/litestream.yml
COPY site/ /opt/yard/site/
COPY --from=yard / /opt/yard/included/

ENV PATH=/opt/yard/bin:/opt/yard/included:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin
USER 10001:10001
WORKDIR /srv/yard
