# Fork backend container image

The fork publishes its self-hosted backend to GitHub Container Registry:

```text
ghcr.io/bl4ckbl1zz/convex-backend
```

The `Publish Fork Backend Image` GitHub Actions workflow builds the image from
every commit merged into `main`. It currently publishes a Linux amd64 image for
the x86-64 Dokploy hosts.

## Tags

- `latest` and `main` point to the newest successful build from `main`.
- `sha-<full-git-sha>` is immutable and identifies the exact source revision.

Use `latest` for automatic updates or set `CONVEX_BACKEND_IMAGE` to an immutable
tag or digest for controlled production rollouts:

```dotenv
CONVEX_BACKEND_IMAGE=ghcr.io/bl4ckbl1zz/convex-backend:sha-<full-git-sha>
```

Both Dokploy Compose profiles use the published image and set
`pull_policy: always`. They no longer compile the Rust backend on the Dokploy
host.

## Minimal Compose service

```yaml
services:
  backend:
    image: ${CONVEX_BACKEND_IMAGE:-ghcr.io/bl4ckbl1zz/convex-backend:latest}
    pull_policy: always
    restart: unless-stopped
```

The package must be public for an anonymous Dokploy pull. If it remains private,
configure Dokploy's registry credentials with a GitHub token that has
`read:packages`.
