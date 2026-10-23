# Releasing Broom

Everything reaches `main` through a branch and a pull request. Two rulesets
enforce it, applied from this repository and visible under Settings › Rules:

- `.github/rulesets/main.json`: `main` refuses direct pushes, force pushes,
  deletion, unsigned commits, merge commits (squash or rebase only) and any
  merge without a green **Build and test (macOS)**. No bypass for anyone,
  owner included.
- `.github/rulesets/tags.json`: `v*` tags must be signed and can never be
  moved or deleted.

Re-apply either with `gh api -X POST repos/sriinnu/broom/rulesets --input <file>`
(or `PUT …/rulesets/<id>` to update).

**While the repository is private** the live `main` ruleset has the
`required_status_checks` rule removed: macOS runners bill minutes on a
private repository and the job never starts, which would block every merge.
`main.json` keeps the rule as the intended state. The day the repository
goes public, re-apply the file as is (`PUT …/rulesets/25717486`) and CI is
required again.

## Cutting a release

1. Bump `version` in `Cargo.toml` on a branch, open a PR, merge it.
2. On `main`, create a signed, annotated tag and push it:

       git switch main && git pull
       git tag -s v0.2.0 -m "Broom 0.2.0"
       git push origin v0.2.0

   Tags sign by default here (`tag.gpgSign true`) with the same SSH key as
   commits, so the Titan key needs a touch.
3. The Release workflow refuses to run unless GitHub verifies the tag's
   signature and the tag is on `main`. It then builds `Broom.app` and the
   DMG, signs and notarizes them when the Developer ID secrets are set,
   writes `SHA256SUMS`, attests build provenance (public repositories only),
   and publishes a GitHub release from the tag with the DMG and checksums.

Verify a download:

    shasum -a 256 -c SHA256SUMS
    gh attestation verify Broom-0.2.0.dmg -R sriinnu/broom   # when attested

## Supply chain

- Every action in the workflows is pinned to a commit, with the tag in a
  comment. Dependabot opens a PR when one moves.
- The repository allows only GitHub-owned actions plus `dtolnay/rust-toolchain`,
  `Swatinem/rust-cache` and `actions/attest-build-provenance`
  (Settings › Actions › General).
- Workflow tokens default to read-only; the release job asks for exactly the
  write scopes it needs.
- `cargo build --locked` in CI fails if `Cargo.lock` doesn't match `Cargo.toml`.

## Secrets

All optional; without them the DMG is ad hoc signed and runs only on the Mac
that built it.

| Secret | What |
|---|---|
| `DEVELOPER_ID_P12` | base64 of the Developer ID Application certificate (.p12) |
| `DEVELOPER_ID_P12_PASSWORD` | its password |
| `DEVELOPER_ID_SIGN_ID` | e.g. `Developer ID Application: Name (TEAMID)` |
| `NOTARY_APPLE_ID`, `NOTARY_TEAM_ID`, `NOTARY_APP_PASSWORD` | notarytool credentials (an app-specific password) |

Locally, `scripts/bundle.sh --dmg` does the same build; `scripts/deploy.sh`
builds and installs to /Applications.
