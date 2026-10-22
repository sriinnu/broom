# Releasing Broom

Everything reaches `main` through a branch and a pull request; `main` refuses
direct pushes, force pushes, unsigned commits and merges that skip CI.

1. Bump `version` in `Cargo.toml` on a branch, open a PR, merge it.
2. On `main`, create a signed tag and push it:

       git tag -s v0.2.0 -m "Broom 0.2.0"
       git push origin v0.2.0

   Tags sign by default here (`tag.gpgSign true`), with the same SSH key as
   commits.
3. The Release workflow builds `Broom.app` and `Broom.dmg`, signs and
   notarizes them when the Developer ID secrets are configured, and publishes
   a GitHub release from the tag with the DMG attached.

Secrets the workflow looks for (all optional; without them the DMG is ad hoc
signed and runs only on the Mac that built it):

| Secret | What |
|---|---|
| `DEVELOPER_ID_P12` | base64 of the Developer ID Application certificate (.p12) |
| `DEVELOPER_ID_P12_PASSWORD` | its password |
| `DEVELOPER_ID_SIGN_ID` | e.g. `Developer ID Application: Name (TEAMID)` |
| `NOTARY_APPLE_ID`, `NOTARY_TEAM_ID`, `NOTARY_APP_PASSWORD` | notarytool credentials (an app-specific password) |

Locally, `scripts/bundle.sh --dmg` does the same build; `scripts/deploy.sh`
builds and installs to /Applications.
