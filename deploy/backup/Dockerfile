# Exact PG18.6 clients from the same immutable image as the database, without server binaries/layers.
FROM postgres:18.6@sha256:5a5a84b19854a9ffaa54082c166ff4ec27473a361e496e5ea167f298f2da9722 AS client-tools
COPY backup/client-tools.sh /client-tools.sh
RUN bash /client-tools.sh

FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
RUN apt-get update && apt-get install -y --no-install-recommends age python3 tzdata bash openssl \
    && rm -rf /var/lib/apt/lists/*
COPY --from=client-tools /client-root/ /
RUN set -eu; for tool in pg_dump psql pg_restore; do \
      "$tool" --version | grep -E "^$tool \\(PostgreSQL\\) 18[.]6( |$)"; \
    done; ! command -v postgres; test ! -e /usr/lib/postgresql/18/bin/postgres; \
    python3 -I -c 'import ssl, zoneinfo; zoneinfo.ZoneInfo("America/New_York")'
COPY backup /opt/porchlight/backup
COPY lib /opt/porchlight/lib
ENTRYPOINT ["/bin/bash", "/opt/porchlight/backup/backup.sh"]
CMD ["daemon"]
