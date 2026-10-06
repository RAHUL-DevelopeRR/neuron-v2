# Default skills and integrations

Every new Neuron system prompt contains the full Ponytail and AntiSlop core
instructions and the gstack router. The Skill tool and `/skills list` expose the
bundled companions: `antislop-ui`, `antislop-copywriting`, `antislop-code`,
`antislop-human`, `antislop-layoutmobile`, `gstack-investigate`, `gstack-review`,
`gstack-qa`, `gstack-qa-only` and `gstack-ship`.

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

`mcpServers` in `settings.json` configures real MCP processes or remote endpoints.
`/mcp list` shows configuration; actual initialization and tool discovery must
succeed before the model uses those tools. No preset name creates a server by
itself. For Microsoft's official browser server, install Node.js and configure:

```json
{
  "mcpServers": {
    "playwright": {
      "command": "npx",
      "args": ["@playwright/mcp@latest"]
    }
  }
}
```

See the [Playwright MCP documentation](https://github.com/microsoft/playwright-mcp)
for browser installation and platform requirements. This provides browser control.
Desktop control needs a separately installed computer-use MCP server, its actual
launch command or endpoint, and a model capable of interpreting its observations.
Neuron forwards discovered MCP tools to the model; it does not bundle a desktop
automation service or certify a model's computer-use capability.

## Agent Reach and CodebaseMap

[Agent Reach](https://github.com/Panniantong/Agent-Reach) is a CLI toolkit that can
configure additional integrations. It is not itself an MCP transport. Follow its
[official installation instructions](https://github.com/Panniantong/Agent-Reach/blob/main/docs/install.md),
then run `agent-reach doctor` to verify the installed channels. Neuron can invoke
available CLI tools through its command tool. Do not configure `agent-reach` as a
stdio MCP process unless a separately identified server implements that protocol.

CodebaseMap requires the exact project/server URL and its documented command or
endpoint. Its runtime status remains pending until that server is configured and
discovered. This prevents the model from claiming unavailable mapping tools.

Skills remain active as instructions throughout the conversation. Instructions
do not install integration runtimes, supply provider credentials, or bypass the
user's tool permissions. The gstack adapter preserves investigation and validation
gates while skipping upstream onboarding, telemetry and unavailable host helpers.
