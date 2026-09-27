# Contributing

## Setup

```bash
nix develop        # pins cargo, k3d, kubectl, just, sqlx-cli, node
just doctor        # checks docker + cluster
```

Docker must run outside the devshell — see [DEV.md](docs/DEV.md) for the
one-time setup and the dev loops.

## The bar for every change

```bash
just verify-all    # build + workspace tests
just lint          # clippy -D warnings
just fmt           # rustfmt
```

`just verify-all` must pass before you open a PR. New behavior needs tests;
fixes need a test that fails without the fix.

## Code style

- Rust, stable toolchain, edition 2024. Match the style around your change.
- Functional style: plain functions and explicit data. No object-oriented
  patterns, no trait-object layering without a reason.
- Validate external input at the boundary. Use concrete types everywhere
  else. No `unwrap()` outside tests.
- Comments are minimal: one short header per file, inline comments only for
  non-obvious decisions. Never narrate what the code already says.
- Coding agents: follow `AGENTS.md` — it is the operational contract for
  working in this repo.

## Commits

- Format: `type: imperative summary under 50 chars`, then a body that
  explains the why more than the what.
- Types: `feat`, `fix`, `refactor`, `docs`, `build`, `ci`, `test`, `chore`.
- One logical change per commit. Lock files and vendored code get their own
  commits.
- Commits are authored by the repository owner. No AI co-author trailers.

## Documentation

Docs are part of the change:

- `docs/CHECKPOINTS.md` — mark a checkpoint `[x]` only after running its
  gate command, with the date.
- Checkpoints cite the spec section they implement (`[§...]` on each
  milestone). Keep those references current when the spec moves.
- Dev workflows belong in `docs/DEV.md`, not the README. The README stays a
  project overview.

## Reporting issues

Include the failing gate command, the full error output, and the commit you
are on. Kubernetes and docker state questions (`kubectl describe`, `docker
logs`) beat screenshots.
