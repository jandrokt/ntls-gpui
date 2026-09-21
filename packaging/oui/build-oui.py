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

Names are tidied on the way out: the corporate form trailing off the end of a
registration ("Dell Inc." , "Brother Industries, LTD.") is not what anybody
calls the company, and a column of them at a glance is harder to read for it.

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
#
# "Private" is the IEEE's word for a registrant who paid not to be listed. It
# is not a company, and a scan of an office that printed it against a dozen
# rows was reporting the registry's bookkeeping as if it were an answer.
# Nothing is the honest thing to say there, and it is what other scanners say.
PLACEHOLDERS = ("ieee registration authority", "private")


def uninformative(name):
    n = (name or "").strip().lower().strip(".,")
    return not n or n.startswith(PLACEHOLDERS)


# The words a company registers itself under and nobody calls it by. They are
# noise in a column read at a glance: what tells two rows apart is "Dell" and
# "Brother", not that one of them is an Inc. and the other a LTD.
#
# Only a trailing run of them goes: a form in the middle of a name is part of
# the name ("Sony Interactive Entertainment"), and a name made of nothing but
# forms is left exactly as the registry wrote it.
LEGAL_FORMS = {
    "ab", "ag", "ao", "apS", "as", "bhd", "bv", "cjsc", "co", "coltd",
    "company", "corp", "corporate", "corporation", "doo", "dooel", "gbr",
    "gmbh", "inc", "incorporated", "incorporation", "jsc", "kg", "kgaa", "kk",
    "lda", "limited", "llc", "llp", "lp", "ltd", "ltda", "mbh", "nv", "ohg",
    "ojsc", "ooo", "oy", "oyj", "plc", "pte", "pty", "pvt", "sa", "sarl",
    "sas", "sdn", "spa", "sro", "srl", "ug", "zoo",
}


def tidy(name):
    """The name without the corporate form trailing off the end of it."""
    out = trim_end(re.sub(r"\s+", " ", (name or "").strip()))
    parts = out.split(" ")
    while len(parts) > 1:
        tail = re.sub(r"[^0-9a-z]", "", parts[-1].lower())
        if tail not in LEGAL_FORMS:
            break
        parts.pop()
    return trim_end(" ".join(parts)) or out


def trim_end(name):
    """Drops the punctuation a trailing form left behind.

    A full stop closing the last word goes with it; one inside an
    abbreviation is part of the name, so "T.L.S. Corp." keeps its own.
    """
    out = name.strip(" ,;-")
    last = out.rsplit(" ", 1)[-1]
    if out.endswith(".") and last.count(".") == 1:
        out = out[:-1]
    return out.strip(" ,;-")


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

    kept = {p: tidy(v) for p, v in merged.items() if not uninformative(v)}
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
