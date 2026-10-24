# Releasing the Neuron apps

How the desktop app, the release manifests and the mobile app are built,
signed and published, which accounts, certificates and keys this needs, and how
to roll back. The CLI and full-screen UI binaries, npm and PyPI are in
[PUBLISHING.md](../PUBLISHING.md); this page adds only the CLI manifest to that
flow.

| What | Workflow | Trigger | Output |
|---|---|---|---|
| Apps CI and brand lint | [`apps-ci.yml`](../.github/workflows/apps-ci.yml) | every pull request; pushes to `ux-audit-fixes` and `main` | the required check **Apps CI result** |
| CLI and TUI binaries, npm, PyPI | [`release.yml`](../.github/workflows/release.yml) | tag `vX.Y.Z` | GitHub release (see PUBLISHING.md) |
| CLI version manifest | [`cli-manifest.yml`](../.github/workflows/cli-manifest.yml) | Release finishes; a CLI release is published by hand; by hand | `releases/cli/latest.json` |
| Desktop installers | [`desktop-release.yml`](../.github/workflows/desktop-release.yml) | tag `desktop-vX.Y.Z`; by hand (dry run) | GitHub release `desktop-vX.Y.Z`, `releases/desktop/latest.json` |
| Mobile builds (P1) | [`mobile-eas.yml`](../.github/workflows/mobile-eas.yml) | by hand | EAS builds, TestFlight, Play internal testing |

Until the secrets below exist, every workflow still runs: it skips the signing
or upload it cannot do and says so in a notice on the run. The checks and
tests still run either way. An unsigned desktop build is never published.

## What Rahul has to provide

None of these exist yet. Add each one as the named GitHub secret or variable
(Settings, Secrets and variables, Actions). Secrets may also live on the
environment that uses them. No workflow prints a secret; the metadata job only
reports whether each one is set.

### Alpha (P0)

**1. Windows code signing: one of these two.**

New Authenticode certificates from public CAs keep their private key on a
hardware token or in a cloud HSM, so most can no longer be exported as a PFX
file. Then use (B), or a CA's cloud signing service wired in the same way as
(B) through Tauri's `signCommand`.

| | Secrets | Variables |
|---|---|---|
| (A) An exportable code-signing certificate (OV or EV) | `WINDOWS_CERTIFICATE`: the `.pfx`, base64-encoded (`base64 -w0 cert.pfx`); `WINDOWS_CERTIFICATE_PASSWORD` | `WINDOWS_TIMESTAMP_URL` (optional; default `http://timestamp.digicert.com`) |
| (B) Azure Artifact Signing (formerly Trusted Signing): an Azure subscription, an Artifact Signing account with a verified identity and a certificate profile, and an app registration with the *Artifact Signing Certificate Profile Signer* role | `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET`, `AZURE_TENANT_ID` | `AZURE_SIGNING_ENDPOINT` (for example `https://eus.codesigning.azure.net`), `AZURE_SIGNING_ACCOUNT`, `AZURE_SIGNING_PROFILE` |

Before paying for (B), check that Artifact Signing accepts Zero-X's country and
entity type.

Signed: the installer, `neuron-desktop.exe`, the bundled `neuron.exe` and the
uninstaller. The install test checks every one of them with
`Get-AuthenticodeSignature`.

**2. Apple Developer Program membership.** Use an organization account if
Zero-X should be the name users see.

- A **Developer ID Application** certificate, exported from Keychain Access
  with its private key as a `.p12`:
  - secret `APPLE_CERTIFICATE`: the `.p12`, base64-encoded (`base64 -i cert.p12`);
  - secret `APPLE_CERTIFICATE_PASSWORD`;
  - optional variable `APPLE_SIGNING_IDENTITY`. It defaults to the first
    "Developer ID Application" identity in the certificate.
- Notarization credentials, one of:
  - **an App Store Connect API key** (recommended; Users and Access,
    Integrations, Team Keys, Developer role): secrets `APPLE_API_ISSUER` (the
    issuer ID), `APPLE_API_KEY_ID` and `APPLE_API_PRIVATE_KEY` (the `.p8` file's
    contents);
  - **an Apple ID** with an app-specific password: secrets `APPLE_ID`,
    `APPLE_APP_SPECIFIC_PASSWORD` and `APPLE_TEAM_ID`.

Signed and notarized: `Neuron.app` with the hardened runtime (the bundled
`neuron` included), stapled, and the dmg, signed, notarized and stapled. The
build checks `codesign`, `stapler` and `spctl`.

