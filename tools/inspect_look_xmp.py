#!/usr/bin/env python3
"""Inspect Cobalt / ACR Look XMP files: metadata + embedded table blob stats.

Reference oracle for workstreams W5/W7. Prints everything EXCEPT decoded table
data. Does not, and must not, emit vendor table contents.

Usage: python3 inspect_look_xmp.py <file.xmp> [more.xmp ...]
"""
import re, sys, os

ATTRS = ["PresetType", "Cluster", "UUID", "SupportsAmount", "SupportsColor",
         "SupportsMonochrome", "SupportsHighDynamicRange", "SupportsSceneReferred",
         "RequiresRGBTables", "CameraModelRestriction", "Copyright", "Version",
         "ProcessVersion", "ConvertToGrayscale", "CameraProfile", "RGBTable", "HasSettings"]

def elem(d, tag):
    """Extract a crs:<tag> alt-text value.

    NOTE: the block MUST be scoped to </crs:tag> first. These files write empty
    values as a SELF-CLOSING <rdf:li xml:lang="x-default"/>, so an unscoped lazy
    match will skip past it and silently capture the NEXT element's text
    (e.g. ShortName picking up Group). See COBALT_DCP_SPEC.md section 1.1.
    """
    block = re.search(r"<crs:%s>(.*?)</crs:%s>" % (tag, tag), d, re.S)
    if not block:
        return ""
    m = re.search(r"<rdf:li[^>]*>([^<]*)</rdf:li>", block.group(1), re.S)
    return m.group(1).strip() if m else ""

for path in sys.argv[1:]:
    d = open(path, encoding="utf-8").read()
    print("=" * 72)
    print(os.path.basename(path))
    print("=" * 72)
    for a in ATTRS:
        m = re.search(r'crs:%s="([^"]*)"' % a, d)
        print(f"  {a:26} {m.group(1) if m else '<absent>'!r}")
    for t in ("Name", "ShortName", "SortName", "Group", "Description"):
        print(f"  <{t}>{'':>{max(0,20-len(t))}}      {elem(d,t)!r}")

    m = re.search(r'crs:Table_([A-F0-9]+)="([^"]*)"', d, re.S)
    if not m:
        print("  table: <none>")
        continue
    blob = "".join(m.group(2).split())
    cs = sorted(set(blob))
    missing = [chr(c) for c in range(33, 126) if chr(c) not in set(blob)]
    print(f"  table uuid                 {m.group(1)}")
    print(f"  table length               {len(blob)} chars")
    print(f"  alphabet size              {len(cs)}  (ord {min(map(ord,cs))}..{max(map(ord,cs))})")
    print(f"  excluded from 33..125      {missing}")
    print(f"  magic prefix (first 5)     {blob[:5]!r}")
    print(f"  next 8 chars               {blob[5:13]!r}")
