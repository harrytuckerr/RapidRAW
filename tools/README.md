# Reference tooling

Independent Python implementations used to establish the ground truth in
`COBALT_DCP_SPEC.md` §1. They exist so Rust work (W1) can be cross-checked
against a second, independent reading of the same bytes.

| Script | Purpose |
|---|---|
| `inspect_dcp.py` | Dump a `.dcp` IFD: every tag, type, count, value. Verified against `Fujifilm X-Pro2 Cobalt Flat v3.0.dcp`. |
| `inspect_look_xmp.py` | Dump Look XMP metadata + table blob statistics (never contents). |

```bash
python3 tools/inspect_dcp.py "path/to/profile.dcp"
python3 tools/inspect_look_xmp.py path/to/*.xmp
```

These are diagnostic aids, not part of the shipped app. Point them at your own
licensed files; never commit their output containing vendor table data.