**3. The release-manifest bucket.** This is a Cloudflare R2 bucket that
`https://zero-x.live/releases/` serves (WP5 adds that route to the gateway).

- secrets `RELEASES_R2_ACCESS_KEY_ID` and `RELEASES_R2_SECRET_ACCESS_KEY`: an R2
  API token with *Object Read & Write* on this one bucket only;
- variables `RELEASES_R2_ACCOUNT_ID` (the Cloudflare account ID),
  `RELEASES_R2_BUCKET` and, if the bucket is served from somewhere else,
  `RELEASES_PUBLIC_URL` (default `https://zero-x.live/releases`).

The workflows write the objects `releases/cli/latest.json`,
`releases/desktop/latest.json`, `releases/desktop/prerelease.json` and (P1)
`releases/desktop/updater.json`, each with `Cache-Control: public, max-age=300`.

**4. npm.** `release.yml` publishes with npm trusted publishing (OIDC), so no
workflow uses an npm token. What is needed is owner access to
`@zero-x.live/neuron` on npmjs.com, to add the trusted publisher (repository
`RAHUL-DevelopeRR/neuron-v2`, workflow `release.yml`, environment `npm`):
PUBLISHING.md, One-time setup.

**5. GitHub settings.**

- Environments, each with Rahul as required reviewer:
  - `desktop-release` (publishing a desktop release);
  - `release-manifests` (uploading manifests). Limit its deployment branches
    and tags to `ux-audit-fixes`, `v*` and `desktop-v*`;
  - `mobile-release` (EAS builds, P1);
  - `npm` and `pypi` (PUBLISHING.md).
- Branch protection on `ux-audit-fixes` (and `feat/neuron-platform` if
  wanted): require **Apps CI result**.
