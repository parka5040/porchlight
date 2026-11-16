"""Shared stdlib-only safety, histograms and incremental SSE framing."""
import collections
import hashlib
import json
import math
import os
from pathlib import Path
import stat
import zlib


class BodyDecoder:
    """Bounded gzip decoding; wire bytes mean HTTP entity bytes, excluding TLS/headers."""
    def __init__(self, encoding, cap=512*1024*1024):
        encoding = (encoding or 'identity').lower().strip()
        if encoding not in ('identity', 'gzip'): raise ValueError('unsupported content encoding')
        self.decoder = zlib.decompressobj(16+zlib.MAX_WBITS) if encoding == 'gzip' else None
        self.wire_bytes = self.decoded_bytes = 0
        self.cap = cap

    def feed(self, chunk):
        self.wire_bytes += len(chunk)
        if self.wire_bytes > self.cap: raise ValueError('wire body cap')
        try:
            out = self.decoder.decompress(chunk, self.cap-self.decoded_bytes+1) if self.decoder else chunk
        except zlib.error:
            raise ValueError('invalid gzip body') from None
        self.decoded_bytes += len(out)
        if self.decoded_bytes > self.cap: raise ValueError('decoded body cap')
        if self.decoder and self.decoder.unused_data: raise ValueError('trailing gzip data')
        return out

    def finish(self):
        if self.decoder and not self.decoder.eof: raise ValueError('truncated gzip body')


def runtime_dir(path):
    p = Path(path).absolute()
    resolved = p.resolve()
    repo = Path(__file__).resolve().parents[3]
    in_tree_wrong = resolved.is_relative_to(repo) and not resolved.is_relative_to(repo / 'var')
    if in_tree_wrong or 'var' not in resolved.parts or resolved.name == 'var' or resolved != p:
        raise ValueError('output must be below a real var/ directory (no symlinks or ..)')
    return resolved


def private_file(path):
    p = Path(path).absolute()
    try:
        s = p.lstat()
    except OSError:
        raise ValueError('private file missing') from None
    if p.resolve() != p or not stat.S_ISREG(s.st_mode) or stat.S_IMODE(s.st_mode) != 0o600 or not s.st_size:
        raise ValueError('expected nonempty regular 0600 file without symlinks')
    return p


def write_private(path, text):
    p = Path(path)
    runtime_dir(p.parent)
    p.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    flags = os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW
    fd = os.open(p, flags, 0o600)
    with os.fdopen(fd, 'wb') as f:
        os.fchmod(f.fileno(), 0o600)
        f.write(text.encode() if isinstance(text, str) else text)
        f.flush()
        os.fsync(f.fileno())


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


class Histogram:
    """Upper-rounded 1% logarithmic millisecond buckets; bounded storage, nearest rank."""
    def __init__(self):
        self.buckets = collections.Counter()
        self.count = 0
        self.maximum = 0

    def add(self, ms):
        if not math.isfinite(ms) or ms < 0:
            raise ValueError('invalid duration')
        key = math.ceil(math.log(max(ms, 0.001) / 0.001, 1.01))
        self.buckets[key] += 1
        self.count += 1
        self.maximum = max(self.maximum, ms)

    def percentile(self, p):
        if not self.count:
            return None
        rank = math.ceil(p * self.count / 100)
        seen = 0
        for k, n in sorted(self.buckets.items()):
            seen += n
            if seen >= rank:
                return min(self.maximum, 0.001 * 1.01 ** k)

    def summary(self):
        return {'count': self.count, 'p50_ms': self.percentile(50), 'p95_ms': self.percentile(95),
                'p99_ms': self.percentile(99), 'max_ms': self.maximum if self.count else None}

    def as_dict(self):
        return dict(self.summary(), buckets=dict(self.buckets), bucket_base=1.01, bucket_unit_ms=0.001)

    @classmethod
    def from_dict(cls, value):
        h = cls()
        h.buckets = collections.Counter({int(k): n for k, n in value['buckets'].items()})
        h.count = sum(h.buckets.values())
        h.maximum = value['max_ms'] or 0
        return h


class SSEParser:
    def __init__(self):
        self.buffer = b''
        self.fields = {}
        self.data = []
        self.frame_size = 0

    def feed(self, chunk):
        self.buffer += chunk
        frames = []
        while b'\n' in self.buffer:
            raw, self.buffer = self.buffer.split(b'\n', 1)
            self.frame_size += len(raw) + 1
            if len(raw) > 65536 or self.frame_size > 262144:
                raise ValueError('oversized SSE frame')
            line = raw.rstrip(b'\r').decode('utf-8')
            if line.startswith(':'):
                frames.append({'event': 'heartbeat', 'data': ''})
            elif not line:
                if self.data or self.fields:
                    frames.append(dict(self.fields, event=self.fields.get('event', 'message'), data='\n'.join(self.data)))
                self.fields, self.data, self.frame_size = {}, [], 0
            else:
                key, _, value = line.partition(':')
                value = value.removeprefix(' ')
                if key == 'data': self.data.append(value)
                elif key in ('event', 'id', 'retry'): self.fields[key] = value
        if len(self.buffer) > 65536:
            raise ValueError('oversized SSE line')
        return frames
