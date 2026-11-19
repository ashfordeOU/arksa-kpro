#!/usr/bin/env python3
# PRIVATE — Arksa-exclusive (arksa-kpro).
"""Fetch the public anomaly-detection benchmark files the `ops ad-score` opt-in tests use.

Nothing fetched here is committed to this repository. Standard library only.

  python3 -I scripts/fetch_ad_benchmarks.py <out-dir>

Writes:
  <out-dir>/opssat-ad/dataset.csv, LICENSE
      OPSSAT-AD, doi:10.5281/zenodo.15108715. Zenodo record licence: CC BY 4.0; the record's
      LICENSE file is an MIT licence. Attribute: Ruszczak, B., Kotowski, K., Evans, D., Nalepa, J.,
      "The OPS-SAT benchmark for detecting anomalies in satellite telemetry", Scientific Data 12 (2025), doi:10.1038/s41597-025-05035-3.
  <out-dir>/esa-ad/ESA-Mission1/labels.csv, anomaly_types.csv, channels.csv
      ESA Anomalies Dataset v1.0, doi:10.5281/zenodo.12528696, licence CC BY 3.0 IGO. Attribute:
      Kotowski, K., Haskamp, C., Andrzejewski, J., Ruszczak, B. et al., "European Space Agency Benchmark for
      Anomaly Detection in Satellite Telemetry", arXiv:2406.17826 (2024).
      Only these small annotation files are read, by HTTP range requests into the 3.7 GB
      archive; the telemetry itself is not downloaded.

Then run the opt-in tests:
  ARKSA_OPSSAT_AD_CSV=<out-dir>/opssat-ad/dataset.csv \\
  ARKSA_ESA_AD_DIR=<out-dir>/esa-ad/ESA-Mission1 \\
  cargo test --test ops_assurance -- --ignored
"""

import hashlib
import io
import os
import sys
import urllib.request
import zipfile

ZENODO = "https://zenodo.org/api/records"
OPSSAT = (15108715, {"dataset.csv": "b524177da6f516d5c9f63c7acbc385341f0ad42046ef20c1bed2d25e51b98f02", "LICENSE": None})
ESA_AD = (12528696, "ESA-Mission1.zip", {
    "ESA-Mission1/labels.csv": "1564d630f1ec1387d699ec81531ce57118051a220c8b7cba050a446a652b9082",
    "ESA-Mission1/anomaly_types.csv": "e0cadee4ee9697c5ca8d4f9e81067454d7d7a00d063ee0de33cbc77660eec8bc",
    "ESA-Mission1/channels.csv": "92fe4582f1907e42129a764e52afddaef02de8e555e4c47302acd4225222abbf",
})
UA = {"User-Agent": "ad-benchmark-fetch/1"}


def get(url, headers=None):
    req = urllib.request.Request(url, headers={**UA, **(headers or {})})
    with urllib.request.urlopen(req, timeout=120) as r:
        return r.read()


class RangeFile(io.RawIOBase):
    """A read-only, seekable view of a remote file through HTTP range requests."""

    def __init__(self, url):
        self.url, self.pos = url, 0
        req = urllib.request.Request(url, method="HEAD", headers=UA)
        with urllib.request.urlopen(req, timeout=120) as r:
            self.size = int(r.headers["Content-Length"])

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self.pos

    def seek(self, off, whence=0):
        self.pos = off if whence == 0 else self.pos + off if whence == 1 else self.size + off
        return self.pos

    def readinto(self, b):
        if self.pos >= self.size:
            return 0
        end = min(self.size, self.pos + len(b)) - 1
        data = get(self.url, {"Range": f"bytes={self.pos}-{end}"})
        b[: len(data)] = data
        self.pos += len(data)
        return len(data)


def check(name, data, sha):
    h = hashlib.sha256(data).hexdigest()
    if sha and h != sha:
        sys.exit(f"{name}: SHA-256 {h} differs from the expected {sha}; the upstream file changed")
    print(f"{name}: {len(data)} bytes, sha256 {h}")


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    out = sys.argv[1]
    rec, files = OPSSAT
    d = os.path.join(out, "opssat-ad")
    os.makedirs(d, exist_ok=True)
    for name, sha in files.items():
        data = get(f"{ZENODO}/{rec}/files/{name}/content")
        check(name, data, sha)
        open(os.path.join(d, name), "wb").write(data)
    rec, archive, files = ESA_AD
    z = zipfile.ZipFile(io.BufferedReader(RangeFile(f"{ZENODO}/{rec}/files/{archive}/content"), 1 << 16))
    for name, sha in files.items():
        data = z.read(name)
        check(name, data, sha)
        p = os.path.join(out, "esa-ad", name)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        open(p, "wb").write(data)


if __name__ == "__main__":
    main()
