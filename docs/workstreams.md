# Parallel workstreams (post-P0)

| ID | Crate / area         | Depends on | Can start when         |
|----|----------------------|------------|------------------------|
| W1 | `aap-proto`          | P0         | P0 merged              |
| W2 | `aap-transport`      | P0         | P0 merged              |
| W3 | Docker (`docker/`)   | P0         | P0 merged              |
| W4 | CI + repo hygiene    | P0         | P0 merged              |
| W5 | `aap-core` + bin     | W1, W2     | W1+W2 merged           |
| W6 | `aap-video`          | W5         | W5 merged              |
| W7 | `scripts/`           | W3 + W5    | W3+W5 merged           |

Each workstream owns its directory exclusively. Shared files
(`server/Cargo.toml`, `.github/workflows/ci.yml`, `docker/docker-compose.yml`)
are pre-populated in P0; later edits are append-only to minimise contention.
