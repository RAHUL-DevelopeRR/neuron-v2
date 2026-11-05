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
the entry's `install.integrity` for P4 (verified installs). P1 starts packages with
`npx -y <package>@<version>` or `uvx <package>@<version>`, so only the version is
pinned today. Tool names come from each server's README (or, for Supabase, the tool
names in the 0.15.0 package); a tool the entry does not list gets the entry's
`default_tier`.

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
