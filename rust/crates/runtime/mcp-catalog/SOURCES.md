# Connector catalog: what was checked

`catalog.json` is compiled into `neuron` (`runtime/src/mcp_catalog.rs`) and is trusted
because it ships in the binary. It lists MCP servers that are published by the service's
own vendor or by the Model Context Protocol project, are open source, and were
maintained when the catalog was built. Nothing here is copied code: an entry names a
package, a pinned version, how to start it, and the permission tier of each tool.

Checked on 4 November 2026 against the GitHub API (`/repos/<owner>/<repo>`: licence,
archived flag, last push), the npm registry (`/<package>`: latest version, licence,
`dist.integrity`, install scripts) and PyPI (`/pypi/<package>/json`: version, licence,
wheel sha256). Remote endpoints were probed with an MCP `initialize` POST.

## Kept

| Id | Server | Licence | Pinned | Maintenance on 2026-11-04 | Checked at |
| --- | --- | --- | --- | --- | --- |
| `github` | GitHub's remote GitHub MCP Server | MIT | remote (vendor-hosted; source v2.1.0 released 2026-11-03) | last push 2026-11-04 | https://github.com/github/github-mcp-server ; `POST https://api.githubcopilot.com/mcp/` answered 401 without a token |
| `filesystem` | `@modelcontextprotocol/server-filesystem` | Apache-2.0 AND MIT (see below) | 2.1.0 (2026-10-28) | last push 2026-11-04 | https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem , https://registry.npmjs.org/@modelcontextprotocol/server-filesystem |
| `git` | `mcp-server-git` (PyPI) | MIT | 2026.10.28 | last push 2026-11-04 | https://github.com/modelcontextprotocol/servers/tree/main/src/git , https://pypi.org/project/mcp-server-git/ |
| `fetch` | `mcp-server-fetch` (PyPI) | MIT | 2026.10.28 | last push 2026-11-04 | https://github.com/modelcontextprotocol/servers/tree/main/src/fetch , https://pypi.org/project/mcp-server-fetch/ |
| `memory` | `@modelcontextprotocol/server-memory` | Apache-2.0 AND MIT (as filesystem) | 2.0.1 (2026-10-28) | last push 2026-11-04 | https://github.com/modelcontextprotocol/servers/tree/main/src/memory , https://registry.npmjs.org/@modelcontextprotocol/server-memory |
| `playwright` | `@playwright/mcp` (Microsoft) | Apache-2.0 | 0.0.83 (2026-09-28), the version Neuron bundles; 0.0.84 is latest | last push 2026-11-03 | https://github.com/microsoft/playwright-mcp , https://registry.npmjs.org/@playwright/mcp |
| `codebase-memory` | `codebase-memory-mcp` (DeusData) | MIT | 0.11.0 (2026-09-15), the version Neuron bundles; 0.12.1 is latest | last push 2026-11-03; the package runs `node install.js` on install (P4 checks it) | https://github.com/DeusData/codebase-memory-mcp , https://registry.npmjs.org/codebase-memory-mcp |
| `supabase` | Supabase's remote MCP server (`supabase/mcp`) | Apache-2.0 | remote (vendor-hosted; package 0.15.0 of 2026-10-29) | last push 2026-11-04 | https://github.com/supabase/mcp ; `POST https://mcp.supabase.com/mcp` answered 401 (OAuth) |
| `context7` | Upstash's remote Context7 server | MIT | remote (vendor-hosted; package 4.5.0 of 2026-11-03) | last push 2026-11-04 | https://github.com/upstash/context7 ; `POST https://mcp.context7.com/mcp` answered 200 without a key |
| `cloudflare-docs` | Cloudflare's documentation server | Apache-2.0 | remote (vendor-hosted; server reports `docs-ai-search` 0.5.0) | last push 2026-10-22 | https://github.com/cloudflare/mcp-server-cloudflare ; `initialize` and `tools/list` answered without auth |
| `aws-docs` | `awslabs.aws-documentation-mcp-server` (PyPI) | Apache-2.0 | 1.2.4 (2026-10-30) | last push 2026-11-04 | https://github.com/awslabs/mcp/tree/main/src/aws-documentation-mcp-server , https://pypi.org/project/awslabs.aws-documentation-mcp-server/ |

