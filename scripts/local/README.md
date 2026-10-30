# Maintainer-only sync config

This directory is gitignored except this README and `*.example` files.

```bash
cp scripts/local/upstream.env.example scripts/local/upstream.env
# edit SHISHA=/absolute/path/to/shisha
```

Optional: `extra_denylist.txt` (same `[paths]` / `[cargo]` sections as
`scripts/public_denylist.txt`) is merged by `check_public_boundary.sh` when present.

Never commit `upstream.env` or other non-example files here.
