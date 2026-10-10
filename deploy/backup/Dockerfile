# Same pinned PG18 tools as the database: independently restorable custom-format dumps.
FROM postgres:18.6@sha256:5a5a84b19854a9ffaa54082c166ff4ec27473a361e496e5ea167f298f2da9722
RUN apt-get update && apt-get install -y --no-install-recommends age python3 tzdata bash openssl \
    && rm -rf /var/lib/apt/lists/*
COPY backup /opt/porchlight/backup
COPY lib /opt/porchlight/lib
ENTRYPOINT ["/bin/bash", "/opt/porchlight/backup/backup.sh"]
CMD ["daemon"]