`filesystem` and `memory` are recorded as `Apache-2.0 AND MIT`, not as a choice of
either: rechecked on 5 November 2026, the npm metadata of 2.1.0 and 2.0.1 says "SEE
LICENSE IN LICENSE", neither tarball ships a licence file, the GitHub API reports the
repository licence as `NOASSERTION`, and the repository `LICENSE` says the project is
moving from MIT to Apache-2.0: new and relicensed contributions are Apache-2.0, and
contributions whose authors have not agreed to relicense stay MIT. Both licences
apply, each to its part.

The npm `sha512` integrity and the wheel `sha256` of each pinned package are recorded in
the entry's `install.integrity`. Settings name a local entry as `npx -y
<package>@<version>` or `uvx <package>@<version>`, but the server starts from a verified
install of what the entry pins (see "Verified installs" below), never through `npx` or
`uvx`. Tool names come from each server's README (or, for Supabase, the tool names in
the 0.15.0 package); a tool the entry does not list gets the entry's `default_tier`.

## Left out

| Candidate | Why | Checked at |
| --- | --- | --- |
| Sentry (`@sentry/mcp-server`, now `getsentry/toolkit`) | Licence is FSL-1.1-ALv2 (source-available, Apache-2.0 only after two years), not open source today | https://registry.npmjs.org/@sentry/mcp-server (0.42.0) |
| Notion (`@notionhq/notion-mcp-server`) | Its README says the local server "is no longer actively maintained or supported" and points to Notion's hosted server, which is not open source. Left for hosted connectors (P5) | https://github.com/makenotion/notion-mcp-server (MIT, 2.6.1) |
| Linear, Atlassian | Vendor-hosted remote servers whose code is not published. Left for hosted connectors (P5) | - |
| Slack, Google Drive | The reference servers were archived (`modelcontextprotocol/servers-archived`, archived, last push 2025-05-28) and there is no maintained open-source server from the vendor. Left for hosted connectors (P5) | https://github.com/modelcontextprotocol/servers-archived |
| Postgres | The reference server is archived (as above); `crystaldba/postgres-mcp` (MIT, last push 2026-08-17) is not a vendor's; Google's `googleapis/mcp-toolbox` (Apache-2.0, active) is a release binary that needs a tools file, so it waits for binary installs (P4) | https://github.com/crystaldba/postgres-mcp , https://github.com/googleapis/mcp-toolbox |

## Tiers

`read` reads and changes nothing. `write` creates or changes something that can be put
back. `destructive` deletes, overwrites, deploys, runs arbitrary code or SQL, or costs
money. `external` reaches arbitrary hosts or acts on the outside world in ways the
entry cannot bound. An entry marked `writes_outside_workspace` (Memory, whose graph
lives in its package folder or at `MEMORY_FILE_PATH`, and Codebase memory, whose index
lives in the user's cache folder) has its `write` tools ask for full access, as a remote
server's do. See `docs/DEFAULT_INTEGRATIONS.md` for how a tier becomes a permission.

## Sandbox profiles

Each entry's `sandbox` is what its stdio server may read (`fs_read`), write
(`fs_write`), reach (`network`, `*` for any) and inherit (`env_allow`); see
`runtime/src/mcp_sandbox.rs`. Version `2026-11-05.1` changed two entries, checked on
5 November 2026 against the npm packages in the local npx cache:

- Playwright (`@playwright/mcp@0.0.83`) keeps its browsers and the browser profile it
  creates in Playwright's cache folder (`ms-playwright` under `%LOCALAPPDATA%`,
  `~/Library/Caches` or `~/.cache`), reads files the user names in the workspace, and
  needs the display variables on Linux.
- Codebase memory (`codebase-memory-mcp@0.11.0`) keeps its index in
  `~/.cache/codebase-memory-mcp` on every platform. Its `bin.js` downloads the release
  binary from GitHub when the install script did not run, and GitHub release downloads
  redirect to `*.githubusercontent.com`.

Version `2026-11-05.2`, checked the same day against `codebase-memory-mcp@0.11.0`
(`bin.js`, `install.js`): `install.js` writes the binary into the package's own `bin`
folder (`path.join(__dirname, 'bin')`), so a copy installed with `--ignore-scripts`, or
whose install script failed, writes it there on first start. The entry's `fs_write`
adds `${package}/bin`: the `bin` folder of the npm package whose script the server
runs (skipped when Neuron cannot see that script, as with `npx`, whose cache the entry
may write because it reaches `registry.npmjs.org`).

## Verified installs

Version `2026-11-05.3` (5 November 2026) adds what each local entry installs, so the
catalog signature covers it and `runtime/src/mcp_install/verified.rs` checks every
download against it:

- npm entries (`filesystem`, `memory`, `playwright`): `lock` is the dependency tree
  from `npm install --package-lock-only --ignore-scripts --save-exact
  <package>@<version>` (npm 11.6.2) in an empty folder, with each tarball's registry URL
  and `sha512` integrity; `bin` is the package's program from its `bin` field. The
  locked package's own integrity equals `install.integrity`. None of the locked packages
  declares an install script.
- Python entries (`git`, `fetch`, `aws-docs`): `requirements` is the output of `uv pip
  compile --universal --generate-hashes --python-version 3.10` (uv 0.13.6) for
  `<package>==<version>`: every requirement with its environment marker and the
  `sha256` of each wheel and sdist PyPI lists for it. The server's own wheel hash equals
  `install.integrity`. Neuron installs wheels only (`--only-binary :all:`), so no build
  script runs.
- `codebase-memory`: the npm package (`bin.js`, `install.js`, read on 5 November 2026)
  holds no server code. `install.js`, its `postinstall` script, downloads
  `codebase-memory-mcp-<os>-<arch>[-portable].<zip|tar.gz>` from the GitHub release
  `v0.11.0`, checks it against the release's own `checksums.txt` (from the same release,
  so it is not pinned anywhere), and unpacks the one executable into the package's `bin`
  folder; `bin.js` reruns `install.js` when that executable is missing (as after
  `--ignore-scripts`) and then runs it. The script is therefore required to get the
  server, but its download is not pinned. The entry's `release` instead pins each
  platform's archive by the `sha256` in
  https://github.com/DeusData/codebase-memory-mcp/releases/download/v0.11.0/checksums.txt
  (read on 5 November 2026; Linux uses the `-portable` build as `install.js` does), and
  Neuron downloads the archive, checks it, and unpacks only the executable. Neither
  `install.js` nor `bin.js` runs, and the npm package is never fetched.

Release asset sha256 for v0.11.0:

| Platform | Asset | sha256 |
| --- | --- | --- |
| windows-x86_64 | `codebase-memory-mcp-windows-amd64.zip` | `6eb6beaf261b19e419766e78baf93cbc3cf1c6338cff8fb7c0234859f96d1685` |
| windows-aarch64 | `codebase-memory-mcp-windows-arm64.zip` | `52b29881214fce47d529e098308b1de77c40f6812e20a34d588ee4c25b84fd1a` |
| macos-x86_64 | `codebase-memory-mcp-darwin-amd64.tar.gz` | `dbf1c73bfcbde64e7dde4cd1320da7afc02e2c972ee1789ae039521411f5132e` |
| macos-aarch64 | `codebase-memory-mcp-darwin-arm64.tar.gz` | `4dee7f38b63740e6751d7a7ed7eb10291c1f2a3ea2415f599dc68370ca0a2d18` |
| linux-x86_64 | `codebase-memory-mcp-linux-amd64-portable.tar.gz` | `1f9e8293eb2bc5c05cfa27a7e8fc033da6d729ffad525ccfcdaa3fd606306683` |
| linux-aarch64 | `codebase-memory-mcp-linux-arm64-portable.tar.gz` | `d62eeb224d5ee3eba3070938ec62cf1033f10b041ec1c4b2fb67f7aef390cc7b` |

Version `2026-11-05.4` (5 November 2026) adds `install.data` to `memory`. Checked
against `@modelcontextprotocol/server-memory@2.0.1` (`dist/index.js` on unpkg): its
default file is `path.join(path.dirname(fileURLToPath(import.meta.url)),
"memory.jsonl")`, the package's own `dist` folder, and a relative `MEMORY_FILE_PATH`
is joined to that folder too. With verified installs that folder is the install, which
a reinstall replaces and the sandbox does not let it write, so Neuron points the
variable at `<config home>/mcp-data/memory/memory.jsonl` instead.

To update an entry, regenerate its `lock` or `requirements` the same way for the new
version, update `install.integrity` and `release`, and bump the catalog `version`;
`McpCatalog::parse` refuses an entry whose own package is missing from its lock or
requirements, or whose downloads lack a hash.