- Optional variables:
  - `DOWNLOAD_PAGE_URL` (default `https://zero-x.live/download`);
  - `DESKTOP_PLATFORMS=all`, so tag builds include the P1 platforms;
  - `BRAND_LINT_MODE` (see [Brand lint](#brand-lint)).

### Beta (P1)

**6. Tauri updater key pair**, for in-place desktop updates (D23):

```sh
cd ui/neuron-desktop
bun run tauri signer generate -w ~/.tauri/neuron-updater.key
```

- Use a password when generating it.
- Secrets: `TAURI_SIGNING_PRIVATE_KEY` (the private key file's contents) and
  `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
- WP3 puts the public key in `tauri.conf.json` (`plugins.updater.pubkey`), with
  the endpoint `https://zero-x.live/releases/desktop/updater.json`.
- Keep an offline copy of the private key. Without it, installed apps can never
  receive another in-place update.

**7. Mobile.**

- **An Expo account:** secret `EXPO_TOKEN` (a robot token). Then run
  `bunx eas-cli init` once in `ui/neuron-mobile`. It writes `extra.eas.projectId`
  into `app.json`, which WP4 owns, so commit that change with them.
- **Apple:** the same Developer Program membership, and an App Store Connect app
  for `live.zero-x.neuron.mobile`. Let EAS manage the signing credentials, and
  upload an App Store Connect API key to EAS for submissions. Add the app's
  `ascAppId` to `eas.json` (`submit.production.ios`) once the app exists.
- **Google Play:** a Play Console developer account, an app for
  `live.zero_x.neuron.mobile`, and a service-account JSON key uploaded to EAS for
  submissions. The first upload of a new Play app must be made by hand in the
  Play Console.
- **Push (with WP5 A12):** an APNs key and a Firebase project for FCM.
- **Store listings:** a privacy policy URL and the data-safety and
  privacy-label answers. Remote control and push tokens count as personal data.

## Desktop release

### Prepare

1. Set the same version in `ui/neuron-desktop/src-tauri/tauri.conf.json`,
   `ui/neuron-desktop/src-tauri/Cargo.toml` (and its `Cargo.lock` entry) and
   `ui/neuron-desktop/package.json`. The workflow refuses a mismatch, or a tag
   that does not match. Use a plain `X.Y.Z` for anything users should be told
   about:
   - a version with a `-suffix` is a GitHub prerelease, written to
     `releases/desktop/prerelease.json`;
   - the MSI (P1) needs a numeric version.
2. The engine inside the app is built from the same commit. Its version is the
   one in `rust/Cargo.toml`, and the manifest records it as `engine_version`.
3. Do a dry run from the branch (next section) and read its summary: which
   platforms were signed and notarized, and the manifest it would publish.

### Dry run

```sh
gh workflow run desktop-release.yml --repo RAHUL-DevelopeRR/neuron-v2 --ref <branch> -f platforms=alpha -f dry_run=true
```

A dry run builds the engine and the app for every platform, signs with
whatever secrets exist, and installs each build on a fresh runner:

- Windows: a silent NSIS install, the Authenticode checks, `neuron.exe
  --version`, then a silent uninstall.
- macOS: mount the dmg, copy `Neuron.app`, check that both binaries are
  universal, the `codesign`/`stapler`/`spctl` checks.
- Linux: install the deb, extract the AppImage.

On each runner, the bundled engine must answer `ready` to
`neuron engine --stdio`, and the app must still be running after 15 seconds.
The installers, `SHA256SUMS` and the manifests are kept as workflow artifacts.
Nothing is released or uploaded.

Pull requests that change `desktop-release.yml` or `tauri.release.conf.json`
run the same dry run.

### Release

```sh
git tag -a desktop-v0.2.0 -m "Neuron Desktop 0.2.0"
git push origin desktop-v0.2.0
```

The workflow then works through these steps:

1. **Build and test**, as in a dry run.
2. **Draft release** `desktop-v0.2.0`:
   - It holds the installers, their `.sha256` files and `SHA256SUMS`,
     `desktop-manifest.json` and (P1) `updater.json`.
   - It is never marked **Latest**, because `install.sh` and `install.ps1`
     install the CLI from the release with that mark. If you ever publish a
     desktop draft by hand, untick *Set as the latest release*.
3. **Publish.** This needs every installer signed and the macOS build
   notarized, plus approval in the `desktop-release` environment.
   - Before approving, download the draft's installers onto a clean Windows 11
     machine and a clean Mac. Install, open a folder and run one prompt.
   - Then the job publishes the release, still not marked latest, and checks
     that every download URL answers.
4. **Manifest:** `releases/desktop/latest.json` goes to the bucket, after
   approval in `release-manifests`. The app's update notice (D23) reads it.

Without signing, the run stops after step 2: the draft stays a draft and a
warning on the run says why. A published release's files are never replaced,
so a broken release is fixed with a new version.

Then link the installers from `zero-x.live/download` (WP5, A6).

## CLI manifest

[`cli-manifest.yml`](../.github/workflows/cli-manifest.yml) writes
`releases/cli/latest.json` (T23's update notice reads it). It describes the
release that GitHub marks **Latest**, which is exactly what `install.sh` and
`install.ps1` install, so `release.yml` needs no change.

- **It runs when Release finishes.** `release.yml` publishes with the repository
  token, whose events never start other workflows, so the `release: published`
  trigger only covers releases published by hand. `release.yml` also marks a
  release latest only after its install checks.
- **It runs only from the default branch.** `workflow_run` and `release`
  triggers start from the copy of the workflow on the default branch
  (`ux-audit-fixes`), so it begins working once this file is merged there.
- **It only names published, stable `vX.Y.Z` releases.** Prereleases never
  become latest.
- **It records npm separately.** `install.npm.version` records what npm's
  `latest` tag holds, because npm publishing is a separate switch in
  `release.yml`.
- **Dry run.** The default when started by hand:

  ```sh
  gh workflow run cli-manifest.yml --repo RAHUL-DevelopeRR/neuron-v2 --ref <branch>
  ```

  Before any CLI release exists, it describes a fixture release with the same
  asset names, so the same code runs.
- **To write it now:** pass `-f dry_run=false`, and optionally `-f tag=vX.Y.Z`.

## Manifests

WP5 serves these files, and WP1 (T23) and WP3 (D23) read them. Anything not
listed may be added later; readers ignore unknown fields. `schema` changes only
when an existing field changes meaning.

`releases/cli/latest.json`:

```json
{
  "schema": 1,
  "product": "neuron",
  "version": "6.3.0",
  "tag": "v6.3.0",
  "published_at": "2026-11-02T10:00:00Z",
  "notes_url": "https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/tag/v6.3.0",
  "download_url": "https://zero-x.live/download",
  "install": {
    "npm": { "command": "npm install -g @zero-x.live/neuron@latest", "version": "6.3.0" },
    "sh": { "command": "curl -fsSL https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/latest/download/install.sh | sh" },
    "powershell": { "command": "irm https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/latest/download/install.ps1 | iex" }
  },
  "assets": {
    "neuron-linux-x64": { "url": "https://github.com/…/releases/download/v6.3.0/neuron-linux-x64", "sha256": "…" }
  }
}
```

The update notice compares its own version with `version` and shows the
`install` command for how it was installed. When installed through npm, it
compares with `install.npm.version`, because npm can lag.

`releases/desktop/latest.json` (`prerelease.json` for `-suffix` versions):

```json
{
  "schema": 1,
  "product": "neuron-desktop",
  "channel": "stable",
  "version": "0.2.0",
  "tag": "desktop-v0.2.0",
  "engine_version": "6.3.0",
  "published_at": "2026-11-02T10:00:00Z",
  "notes_url": "https://github.com/RAHUL-DevelopeRR/neuron-v2/releases/tag/desktop-v0.2.0",
  "download_url": "https://zero-x.live/download",
  "platforms": {
    "windows-x64": {
      "signed": true,
      "files": { "nsis": { "name": "Neuron_0.2.0_windows-x64-setup.exe", "url": "…", "sha256": "…" } }
    },
    "macos-universal": {
      "signed": true,
      "notarized": true,
      "files": { "dmg": { "name": "Neuron_0.2.0_macos-universal.dmg", "url": "…", "sha256": "…" } }
    }
  }
}
```

The alpha's notice (D23) needs only `version` and `download_url`: "Neuron 0.2.1
is available", linking to the downloads page. With `platforms: all`, the file
kinds `msi`, `appimage`, `deb` and `updater` appear too.

`releases/desktop/updater.json` (P1) is Tauri's static updater format
(`version`, `notes`, `pub_date`, `platforms.<os-arch>.{signature,url}`). It is
written only for stable versions when every build has an updater signature.

## Mobile (P1)

```sh
gh workflow run mobile-eas.yml --repo RAHUL-DevelopeRR/neuron-v2 --ref <branch>
gh workflow run mobile-eas.yml --repo RAHUL-DevelopeRR/neuron-v2 --ref <branch> -f dry_run=false -f profile=preview -f platform=android
gh workflow run mobile-eas.yml --repo RAHUL-DevelopeRR/neuron-v2 --ref <branch> -f dry_run=false -f profile=production -f submit=true
```

- **Dry run (the default).** Typecheck, tests, the resolved Expo config and the
  profiles in [`eas.json`](../ui/neuron-mobile/eas.json). No account needed.
- **Build profiles:**
  - `simulator`: an iOS simulator build and an Android APK. Needs only the Expo
    account.
  - `preview`: internal distribution. iOS devices must be registered with EAS.
  - `production`: store builds, with build numbers kept by EAS.
- **`submit`** sends the production builds to TestFlight and to Play internal
  testing, as a draft.
- **Moving out of testing:** store review and public listing are done in App
  Store Connect and the Play Console. They need the relay and push (WP5 A11,
  A12) and `neuron engine --share` (WP1 R1) first.

## Brand lint

[`scripts/brand-lint.mjs`](../scripts/brand-lint.mjs) keeps the reference
product's and its maker's names out of Neuron (plan guardrail 2, T6). Its
configuration is [`scripts/brand-lint.allowlist.json`](../scripts/brand-lint.allowlist.json).

**What it reads.** Only string literals and UI text, never comments or tests:

- TypeScript and TSX through the TypeScript compiler: string and template
  literals and JSX text;
- Rust through a small lexer that skips comments, doc attributes and
  `#[cfg(test)]` and `#[test]` items;
- JSON string values, and HTML outside comments.

**Scopes.**

- `ui`: every `ui/neuron-*` package, the Tauri shell and its config.
- `commands`: slash-command summaries and help, in `rust/crates/commands`.
- `cli-help`: `neuron --help` and argument errors, in `args.rs` and `main.rs`.
- `prompt`: the system prompt, in `runtime/src/prompt.rs`.
- `identity`: the app name and beta headers, in `telemetry` and `api`. This
  scope looks only for the reference product's name.
- `engine`: the rest of the engine. Report only: it warns but never fails.

**What is allowed.**

- Model IDs exactly as catalogs return them (`claude-sonnet-4-6`).
- Compatibility files and directories (`CLAUDE.md`, `.claude/`, `AGENTS.md`).
- The provider's own environment variables (`ANTHROPIC_API_KEY`), the provider
  ID `anthropic`, and its protocol headers and API host.
- Internal crate names (`rusty-claude-cli`, `mock-anthropic-service`), which are
  renamed before any public source release (P2).

A display name such as "Claude Opus 4.6" still fails.

**Run it.**

```sh
(cd ui/neuron-client && bun install --frozen-lockfile)   # the TypeScript compiler
node scripts/brand-lint.mjs            # new leaks fail; the baseline warns
node scripts/brand-lint.mjs --strict   # the baseline fails too
node scripts/brand-lint.mjs --report   # never fails
node --test scripts/brand-lint.test.mjs
```

The test plants leaks into copies of the real files and checks that exactly
those are caught. It covers a command summary, the system prompt, JSX text, a
JSX attribute, a template literal, a raw Rust string and a request identity. It
also checks that comments, tests, model IDs and compatibility names pass.

**The baseline.** It lists the known leaks, each with an owner. Today these are
WP1's T6 fixes:

- the "Claude Opus 4.6" model family and the "# Claude instructions" heading
  in the system prompt;
- the `claude-code-20250219` beta header and the `claude-code` telemetry app
  name;
- three command summaries;
- the OMC note in `args.rs`;
- two more that brand-lint found in `args.rs` and `main.rs`.

Baseline entries warn without failing. When a fix lands, the run reports that
entry as no longer matching; delete it. Once the baseline is empty, set the
repository variable `BRAND_LINT_MODE=strict`, or simply keep the default: with
an empty baseline both behave the same.

`BRAND_LINT_MODE=report` turns every failure into a warning, for an emergency.

To allow something new, add an `allow` pattern, an `exact` literal, or a
one-file `accepted` entry, always with a reason. Never use the baseline for
that.

**The TUI session** adds the same two commands (install in `ui/neuron-client`,
then `node scripts/brand-lint.mjs`) to `rust-ci.yml` or `packaging-ci.yml`.
Apps CI already runs it on every pull request that touches `ui/neuron-*` or
`rust/crates/`.

## Roll back

The manifests decide what the update notices offer, so rolling back means
pointing them at the previous good version.

- **Desktop.**
  1. Withdraw the bad release: edit its notes to say so, and mark it a
     prerelease.
  2. Point the manifest back. Re-run the **Upload the version manifest** job of
     the last good version's Desktop release run, if that run's artifacts have
     not expired (90 days). Otherwise, upload that release's
     `desktop-manifest.json` by hand:

     ```sh
     gh release download desktop-v0.2.0 --repo RAHUL-DevelopeRR/neuron-v2 --pattern desktop-manifest.json
     aws s3 cp desktop-manifest.json s3://$RELEASES_R2_BUCKET/releases/desktop/latest.json \
       --endpoint-url https://$RELEASES_R2_ACCOUNT_ID.r2.cloudflarestorage.com \
       --content-type application/json --cache-control 'public, max-age=300'
     ```

  3. P1, in-place updates: do the same with `updater.json`. Tauri never
     downgrades, so users who already updated need a new, higher patch version.
- **CLI.**
  1. Follow PUBLISHING.md (mark the bad release a prerelease, then
     `gh release edit vOLD --latest`).
  2. Run `cli-manifest.yml` with `dry_run=false`. It describes whatever is
     Latest again.
  3. For npm: `npm dist-tag add @zero-x.live/neuron@OLD latest`.
- **Gateway:** `wrangler rollback` (WP5).
- **Mobile:** stop the build in TestFlight, or halt the rollout in the Play
  Console. The app has no over-the-air updates.

## First runs and limits

- **Untested paths.** The macOS universal build, notarization and Windows
  signing have not run yet. Neither have the P1 platforms (Windows arm64
  cross-compiled on x64, MSI, AppImage and deb) and the updater artifacts.
  Their first runs are the dry runs above.
- **The Windows sidecar layout is confirmed.** A local NSIS build with
  `tauri.release.conf.json` produced an installer holding `neuron-desktop.exe`
  and `neuron.exe` side by side, where the app looks for the engine.
- **The engine name.** The desktop binary is named `neuron-desktop`
  (`mainBinaryName`), so it never collides with the engine's `neuron` on
  case-insensitive file systems.
- **The deb (P1)** installs the engine as `/usr/bin/neuron`. A CLI installed by
  `install.sh` in `~/.local/bin` may shadow it, or be shadowed by it, depending
  on `PATH`.
- **Actions** are pinned to major versions, like the rest of the repository.
  The mobile workflow takes the latest `eas-cli`; pin it after the first good
  build.
