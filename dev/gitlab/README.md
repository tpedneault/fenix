# A local GitLab, for testing the real thing

Fenix's GitLab integration (`SPC g M`) was built against GitLab's API
documentation and verified against a hand-written stub. A stub can only
confirm the assumptions that went into it: it accepts whatever Fenix
sends, so it cannot tell you that a payload GitLab's own validator
rejects is wrong. This directory runs the real server instead.

Everything here is development scaffolding. It never ships, it is
reachable only on `localhost`, and every credential in it is
deliberately fixed and public — a throwaway container holding one seeded
project full of nonsense is not a secret worth protecting, and a fixed
token is what lets `seed.sh` be re-run without editing anything.

## Setup

```bash
docker compose -f dev/gitlab/docker-compose.yml up -d
./dev/gitlab/wait-ready.sh    # first boot takes several minutes
./dev/gitlab/seed.sh
```

`wait-ready.sh` exists because GitLab reports itself healthy well before
`/api/v4` answers — Puma comes up last. Seeding against a container
that is "healthy" but not serving fails with a connection reset, which
looks like a bug in the seed script rather than what it is.

Then add to `config.ini`:

```ini
[gitlab]
base_url = http://localhost:8929
token = fenix-dev-token-0123456789
```

and clone the seeded project somewhere:

```bash
git clone http://root:fenix-dev-token-0123456789@localhost:8929/fenix-dev/widget.git
```

Opening that clone and pressing `SPC g M` reads the merge requests off a
real instance — the project comes from the clone's own `origin`, so
nothing else needs configuring.

## What gets seeded

- `fenix-dev/widget`, with a `main` branch and two feature branches.
- **!1 "Make the timeout configurable"** — one file, with an added
  line, a removed line and context around both, so every case the review
  pane can anchor a comment to is present.
- **!2 "Draft: Expand the README"** — so the draft badge and the
  `Draft:`-already-in-the-title case are both exercised.
- On !1: a review thread anchored to a real diff line, and a plain
  comment on the request as a whole — the two cases the view renders in
  different places (inline on the diff, and in the detail pane).

The seeded thread is created through the same `position` payload shape
Fenix sends. If `seed.sh` succeeds, GitLab's own validator has accepted
that shape.

## Tearing it down

```bash
docker compose -f dev/gitlab/docker-compose.yml down -v
```

`-v` drops the volumes too; without it the next `up` reuses the same
instance, which is usually what you want between sessions.

## Notes

- Boot is slow (several minutes) and memory-hungry (~4 GB). Prometheus,
  Grafana, Alertmanager and SMTP are disabled in the compose file, which
  is most of the difference between a four-minute boot and a ten-minute
  one.
- The instance is Free tier, so approval *rules* don't exist —
  `approvals_required` comes back absent. That is itself worth having:
  it is the case the detail pane has to handle without inventing a rule,
  and it is the case a self-hosted Free instance would hit.
