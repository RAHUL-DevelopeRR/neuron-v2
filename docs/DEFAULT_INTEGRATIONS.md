# Default skills and integrations

Every new Neuron system prompt contains the full Ponytail and AntiSlop core
instructions and the gstack router. The Skill tool and `/skills list` expose the
bundled companions: `antislop-ui`, `antislop-copywriting`, `antislop-code`,
`antislop-human`, `antislop-layoutmobile`, `gstack-investigate`, `gstack-review`,
`gstack-qa`, `gstack-qa-only`, `gstack-ship` and `agent-reach`. Agent Reach's full
instructions are also active in every new system prompt.

Instructions are embedded in the native executable, then materialized under a
content-addressed `bundled-skills` directory in the runtime config home. npm and
PyPI installations need no source checkout or additional skill download.
`NEURON_CONFIG_HOME` selects the config home; otherwise Neuron uses
`~/.neuron` (`USERPROFILE/.neuron` on Windows). Existing `CLAW_CONFIG_HOME` remains
supported. Bundled defaults include their MIT licenses and pinned upstream commit
identifiers in `SOURCES.json`. Maintainers refresh them with
`python scripts/vendor-default-skills.py`; rebuilding the executable embeds the
updated files. `--manifest-only` regenerates the file list without network access.

Project and user skills take precedence over bundled names in the Skill tool.
Install a local skill with `/skills install <directory-with-SKILL.md>`.
Defaults can be disabled in `settings.json` with `{"defaultSkills": false}`.
Plugins remain configurable with `/plugins install`, `/plugins enable` and
`/plugins disable`. Plugin-managed Claude Code skills are not imported; install
their skill directories with `/skills install` instead.

## MCP and browser control

CodebaseMap (`DeusData/codebase-memory-mcp`, npm version `0.11.0`) and Microsoft's
Playwright MCP (`0.0.83`) are configured by default. npm installs their actual
dependencies alongside Neuron; the launcher passes absolute installed entry paths
to avoid downloading packages while a chat initializes. Standalone/PyPI binaries
use pinned `npx -y` commands, requiring Node.js and npm. On Windows, this fallback
runs through `cmd.exe` so `npx.cmd` resolves correctly.

`mcpServers` in `settings.json` overrides any default by server name and configures
additional processes or remote endpoints. Stdio servers use MCP's newline-delimited
JSON-RPC framing by default. For a legacy Content-Length server, set
`env.NEURON_MCP_FRAMING` to `content-length` in that server's config. Neuron sends
`notifications/initialized` after a successful initialize response. `/mcp list` shows effective configuration
with a `bundled` source for defaults. Actual initialization and tool discovery must
succeed before the model uses those tools. To disable both builtins, configure
`{"defaultMcpServers": false}`. To disable individual servers, configure:

```json
{
  "disabledMcpServers": ["playwright", "codebase-memory"]
}
```

See the [Playwright MCP documentation](https://github.com/microsoft/playwright-mcp)
for browser installation and platform requirements. This provides browser control.
Desktop control needs a separately installed computer-use MCP server, its actual
launch command or endpoint, and a model capable of interpreting its observations.
Neuron forwards discovered MCP tools to the model; it does not bundle a desktop
automation service or certify a model's computer-use capability.

## Agent Reach

[Agent Reach](https://github.com/Panniantong/Agent-Reach) routing instructions are
embedded and active on every conversation turn, together with its seven platform
references. The bundle uses upstream's English skill, MIT license and source
commit `a19a171fa980a0785849596492e0af4db800c82f`. Its host adapter checks actual
CLI availability and uses the operating system's temporary directory.
Maintainers can refresh just this project with
`python scripts/vendor-default-skills.py --project=agent-reach`.

Agent Reach is a CLI toolkit that can configure additional integrations. It is
not itself an MCP transport. Its instructions do not install channel executables
or supply authenticated browser sessions. Follow its
[official installation instructions](https://github.com/Panniantong/Agent-Reach/blob/main/docs/install.md),
then run `agent-reach doctor` to verify the installed channels. Neuron can invoke
available CLI tools through its command tool. Do not configure `agent-reach` as a
stdio MCP process unless a separately identified server implements that protocol.

CodebaseMap's default implementation is the official
[Codebase Memory MCP](https://github.com/DeusData/codebase-memory-mcp). Its mapping
and graph tools are available after successful MCP initialization. Configure a
native `codebase-memory-mcp` executable under that server name to avoid the npm
wrapper when using a standalone binary.

Skills remain active as instructions throughout the conversation. Instructions
do not install integration runtimes, supply provider credentials, or bypass the
user's tool permissions. The gstack adapter preserves investigation and validation
gates while skipping upstream onboarding, telemetry and unavailable host helpers.
