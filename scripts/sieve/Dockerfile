FROM debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8
RUN printf '#!/bin/sh\nexit 101\n' > /usr/sbin/policy-rc.d && chmod +x /usr/sbin/policy-rc.d && apt-get update && apt-get install -y --no-install-recommends dovecot-sieve ca-certificates curl make g++ python3 && rm -rf /var/lib/apt/lists/*
ARG CHECK_SIEVE_REV=3a99728878994a7f6f2f14032013fc0f06bacbc1
RUN mkdir /build && curl -fsSL "https://github.com/dburkart/check-sieve/archive/${CHECK_SIEVE_REV}.tar.gz" | tar xz --strip-components=1 -C /build && make -C /build -j2 && install /build/check-sieve /usr/local/bin/check-sieve && rm -rf /build
RUN useradd -m -u 1000 proof
USER proof
ENV HOME=/tmp
ENTRYPOINT ["python3", "/harness/compare.py"]
