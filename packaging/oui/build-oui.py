#!/usr/bin/env python3
"""Builds assets/oui.dat.gz, the MAC prefix to vendor table compiled into ntls.

Two sources, and the reason for the second one is the whole point of this
script.

The IEEE registries are authoritative for the names, and ntls carries all three
of them: MA-L (a 24-bit prefix), MA-M (28-bit) and MA-S (36-bit). But the IEEE
publishes a 24-bit block under the name "IEEE Registration Authority" when it
has subdivided that block into MA-M or MA-S assignments, and it publishes some
blocks under the name "Private" when the registrant asked not to be listed.
Neither is a vendor. A device in such a block resolves to a placeholder or to
nothing, and that is what a scan shows.

Wireshark maintains its own table over the same registries with several
thousand extra fine-grained entries that the IEEE does not publish, which is
why a Wireshark-derived scanner names devices ntls could not. Those entries are
merged in wherever the IEEE has nothing informative to say. The IEEE always
wins where it does.

The output is one `prefix<TAB>vendor` line per assignment, gzipped, prefix in
upper-case hex of 6, 7 or 9 digits. Nothing here runs at build time or at run
time: the file is committed, and this script is how it was made.

    python3 packaging/oui/build-oui.py

Sources, both public and both fetched read-only:
  https://standards-oui.ieee.org/oui/oui.csv        (MA-L, 24-bit)
  https://standards-oui.ieee.org/oui28/mam.csv      (MA-M, 28-bit)
  https://standards-oui.ieee.org/oui36/oui36.csv    (MA-S, 36-bit)
  https://www.wireshark.org/download/automated/data/manuf
"""

import csv
import gzip
import io
import pathlib
import re
import sys
import urllib.request

IEEE = [
    ("https://standards-oui.ieee.org/oui/oui.csv", 6),
    ("https://standards-oui.ieee.org/oui28/mam.csv", 7),
    ("https://standards-oui.ieee.org/oui36/oui36.csv", 9),
]
WIRESHARK = "https://www.wireshark.org/download/automated/data/manuf"

# The names that are not vendors. A block carrying one of these is a block
# whose real owner is either recorded further down at a longer prefix or not
# recorded at all.
PLACEHOLDERS = ("ieee registration authority",)


def uninformative(name):
    n = (name or "").strip().lower()
    return not n or n.startswith(PLACEHOLDERS)


def fetch(url):
    # The IEEE server answers a request with no user agent with a 418, so say
    # who is asking.
    request = urllib.request.Request(url, headers={"User-Agent": "ntls-oui-build"})
    with urllib.request.urlopen(request, timeout=120) as r:
        return r.read()


def read_ieee():
    table = {}
    for url, digits in IEEE:
        raw = fetch(url).decode("utf-8", "replace")
        rows = csv.DictReader(io.StringIO(raw))
        n = 0
        for row in rows:
            prefix = (row.get("Assignment") or "").strip().upper()
            name = (row.get("Organization Name") or "").strip()
            if len(prefix) != digits or not name:
                continue
            table[prefix] = name
            n += 1
        print(f"  IEEE {url.rsplit('/', 1)[-1]}: {n} assignments of {digits} hex digits")
    return table


def read_wireshark():
    table = {}
    text = fetch(WIRESHARK).decode("utf-8", "replace")
    for line in text.splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        cols = [c.strip() for c in line.split("\t") if c.strip()]
        if len(cols) < 2:
            continue
        # The last column is the full vendor name where there is one; the
        # second is Wireshark's own abbreviation, used when there is not.
        name = cols[-1] if len(cols) > 2 else cols[1]
        found = re.match(r"^([0-9A-Fa-f:]+)(?:/(\d+))?$", cols[0])
        if not found:
            continue
        hexes = found.group(1).replace(":", "").upper()
        bits = int(found.group(2)) if found.group(2) else len(hexes) * 4
        digits = {24: 6, 28: 7, 36: 9}.get(bits)
        if digits:
            table[hexes[:digits]] = name
    print(f"  Wireshark manuf: {len(table)} assignments")
    return table


def main():
    root = pathlib.Path(__file__).resolve().parents[2]
    out = root / "assets" / "oui.dat.gz"

    print("reading the registries")
    ieee = read_ieee()
    wireshark = read_wireshark()

    merged = dict(ieee)
    added = 0
    for prefix, name in wireshark.items():
        if uninformative(name):
            continue
        # The IEEE wins wherever it says anything real. Wireshark fills the
        # blocks it has subdivided and the ones it never listed.
        if prefix not in merged or uninformative(merged[prefix]):
            merged[prefix] = name
            added += 1

    kept = {p: v for p, v in merged.items() if not uninformative(v)}
    print(f"\n  IEEE alone:        {len(ieee)}")
    print(f"  Wireshark added:   {added}")
    print(f"  placeholders cut:  {len(merged) - len(kept)}")
    print(f"  written:           {len(kept)}")

    if len(kept) < 50_000:
        sys.exit(f"only {len(kept)} prefixes; refusing to write a table that small")

    body = "".join(f"{p}\t{v}\n" for p, v in sorted(kept.items()))
    # mtime zero so rebuilding identical data gives an identical file.
    with gzip.GzipFile(filename="", mode="wb", fileobj=open(out, "wb"), mtime=0) as gz:
        gz.write(body.encode("utf-8"))
    print(f"\nwrote {out} ({out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
