# Fork backend container image

The fork publishes its self-hosted backend to GitHub Container Registry:

```text
ghcr.io/bl4ckbl1zz/convex-backend
```

The `Publish Fork Backend Image` GitHub Actions workflow builds the image for
every merge into `main`, except merges that only change fork documentation, the
Dokploy profiles under `self-hosted/docker/` or the pull-request workflows;
those keep the previous image. It publishes native Linux amd64 and arm64 images
behind one multi-platform manifest, so Docker automatically selects the correct
image for each Dokploy host: production (Proxmox) is amd64, and RestoreCord's
MicroApi Convex host is arm64. A run takes about 35 minutes; see
[`.github/UPSTREAM_SYNC.md`](../../.github/UPSTREAM_SYNC.md#3-ci-and-image-publishing).

## Tags

- `latest` and `main` point to the newest successful build from `main`. A manual
  run on another branch publishes only the `sha-` tags.
- `sha-<full-git-sha>` is immutable and identifies the exact source revision.
- `sha-<full-git-sha>-amd64` and `sha-<full-git-sha>-arm64` identify the
  platform-specific images used to construct the multi-platform manifest.

Production and ModularBots Dev use `latest`. The publish workflow redeploys them
after each build from `main`: Dev first, then production once Dev answers
`/version` with the new commit. Set `CONVEX_BACKEND_IMAGE` to an immutable tag
only to hold a stack on one version or to roll back:

```dotenv
CONVEX_BACKEND_IMAGE=ghcr.io/bl4ckbl1zz/convex-backend:sha-<full-git-sha>
```

An image's `/version` endpoint answers with the commit it was built from. Images
published before the deploy job existed answer `unknown`.

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
